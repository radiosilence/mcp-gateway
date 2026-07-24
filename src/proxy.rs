//! Streaming reverse proxy: `/mcp/{id}` → the backend MCP for `{id}`.
//!
//! Per request: introspect the bearer at Hydra → `sub`, look up + decrypt the
//! user's credential for this MCP, strip any client-supplied copy of that
//! header, inject the real one, forward, and stream the response back
//! (MCP streamable-HTTP responses can be SSE, so the body is streamed, not
//! buffered).
//!
//! No valid bearer ⇒ 401 + RFC 9728 `WWW-Authenticate`, which starts Claude's
//! OAuth dance.

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

pub async fn handle(
    State(state): State<AppState>,
    Path(id): Path<String>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(mcp) = state.config.mcp(&id) else {
        return (StatusCode::NOT_FOUND, "unknown mcp").into_response();
    };

    // Authenticate: opaque bearer → introspect → subject.
    let Some(token) = bearer(&headers) else {
        return challenge(&state);
    };
    let sub = match state.hydra.introspect(&token).await {
        Ok(Some(sub)) => sub,
        Ok(None) => return challenge(&state),
        Err(e) => {
            tracing::debug!(error = %e, "introspection failed");
            return challenge(&state);
        }
    };

    // Forward headers minus hop-by-hop, auth, host, length — and critically the
    // credential header, so a client can't smuggle its own.
    let mut fwd = headers.clone();
    fwd.remove(header::AUTHORIZATION);
    fwd.remove(header::HOST);
    fwd.remove(header::CONTENT_LENGTH);
    fwd.remove(mcp.credential_header.as_str());
    for h in HOP_BY_HOP {
        fwd.remove(h);
    }

    let mut req = state
        .http
        .request(method, &mcp.backend)
        .headers(fwd)
        .body(body.to_vec());

    // Inject the user's credential for this MCP (absent is not fatal — the
    // backend returns its own "no token" error the model can relay).
    match state.store.get_credential(&sub, &id).await {
        Ok(Some(secret)) => req = req.header(mcp.credential_header.as_str(), secret),
        Ok(None) => tracing::debug!(%sub, mcp = %id, "no credential stored"),
        Err(e) => tracing::error!(error = %e, "credential lookup failed"),
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, mcp = %id, "backend request failed");
            return (StatusCode::BAD_GATEWAY, "backend unavailable").into_response();
        }
    };

    // Relay status + headers, stream the body (SSE-safe).
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_headers = upstream.headers().clone();
    resp_headers.remove(header::CONTENT_LENGTH);
    for h in HOP_BY_HOP {
        resp_headers.remove(h);
    }

    let mut out = Response::new(Body::from_stream(upstream.bytes_stream()));
    *out.status_mut() = status;
    *out.headers_mut() = resp_headers;
    out
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

fn challenge(state: &AppState) -> Response {
    let metadata_url = format!(
        "{}/.well-known/oauth-protected-resource",
        state.config.public_url.trim_end_matches('/')
    );
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            format!("Bearer resource_metadata=\"{metadata_url}\""),
        )],
    )
        .into_response()
}
