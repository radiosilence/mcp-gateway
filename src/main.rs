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

/// The build serving these pages, shown in the footer of each one.
///
/// `Cargo.toml`'s version is what a release is here, so this is the same string
/// the image tag and the GitHub release carry — which is the point: neither of
/// those is visible from a browser, and "which build am I looking at" is the
/// first question a dashboard behaving oddly raises.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Baked into the binary rather than fetched from a CDN. These pages sit behind
/// the login of a service that holds people's credentials, and they are typed
/// into it in plaintext — a third-party script with DOM access there reads them
/// as they are entered. Embedding also keeps the deployment a single artefact,
/// with no second thing to serve or keep in step.
const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
/// Vendored rather than fetched: same reasoning as the rest, and a pinned copy
/// in the tree is also the only version anyone can be served. The same v1.0.2
/// build mariastew carries, so the two cannot drift onto different semantics
/// for the same attribute.
const DATASTAR_JS: &str = include_str!("../assets/datastar.js");

async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

/// Everything the pages need comes from this origin, so the policy can simply
/// say so. No `unsafe-inline` anywhere: the stylesheet and script are served as
/// files and no handler is written into the markup, which is the whole reason
/// they were moved out. `data:` covers the inline SVG favicon.
///
/// `unsafe-eval` is Datastar's price, and is not optional: it compiles each
/// `data-*` expression with `Function()`, so without it no attribute on this
/// page does anything. Serving the bundle ourselves does not change that — eval
/// is about turning strings into code, not about where the file came from.
///
/// What it costs is worth stating plainly, because this is the page people type
/// their credentials into. `script-src 'self'` still refuses script from
/// anywhere else, so nothing new can be *loaded*. What changes is that a
/// `data-*` attribute built from an unescaped value would be executable, and
/// this line would no longer be what stopped it. Askama escapes by default and
/// nothing here opts out; `no_template_opts_out_of_escaping` is the control that
/// used to have this as a backstop and now stands on its own.
///
/// Shared verbatim with mariastew, which reached the same policy from the other
/// direction — it served no headers at all until this comparison was made.
const CSP: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-eval'; \
     style-src 'self'; \
     img-src 'self' data:; \
     connect-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     base-uri 'none'; \
     object-src 'none'";

/// Applied to every response. Harmless on the JSON and SSE the proxy returns,
/// and cheaper to reason about than deciding per route which ones render HTML.
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
    // Redundant beside frame-ancestors for anything current, and free.
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

async fn datastar_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        DATASTAR_JS,
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

    // Rate limiting: only on the abuse-prone entry point — the bounce out to
    // the issuer and back — keyed on the client IP as seen through Traefik
    // (X-Forwarded-For / X-Real-IP), never the proxy peer address. The MCP
    // proxy routes and /healthz are deliberately left unthrottled.
    let auth_governor = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(10)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("valid governor config for auth routes");

    // governor's keyed rate limiter accumulates one entry per client IP;
    // periodically drop entries with no recent activity so it doesn't grow
    // unbounded.
    let auth_limiter = auth_governor.limiter().clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            auth_limiter.retain_recent();
        }
    });

    let auth_routes = Router::new()
        .route("/auth/callback", get(auth::routes::callback))
        .layer(GovernorLayer::new(auth_governor));

    let app = Router::new()
        .route("/", get(dashboard::index))
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/app.css", get(app_css))
        .route("/assets/app.js", get(app_js))
        .route("/assets/datastar.js", get(datastar_js))
        .route("/login", get(auth::routes::login))
        .route("/logout", get(auth::routes::logout))
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
        // MCPs live at the root (`/fastmail`); a reserved-id guard at config
        // load keeps them from shadowing the gateway's own routes above.
        .route("/{id}", any(proxy::handle))
        .merge(auth_routes)
        .layer(middleware::from_fn(security_headers))
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
    /// The policy is only worth anything while it has no inline escape hatch —
    /// which is exactly what someone adds to make a stray inline script work.
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
        // Datastar cannot work without the eval hatch, so that one is stated
        // and argued for above. `unsafe-inline` is the one nothing here needs,
        // and it is what someone reaches for to make a stray inline handler
        // work — which is exactly the change this should refuse.
        assert!(
            csp.contains("'unsafe-eval'"),
            "datastar needs the eval hatch"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    }

    #[test]
    fn the_embedded_assets_are_real() {
        assert!(APP_CSS.contains("box-sizing:border-box"), "no preflight");
        assert!(APP_CSS.contains("max-w-2xl"), "utilities missing");
        assert!(
            APP_JS.contains("data-copy"),
            "app.js is not the real script"
        );
        assert!(
            DATASTAR_JS.starts_with("// Datastar v1.0.2"),
            "not the pinned datastar build"
        );
    }

    /// Datastar's own triggers are hyphenated (`data-on-interval`) and DOM
    /// events are not (`data-on:click`). Getting that backwards is not an
    /// error: the attribute matches no plugin, so the control renders, looks
    /// right, and does nothing. `data-on-load` is the trap this migration was
    /// one keystroke from — there is no `load` plugin at all, only `init`.
    ///
    /// The names are read out of the vendored bundle rather than copied from
    /// documentation, so a version bump that renames or drops one fails here
    /// rather than in a browser.
    #[test]
    fn every_datastar_attribute_is_one_datastar_matches() {
        let registered: Vec<&str> = DATASTAR_JS
            .match_indices("name:\"")
            .filter_map(|(i, m)| {
                let rest = &DATASTAR_JS[i + m.len()..];
                rest.find('"').map(|end| &rest[..end])
            })
            .collect();
        assert!(
            registered.contains(&"init") && registered.contains(&"on"),
            "the bundle registers neither `init` nor `on`; the scan is wrong, not the templates"
        );

        // Ours, read by app.js. Datastar ignores it and so should this.
        const OURS: &[&str] = &["copy"];

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

    /// With eval allowed, escaping is the only thing between a string that
    /// reaches the DOM and code that runs: Datastar compiles `data-*`
    /// attributes, so an unescaped value rendered into one is executable by
    /// design. Backends supply strings this page renders — option labels and
    /// values, and the error text a failed lookup shows — so this is not
    /// hypothetical.
    ///
    /// Askama escapes by default. What needs watching is the opt-outs, and
    /// there are none: a `|safe` here would have to be argued for, and this is
    /// where the argument gets made.
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
