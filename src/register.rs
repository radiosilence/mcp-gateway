//! Dynamic Client Registration proxy (RFC 7591).
//!
//! Hydra advertises this as its `registration_endpoint` (via
//! `webfinger.oidc_discovery.client_registration_url`). We create the client
//! through Hydra's admin API and return a clean response — Hydra's own DCR
//! output includes empty `client_uri`/`logo_uri`/`tos_uri` fields that Claude
//! rejects, so we build the response ourselves and omit empties.
//!
//! Public + unauthenticated by design (that's DCR). The real control against
//! abuse is the login allowlist gating token issuance — see README.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> AppResult<Response> {
    let take = |key: &str, default: Value| {
        req.get(key)
            .cloned()
            .filter(|v| !v.is_null())
            .unwrap_or(default)
    };

    // Register the client allowing every scope an MCP client might request:
    // Claude Desktop asks for `offline`, Claude Code for `offline_access` (both
    // grant a refresh token in Hydra). Union the requested scope with the
    // standard set so the later authorization request is always a subset —
    // otherwise Hydra rejects it with invalid_scope. We auto-grant at consent
    // anyway (single-tenant), so widening the allowed set is safe.
    let requested_scope = take("scope", json!(""));
    let mut scopes: Vec<&str> = requested_scope
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    for s in ["openid", "offline", "offline_access"] {
        if !scopes.contains(&s) {
            scopes.push(s);
        }
    }
    let scope = scopes.join(" ");

    // Translate the DCR request into a Hydra admin client-create body.
    let body = json!({
        "client_name": take("client_name", json!("MCP Client")),
        "redirect_uris": take("redirect_uris", json!([])),
        "grant_types": take("grant_types", json!(["authorization_code", "refresh_token"])),
        "response_types": take("response_types", json!(["code"])),
        "scope": scope,
        "token_endpoint_auth_method": take("token_endpoint_auth_method", json!("none")),
    });

    let created = state
        .hydra
        .create_client(&body)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;

    // Log every registration so DCR abuse is visible in the logs (public DCR is
    // required for Claude; token issuance is still gated by the login allowlist).
    tracing::info!(
        client_id = created
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?"),
        client_name = created
            .get("client_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?"),
        "DCR: registered oauth client"
    );

    // Build a clean RFC 7591 response — only non-empty fields.
    let mut out = Map::new();
    for key in [
        "client_id",
        "client_secret",
        "client_name",
        "redirect_uris",
        "grant_types",
        "response_types",
        "scope",
        "token_endpoint_auth_method",
    ] {
        put(&mut out, key, created.get(key));
    }
    out.insert("client_secret_expires_at".into(), json!(0));

    Ok((StatusCode::CREATED, Json(Value::Object(out))).into_response())
}

/// Insert `key` only if the value is present, non-null, and not an empty string.
fn put(out: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(v) = value {
        let empty_str = v.as_str().map(str::is_empty).unwrap_or(false);
        if !v.is_null() && !empty_str {
            out.insert(key.to_string(), v.clone());
        }
    }
}
