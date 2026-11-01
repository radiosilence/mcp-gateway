//! Browser-facing auth routes:
//! - `/login`, `/logout`   — the dashboard session
//! - `/auth/callback`      — the issuer's return leg
//!
//! All session and flow state is server-side; cookies carry only opaque ids.

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

fn session_id(headers: &HeaderMap) -> Option<&str> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie::read_cookie(cookies, SESSION_COOKIE)
}

/// The signed-in session, if any. A store that cannot answer is an error, not
/// a signed-out user.
pub async fn current_session(state: &AppState, headers: &HeaderMap) -> AppResult<Option<Session>> {
    match session_id(headers) {
        Some(id) => state
            .store
            .get_session(id)
            .await
            .map_err(AppError::Unavailable),
        None => Ok(None),
    }
}

/// `cookies`, plus clearing the pre-`__Host-` names.
fn clearing(cookies: &[String]) -> Vec<String> {
    cookies
        .iter()
        .cloned()
        .chain(cookie::LEGACY_COOKIES.map(cookie::clear_cookie))
        .collect()
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
        .await?;
    Ok(redirect(
        &oidc::authorize_url(&state.config, &csrf, &challenge),
        &[cookie::set_cookie(OAUTH_COOKIE, &flow_id, OAUTH_FLOW_TTL)],
    ))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = session_id(&headers)
        && let Err(e) = state.store.delete_session(id).await
    {
        tracing::warn!(
            error = format!("{e:#}"),
            "deleting a session at logout failed"
        );
    }
    // Only this gateway's session; the issuer's serves other relying parties.
    redirect("/", &clearing(&[cookie::clear_cookie(SESSION_COOKIE)]))
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
    // Consumed on read, so a replayed callback finds nothing.
    let flow_id = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| cookie::read_cookie(c, OAUTH_COOKIE))
        .ok_or(AppError::Unauthorized)?;
    let flow = state
        .store
        .take_oauth_flow(flow_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if flow.csrf != q.state {
        return Err(AppError::BadRequest("state mismatch".into()));
    }

    // Who may sign in is the login provider's decision; a valid code is it.
    let identity = oidc::exchange_code(&state.config, &state.http, &q.code, &flow.verifier)
        .await
        .map_err(AppError::Upstream)?;

    let session_id = state
        .store
        .create_session(&identity.subject, &identity.login, SESSION_TTL)
        .await?;

    Ok(redirect(
        "/dashboard",
        &clearing(&[
            cookie::clear_cookie(OAUTH_COOKIE),
            cookie::set_cookie(SESSION_COOKIE, &session_id, SESSION_TTL),
        ]),
    ))
}
