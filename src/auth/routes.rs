//! Browser-facing auth routes:
//! - `/login`, `/logout`         — dashboard session (our own)
//! - `/auth/login`               — Hydra login provider callback
//! - `/auth/consent`             — Hydra consent provider callback
//! - `/auth/github/callback`     — GitHub OAuth return (serves both flows)
//!
//! All session/flow state is server-side; cookies carry only opaque ids.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;

use super::cookie::{self, OAUTH_COOKIE, SESSION_COOKIE};
use super::github;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::Session;

const OAUTH_FLOW_TTL: i64 = 600; // 10 min to complete the GitHub round-trip
const SESSION_TTL: i64 = 7 * 24 * 3600; // 1 week

fn random_token() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
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

/// Create an OAuth flow row and return the cookie carrying its id.
async fn begin_github(
    state: &AppState,
    login_challenge: Option<&str>,
) -> AppResult<(String, String)> {
    let csrf = random_token();
    let flow_id = state
        .store
        .create_oauth_flow(&csrf, login_challenge, OAUTH_FLOW_TTL)
        .await
        .map_err(AppError::Internal)?;
    let url = github::authorize_url(&state.config, &csrf);
    let cookie = cookie::set_cookie(OAUTH_COOKIE, &flow_id, OAUTH_FLOW_TTL);
    Ok((url, cookie))
}

// ---- Dashboard session ----

/// Start a dashboard login: bounce through GitHub (no Hydra challenge).
pub async fn login(State(state): State<AppState>) -> AppResult<Response> {
    let (url, cookie) = begin_github(&state, None).await?;
    Ok(redirect(&url, &[cookie]))
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(id) = cookie::read_cookie(cookies, SESSION_COOKIE)
    {
        let _ = state.store.delete_session(id).await;
    }
    redirect("/", &[cookie::clear_cookie(SESSION_COOKIE)])
}

// ---- Hydra login provider ----

#[derive(Deserialize)]
pub struct LoginQuery {
    login_challenge: String,
}

/// Hydra redirects here to have us authenticate the user. We bounce to GitHub,
/// carrying the login_challenge in the server-side flow row.
pub async fn hydra_login(
    State(state): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> AppResult<Response> {
    let (url, cookie) = begin_github(&state, Some(&q.login_challenge)).await?;
    Ok(redirect(&url, &[cookie]))
}

// ---- Hydra consent provider ----

#[derive(Deserialize)]
pub struct ConsentQuery {
    consent_challenge: String,
}

/// Auto-grant consent (single-tenant tool, no consent screen).
pub async fn hydra_consent(
    State(state): State<AppState>,
    Query(q): Query<ConsentQuery>,
) -> AppResult<Response> {
    let req = state
        .hydra
        .get_consent(&q.consent_challenge)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let redirect_to = state
        .hydra
        .accept_consent(&q.consent_challenge, &req)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    Ok(redirect(&redirect_to, &[]))
}

// ---- GitHub callback (both flows) ----

#[derive(Deserialize)]
pub struct GithubCallback {
    code: String,
    state: String,
}

pub async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GithubCallback>,
) -> AppResult<Response> {
    // Recover and consume the one-shot OAuth flow row.
    let cookies = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let flow_id = cookie::read_cookie(cookies, OAUTH_COOKIE).ok_or(AppError::Unauthorized)?;
    let flow = state
        .store
        .take_oauth_flow(flow_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or(AppError::Unauthorized)?;

    if flow.csrf != q.state {
        return Err(AppError::BadRequest("state mismatch".into()));
    }

    let (sub, login) = github::exchange_code(&state.config, &state.http, &q.code)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;

    // Identity gate: only allowlisted GitHub logins may authenticate. This is
    // the control that makes public DCR safe — a registered client is useless
    // without a token, and tokens only issue to allowlisted users.
    if !state.config.github_login_allowed(&login) {
        tracing::warn!(%login, "rejected login: not in GH_ALLOWED");
        return Err(AppError::Unauthorized);
    }

    match flow.login_challenge {
        // Servicing a Hydra login: tell Hydra who this is.
        Some(lc) => {
            let redirect_to = state
                .hydra
                .accept_login(&lc, &sub)
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?;
            Ok(redirect(
                &redirect_to,
                &[cookie::clear_cookie(OAUTH_COOKIE)],
            ))
        }
        // Dashboard login: create a server-side session.
        None => {
            let session_id = state
                .store
                .create_session(&sub, &login, SESSION_TTL)
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
    }
}
