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
mod state;
mod store;
mod well_known;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::http::{HeaderValue, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
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

/// Shown in each page's footer; the same string as the image tag and release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Embedded rather than loaded from a CDN: credentials are typed into these
/// pages, so no third-party script gets DOM access to them.
const APP_CSS: &str = include_str!("../assets/app.css");
/// `web/app.ts` and the vendored Datastar, bundled (`mise run web`).
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

/// Everything comes from this origin and nothing is inline; `data:` covers the
/// SVG favicon.
///
/// `unsafe-eval` is required by Datastar, which compiles each `data-*`
/// expression with `Function()`. It means a `data-*` attribute rendered from an
/// unescaped value would execute, so escaping is the control:
/// `no_template_opts_out_of_escaping` guards it.
const CSP: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-eval'; \
     style-src 'self'; \
     img-src 'self' data:; \
     connect-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     base-uri 'none'; \
     object-src 'none'";

/// Applied to every response, including the proxy's, which they do not affect.
async fn security_headers(request: axum::extract::Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
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
    };

    // Only the login round trip is rate limited, keyed on the client IP from
    // the ingress's forwarding headers. `/login` writes a row per request.
    let auth_governor = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(10)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("valid governor config for auth routes");

    // The keyed limiter keeps an entry per IP until told to drop idle ones.
    let auth_limiter = auth_governor.limiter().clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            auth_limiter.retain_recent();
        }
    });

    // Every replica sweeping is harmless: the delete is idempotent.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3_600));
        loop {
            interval.tick().await;
            if let Err(e) = store.delete_expired().await {
                tracing::warn!(error = %e, "sweeping expired sessions failed");
            }
        }
    });

    let auth_routes = Router::new()
        .route("/login", get(auth::routes::login))
        .route("/auth/callback", get(auth::routes::callback))
        .layer(GovernorLayer::new(auth_governor));

    let app = Router::new()
        .route("/", get(dashboard::index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/app.css", get(app_css))
        .route("/assets/app.js", get(app_js))
        .route("/logout", post(auth::routes::logout))
        .route("/dashboard", get(dashboard::dashboard))
        .route("/dashboard/{mcp_id}/token", post(dashboard::set_credential))
        .route("/dashboard/{mcp_id}/status", get(dashboard::mcp_status))
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
        .route(
            "/.well-known/oauth-protected-resource",
            get(well_known::protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/{id}",
            get(well_known::protected_resource_for),
        )
        // MCPs live at the root; config rejects ids that shadow routes above.
        .route("/{id}", any(proxy::handle))
        .merge(auth_routes)
        .layer(middleware::from_fn(security_headers))
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
        use axum::http::Request;
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { "hi" }))
            .layer(middleware::from_fn(security_headers));

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
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    }

    /// Plugin names the bundle registers, whichever quote the minifier chose.
    fn registered_plugins() -> Vec<&'static str> {
        let names: Vec<&str> = APP_JS
            .match_indices("name:")
            .filter_map(|(i, m)| {
                let rest = &APP_JS[i + m.len()..];
                let quote = rest
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '"' | '\'' | '`'))?;
                let rest = &rest[1..];
                rest.find(quote).map(|end| &rest[..end])
            })
            .collect();
        assert!(!names.is_empty(), "found no plugin names in the bundle");
        names
    }

    /// The generated assets are committed; a truncated build would still
    /// compile and only show up as a broken page.
    #[test]
    fn the_embedded_assets_are_real() {
        assert!(APP_CSS.contains("box-sizing:border-box"), "no preflight");
        assert!(APP_CSS.contains("max-w-2xl"), "utilities missing");
        assert!(
            APP_JS.contains("datastar-fetch") && registered_plugins().contains(&"copy"),
            "app.js is not the bundle"
        );
    }

    /// An attribute naming no Datastar plugin is ignored silently (there is no
    /// `data-on-load`, only `data-init`). Names come from the vendored bundle,
    /// so an upgrade that renames one fails here.
    #[test]
    fn every_datastar_attribute_is_one_datastar_matches() {
        let registered = registered_plugins();
        assert!(
            registered.contains(&"init") && registered.contains(&"on"),
            "the bundle registers neither `init` nor `on`; the scan is wrong, not the templates"
        );

        // Read by expressions; Datastar ignores them.
        const OURS: &[&str] = &["confirm", "url"];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        for entry in std::fs::read_dir(&dir).expect("templates/") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let body = std::fs::read_to_string(&path).expect("readable");
            for chunk in body.split(" data-").skip(1) {
                let attr: String = chunk
                    .chars()
                    .take_while(|c| !matches!(c, '=' | ' ' | '"' | '>' | '\n'))
                    .collect();
                // `data-on:submit__prevent` names the `on` plugin; the key and
                // the modifiers are its arguments.
                let plugin = attr
                    .split(&[':', '_'][..])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    registered.contains(&plugin.as_str()) || OURS.contains(&plugin.as_str()),
                    "{name}: `data-{attr}` names no Datastar plugin — it will be ignored silently"
                );
            }
        }
    }

    /// With eval allowed, escaping is what keeps backend-supplied strings (option
    /// labels, error text) from executing. See `CSP`.
    #[test]
    fn no_template_opts_out_of_escaping() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("templates/") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            for line in std::fs::read_to_string(&path).expect("readable").lines() {
                // `safe-area-inset` in a Tailwind class is not the filter.
                if line.contains("|safe") || line.contains("escape(") {
                    found.push(format!("{name}: {}", line.trim()));
                }
            }
        }
        found.sort();
        assert_eq!(found, Vec::<String>::new());
    }
}
