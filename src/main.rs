//! Hosted, OAuth-fronted gateway for self-hosted MCP servers.
//!
//! One axum process that is, in Model A terms, the whole service: it terminates
//! the user's OAuth session (via Hydra), stores their per-MCP credentials
//! encrypted, serves a dashboard to manage them, and proxies to each backend
//! MCP with the right values injected per request.

mod auth;
mod backend;
mod config;
mod crypto;
mod dashboard;
mod error;
mod proxy;
mod register;
mod state;
mod store;
mod well_known;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{any, get, patch, post};
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::auth::hydra::HydraAdmin;
use crate::config::Config;
use crate::crypto::Cipher;
use crate::state::AppState;
use crate::store::Store;

/// Baked into the binary rather than fetched from a CDN. These pages sit behind
/// the login of a service that holds people's credentials, and they are typed
/// into it in plaintext — a third-party script with DOM access there reads them
/// as they are entered. Embedding also keeps the deployment a single artefact,
/// with no second thing to serve or keep in step.
const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");

async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mcp_gateway=info,tower_http=info,warn".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();

    if config.github_allowlist.is_empty() {
        tracing::warn!(
            "GH_ALLOWED is empty — ANY GitHub user can authenticate. Set it \
             to a comma-separated list of allowed logins before public deploy."
        );
    }

    let cipher = Cipher::new(&config.token_enc_key)?;
    let store = Store::connect(&config.database_url, cipher).await?;
    store.migrate().await?;
    tracing::info!("database connected & migrated");

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let hydra = HydraAdmin::new(&config.hydra_admin_url, http.clone());

    let state = AppState {
        config: Arc::new(config),
        store,
        http,
        hydra,
    };

    // Rate limiting: only on the abuse-prone entry points (DCR + the GitHub
    // OAuth bounce), keyed on the client IP as seen through Traefik
    // (X-Forwarded-For / X-Real-IP), never the proxy peer address. The MCP
    // proxy routes and /healthz are deliberately left unthrottled.
    let register_governor = GovernorConfigBuilder::default()
        .per_second(10)
        .burst_size(5)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("valid governor config for /register");
    let auth_governor = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(10)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("valid governor config for auth routes");

    // governor's keyed rate limiter accumulates one entry per client IP;
    // periodically drop entries with no recent activity so it doesn't grow
    // unbounded.
    let register_limiter = register_governor.limiter().clone();
    let auth_limiter = auth_governor.limiter().clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            register_limiter.retain_recent();
            auth_limiter.retain_recent();
        }
    });

    let register_routes = Router::new()
        .route("/register", post(register::register))
        .layer(GovernorLayer::new(register_governor));

    let auth_routes = Router::new()
        .route("/auth/login", get(auth::routes::hydra_login))
        .route("/auth/github/callback", get(auth::routes::github_callback))
        .layer(GovernorLayer::new(auth_governor));

    let app = Router::new()
        .route("/", get(dashboard::index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/app.css", get(app_css))
        .route("/assets/app.js", get(app_js))
        .route("/login", get(auth::routes::login))
        .route("/logout", get(auth::routes::logout))
        .route("/dashboard", get(dashboard::dashboard))
        .route("/dashboard/{mcp_id}/token", post(dashboard::set_credential))
        .route(
            "/dashboard/{mcp_id}/options/{field_id}",
            get(dashboard::field_options),
        )
        .route(
            "/dashboard/{mcp_id}/field/{field_id}",
            patch(dashboard::set_field),
        )
        .route(
            "/dashboard/{mcp_id}/delete",
            post(dashboard::delete_credential),
        )
        .route("/auth/consent", get(auth::routes::hydra_consent))
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known::protected_resource),
        )
        // MCPs live at the root (`/fastmail`); a reserved-id guard at config
        // load keeps them from shadowing the gateway's own routes above.
        .route("/{id}", any(proxy::handle))
        .merge(register_routes)
        .merge(auth_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("listening on http://{bind_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stylesheet is generated, so the thing worth guarding is that a real
    /// one got committed — an empty or truncated build would still compile in
    /// and only show up as an unstyled page in production.
    #[test]
    fn the_embedded_assets_are_real() {
        assert!(APP_CSS.contains("box-sizing:border-box"), "no preflight");
        assert!(APP_CSS.contains("max-w-2xl"), "utilities missing");
        assert!(
            APP_JS.contains("data-copy"),
            "app.js is not the real script"
        );
    }
}
