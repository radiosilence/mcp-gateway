//! OAuth Protected Resource Metadata (RFC 9728).
//!
//! This is the only OAuth metadata *we* serve: it names Hydra as the
//! authorization server. Claude fetches this after a 401, then talks OAuth
//! directly to Hydra (DCR, PKCE, token) — we never proxy those endpoints.
//!
//! Each MCP is its own resource (`{public_url}/{id}`), with its metadata at
//! the path RFC 9728 derives from that URL. Clients check that the document's
//! `resource` matches the URL they connected to, so the gateway-wide document
//! alone is rejected by the strict ones.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::state::AppState;

pub async fn protected_resource(State(state): State<AppState>) -> Json<Value> {
    metadata(&state, state.config.public_url.trim_end_matches('/'))
}

pub async fn protected_resource_for(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    if state.config.mcp(&id).is_none() {
        return (StatusCode::NOT_FOUND, "unknown mcp").into_response();
    }
    metadata(&state, &resource_url(&state, &id)).into_response()
}

fn resource_url(state: &AppState, id: &str) -> String {
    format!("{}/{id}", state.config.public_url.trim_end_matches('/'))
}

pub fn metadata_url(state: &AppState, id: &str) -> String {
    format!(
        "{}/.well-known/oauth-protected-resource/{id}",
        state.config.public_url.trim_end_matches('/')
    )
}

fn metadata(state: &AppState, resource: &str) -> Json<Value> {
    let issuer = state.config.hydra_issuer.trim_end_matches('/');
    Json(json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["openid", "offline", "offline_access"],
    }))
}
