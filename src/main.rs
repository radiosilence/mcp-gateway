//! Hosted, OAuth-fronted MCP server for Fastmail.
//!
//! One axum process that is, in Model A terms, the whole service: it terminates
//! the user's OAuth session (via Hydra), stores their Fastmail token encrypted,
//! serves a dashboard to manage it, and mounts the `fastmail-cli` MCP over
//! streamable HTTP with the token injected per request.

mod auth;
mod config;
mod crypto;
mod dashboard;
mod error;
mod mcp;
mod state;
mod store;
mod well_known;

use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::auth::hydra::HydraAdmin;
use crate::auth::jwks::JwksCache;
use crate::config::Config;
use crate::crypto::Cipher;
use crate::state::AppState;
use crate::store::Store;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fastmail_mcp_service=info,tower_http=info,warn".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();

    let cipher = Cipher::new(&config.token_enc_key)?;
    let store = Store::connect(&config.database_url, cipher).await?;
    store.migrate().await?;
    tracing::info!("database connected & migrated");

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let jwks = JwksCache::new(&config.hydra_public_url, http.clone());
    let hydra = HydraAdmin::new(&config.hydra_admin_url, http.clone());

    let state = AppState {
        config: Arc::new(config),
        store,
        http,
        jwks,
        hydra,
    };

    let app = Router::new()
        .route("/", get(dashboard::index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/login", get(auth::routes::login))
        .route("/logout", get(auth::routes::logout))
        .route("/dashboard", get(dashboard::dashboard))
        .route("/dashboard/token", post(dashboard::set_token))
        .route("/dashboard/token/delete", post(dashboard::delete_token))
        .route("/dashboard/token/test", post(dashboard::test_token))
        .route("/auth/login", get(auth::routes::hydra_login))
        .route("/auth/consent", get(auth::routes::hydra_consent))
        .route("/auth/github/callback", get(auth::routes::github_callback))
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known::protected_resource),
        )
        .merge(mcp::router(state.clone()))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("listening on http://{bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
