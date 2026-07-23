//! Browser-facing auth routes:
//! - `/login`, `/logout`         — dashboard session (our own)
//! - `/auth/login`               — Hydra login provider callback
//! - `/auth/consent`             — Hydra consent provider callback
//! - `/auth/github/callback`     — GitHub OAuth return (serves both flows)

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;

use super::cookie::{self, OAUTH_COOKIE, OAuthStateClaims, SESSION_COOKIE, SessionClaims};
use super::github;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn random_token() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Read and verify our dashboard session cookie.
pub fn session_claims(headers: &HeaderMap, secret: &str) -> Option<SessionClaims> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let raw = cookie::read_cookie(cookies, SESSION_COOKIE)?;
    cookie::verify::<SessionClaims>(secret, raw).ok()
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

// ---- Dashboard session ----

/// Start a dashboard login: bounce through GitHub (no Hydra challenge).
pub async fn login(State(state): State<AppState>) -> AppResult<Response> {
    let csrf = random_token();
    let claims = OAuthStateClaims {
        csrf: csrf.clone(),
        login_challenge: None,
        exp: cookie::short_exp(600),
    };
    let cookie = cookie::sign(&state.config.session_secret, &claims).map_err(AppError::Internal)?;
    let url = github::authorize_url(&state.config, &csrf);
    Ok(redirect(
        &url,
        &[cookie::set_cookie(OAUTH_COOKIE, &cookie, 600)],
    ))
}

pub async fn logout() -> Response {
    redirect("/", &[cookie::clear_cookie(SESSION_COOKIE)])
}

// ---- Hydra login provider ----

#[derive(Deserialize)]
pub struct LoginQuery {
    login_challenge: String,
}

/// Hydra redirects here to have us authenticate the user. We bounce to GitHub,
/// carrying the login_challenge in a signed cookie.
pub async fn hydra_login(
    State(state): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> AppResult<Response> {
    let csrf = random_token();
    let claims = OAuthStateClaims {
        csrf: csrf.clone(),
        login_challenge: Some(q.login_challenge),
        exp: cookie::short_exp(600),
    };
    let cookie = cookie::sign(&state.config.session_secret, &claims).map_err(AppError::Internal)?;
    let url = github::authorize_url(&state.config, &csrf);
    Ok(redirect(
        &url,
        &[cookie::set_cookie(OAUTH_COOKIE, &cookie, 600)],
    ))
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
    // Recover and validate the signed state cookie.
    let cookies = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let raw = cookie::read_cookie(cookies, OAUTH_COOKIE).ok_or(AppError::Unauthorized)?;
    let st: OAuthStateClaims =
        cookie::verify(&state.config.session_secret, raw).map_err(|_| AppError::Unauthorized)?;

    if st.csrf != q.state {
        return Err(AppError::BadRequest("state mismatch".into()));
    }

    let (sub, login) = github::exchange_code(&state.config, &state.http, &q.code)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;

    match st.login_challenge {
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
        // Dashboard login: set our own session cookie.
        None => {
            let session = SessionClaims {
                sub,
                login,
                exp: cookie::session_exp(24 * 7),
            };
            let cookie =
                cookie::sign(&state.config.session_secret, &session).map_err(AppError::Internal)?;
            Ok(redirect(
                "/dashboard",
                &[
                    cookie::clear_cookie(OAUTH_COOKIE),
                    cookie::set_cookie(SESSION_COOKIE, &cookie, 24 * 7 * 3600),
                ],
            ))
        }
    }
}
