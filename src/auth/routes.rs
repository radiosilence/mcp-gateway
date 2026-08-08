//! Browser-facing auth routes:
//! - `/login`, `/logout`   — the dashboard session
//! - `/auth/callback`      — the issuer's return leg
//!
//! All session and flow state is server-side; cookies carry only opaque ids.
//!
//! This used to be two flows through one callback: the dashboard's own GitHub
//! OAuth, and the login/consent provider Hydra delegated to. The provider moved
//! out to its own service, so what is left is an ordinary relying party — it
//! sends the browser to the issuer and reads back an identity, and it no longer
//! knows what GitHub is or who is allowed in.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use serde::Deserialize;

use super::cookie::{self, OAUTH_COOKIE, SESSION_COOKIE};
use super::oidc;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::Session;

const OAUTH_FLOW_TTL: i64 = 600; // 10 min to complete the round-trip
const SESSION_TTL: i64 = 7 * 24 * 3600; // 1 week

fn random_token() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Resolve the current dashboard session (opaque cookie → DB lookup).
pub async fn current_session(state: &AppState, headers: &HeaderMap) -> Option<Session> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let id = cookie::read_cookie(cookies, SESSION_COOKIE)?;
    state.store.get_session(id).await.ok().flatten()
}

fn redirect(location: &str, cookies: &[String]) -> Response {
    let mut resp = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .expect("valid redirect");
    for c in cookies {
        if let Ok(v) = HeaderValue::from_str(c) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

/// Start a dashboard login: send the browser to the issuer.
pub async fn login(State(state): State<AppState>) -> AppResult<Response> {
    let csrf = random_token();
    let (verifier, challenge) = oidc::pkce();
    let flow_id = state
        .store
        .create_oauth_flow(&csrf, &verifier, OAUTH_FLOW_TTL)
        .await
        .map_err(AppError::Internal)?;
    Ok(redirect(
        &oidc::authorize_url(&state.config, &csrf, &challenge),
        &[cookie::set_cookie(OAUTH_COOKIE, &flow_id, OAUTH_FLOW_TTL)],
    ))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(id) = cookie::read_cookie(cookies, SESSION_COOKIE)
    {
        let _ = state.store.delete_session(id).await;
    }
    // Only this gateway's session. The issuer's is its own, and ending it for
    // every other service because somebody left this dashboard would be a
    // surprise rather than a courtesy.
    redirect("/", &[cookie::clear_cookie(SESSION_COOKIE)])
}

#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> AppResult<Response> {
    // Recover and consume the one-shot flow row: a replayed callback finds
    // nothing rather than a second usable flow.
    let flow_id = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie::read_cookie(c, OAUTH_COOKIE))
        .ok_or(AppError::Unauthorized)?;
    let flow = state
        .store
        .take_oauth_flow(flow_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;

    if flow.csrf != q.state {
        return Err(AppError::BadRequest("state mismatch".into()));
    }

    // No allowlist check here any more. A token issues only to somebody the
    // provider admitted, so reaching this point with a valid code *is* the
    // check — and a second copy of the list here is a second thing to keep in
    // step and a second way to lock somebody out.
    let identity = oidc::exchange_code(&state.config, &state.http, &q.code, &flow.verifier)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;

    let session_id = state
        .store
        .create_session(&identity.subject, &identity.login, SESSION_TTL)
        .await
        .map_err(AppError::Internal)?;

    Ok(redirect(
        "/dashboard",
        &[
            cookie::clear_cookie(OAUTH_COOKIE),
            cookie::set_cookie(SESSION_COOKIE, &session_id, SESSION_TTL),
        ],
    ))
}
