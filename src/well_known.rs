//! OAuth Protected Resource Metadata (RFC 9728).
//!
//! This is the only OAuth metadata *we* serve: it names Hydra as the
//! authorization server. Claude fetches this after a 401, then talks OAuth
//! directly to Hydra (DCR, PKCE, token) — we never proxy those endpoints.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::state::AppState;

pub async fn protected_resource(State(state): State<AppState>) -> Json<Value> {
    let resource = state.config.public_url.trim_end_matches('/');
    let issuer = state.config.hydra_issuer.trim_end_matches('/');
    Json(json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["openid", "offline"],
    }))
}
