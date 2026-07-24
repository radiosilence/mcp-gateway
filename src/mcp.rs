//! Mount the Fastmail MCP (from the `fastmail-cli` core) over streamable HTTP,
//! behind a bearer-auth layer that resolves the caller's Fastmail token and
//! injects it as the header the core trusts.
//!
//! Flow per request:
//!   Authorization: Bearer <hydra JWT>
//!     → verify vs Hydra JWKS → sub
//!     → store.get_token(sub) → decrypt
//!     → set X-Fastmail-Token header → hand to the core MCP service
//!
//! No valid bearer ⇒ 401 with an RFC 9728 `WWW-Authenticate` pointing at our
//! protected-resource metadata, which is what makes Claude start the OAuth
//! dance.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use fastmail_cli::mcp::{FastmailMcp, TOKEN_HEADER};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::state::AppState;

pub fn router(state: AppState) -> Router<AppState> {
    // One shared core instance (shared client cache) cloned per session.
    let template = FastmailMcp::hosted();
    let mcp_service = StreamableHttpService::new(
        move || Ok(template.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(from_fn_with_state(state, bearer_auth))
}

async fn bearer_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let token = match bearer(&req) {
        Some(t) => t,
        None => return challenge(&state, "missing bearer token"),
    };

    // Opaque token → introspect at Hydra → subject. No JWT, revocable.
    let sub = match state.hydra.introspect(&token).await {
        Ok(Some(sub)) => sub,
        Ok(None) => return challenge(&state, "inactive token"),
        Err(e) => {
            tracing::debug!(error = %e, "introspection failed");
            return challenge(&state, "introspection failed");
        }
    };

    // Resolve the caller's Fastmail token. Absent is not fatal: let the MCP
    // handshake/tool-listing proceed; graphql calls return a friendly
    // "set your token in the dashboard" error from the core.
    match state.store.get_token(&sub).await {
        Ok(Some(fm_token)) => {
            if let Ok(value) = HeaderValue::from_str(&fm_token) {
                req.headers_mut()
                    .insert(axum::http::HeaderName::from_static(TOKEN_HEADER), value);
            }
        }
        Ok(None) => tracing::debug!(%sub, "no fastmail token stored"),
        Err(e) => tracing::error!(error = %e, "token lookup failed"),
    }

    next.run(req).await
}

fn bearer(req: &Request) -> Option<String> {
    let raw = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(str::to_owned)
}

/// 401 with the RFC 9728 resource-metadata pointer that triggers OAuth in Claude.
fn challenge(state: &AppState, _reason: &str) -> Response {
    let metadata_url = format!(
        "{}/.well-known/oauth-protected-resource",
        state.config.public_url.trim_end_matches('/')
    );
    let header_value = format!("Bearer resource_metadata=\"{metadata_url}\"");
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, header_value)],
    )
        .into_response()
}
