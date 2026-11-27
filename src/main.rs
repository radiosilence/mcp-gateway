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
mod error;
mod proxy;
mod state;
mod store;
mod web;
mod well_known;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use askama_datastar::{Csp, SecurityHeaders};
use axum::Router;
use axum::routing::{any, get};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::auth::hydra::HydraAdmin;
use crate::config::Config;
use crate::crypto::Cipher;
use crate::state::AppState;
use crate::store::Store;

/// Shown in each page's footer; the same string as the image tag and release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything comes from this origin and nothing is inline; `data:` covers the
/// SVG favicon.
///
/// `unsafe-eval` is required by Datastar, which compiles each `data-*`
/// expression with `Function()`. It means a `data-*` attribute rendered from an
/// unescaped value would execute, so escaping is the control, which the
/// escaping guard in `web::escaping` checks.
fn csp() -> Csp {
    Csp::new().datastar()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mcp_gateway=info,tower_http=info,warn".into()),
        )
        .init();

    let mut config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();

    // Only the cipher needs the key, so it is not kept in the shared config.
    let cipher = Cipher::new(&std::mem::take(&mut config.token_enc_key))?;
    let store = Store::connect(&config.database_url, cipher).await?;
    store.migrate().await?;
    store.reseal_legacy().await?;
    tracing::info!("database connected & migrated");

    // Redirects are not followed: the dashboard's backend calls carry injected
    // credentials, and reqwest strips only the standard auth headers when a
    // redirect changes host.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    // A total timeout would cut SSE streams, so the proxy uses a read timeout,
    // which resets on every chunk. It is long because a tool call answered as
    // plain JSON sends nothing until it has finished.
    //
    // Redirects are relayed rather than followed: following one would send the
    // injected credentials to whichever host it names.
    let proxy_http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let hydra = HydraAdmin::new(&config.hydra_admin_url, &config.public_url, http.clone())?;

    let state = AppState {
        config: Arc::new(config),
        store: store.clone(),
        http,
        proxy_http,
        hydra,
        questions: Default::default(),
    };

    // Every replica sweeping is harmless: both steps are idempotent.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3_600));
        loop {
            interval.tick().await;
            if let Err(e) = store.delete_expired().await {
                tracing::warn!(error = %e, "sweeping expired sessions failed");
            }
            // Binds credentials older replicas wrote during a rollout.
            if let Err(e) = store.reseal_legacy().await {
                tracing::warn!(error = %e, "resealing legacy credentials failed");
            }
        }
    });

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(web::routes())
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known::protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/{id}",
            get(well_known::protected_resource_for),
        )
        // MCPs live at the root; config rejects ids that shadow routes above.
        // Proxied responses pass through as the backend encoded them.
        .route("/{id}", any(proxy::handle))
        // Applied to every response, the proxy's included: `proxy` drops a
        // backend's own security headers so these stand.
        .layer(SecurityHeaders::new(&csp()))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("listening on http://{bind_addr}");
    let (stopping_tx, stopping) = tokio::sync::oneshot::channel();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _ = stopping_tx.send(());
    })
    .into_future();
    tokio::pin!(server);

    // An open SSE stream never finishes, so draining every connection would
    // wait out the orchestrator's grace period. Requests get a few seconds,
    // then returning drops the runtime and cancels what is left.
    tokio::select! {
        result = &mut server => result?,
        _ = async {
            if stopping.await.is_err() {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        } => tracing::info!("connections still open after the drain period; exiting"),
    }
    Ok(())
}

/// The binary is PID 1 in a scratch image, where the kernel ignores SIGTERM
/// unless a handler is installed.
async fn shutdown_signal() {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        Ok(()) = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        use axum::body::Body;
        use axum::http::{Request, header};
        use tower::ServiceExt;

        let app: Router = Router::new()
            .route("/", get(|| async { "hi" }))
            .layer(SecurityHeaders::new(&csp()));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(
            !csp.contains("unsafe-inline"),
            "csp has an inline escape hatch"
        );
        assert!(
            csp.contains("'unsafe-eval'"),
            "datastar needs the eval hatch"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    }
}
