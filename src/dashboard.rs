//! Minimal dashboard: log in with our OAuth (GitHub upstream), then set /
//! update / delete / test your Fastmail token. Post-Redirect-Get with a `flash`
//! query param for feedback.

use askama::Template;
use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::routes::current_session;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    login: String,
    has_token: bool,
    updated_at: String,
    flash: String,
    mcp_url: String,
}

#[derive(Deserialize)]
pub struct FlashQuery {
    #[serde(default)]
    flash: String,
}

#[derive(Deserialize)]
pub struct TokenForm {
    token: String,
}

fn flash_redirect(msg: &str) -> Response {
    let enc: String = url::form_urlencoded::byte_serialize(msg.as_bytes()).collect();
    Redirect::to(&format!("/dashboard?flash={enc}")).into_response()
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if current_session(&state, &headers).await.is_some() {
        return Redirect::to("/dashboard").into_response();
    }
    match IndexTemplate.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => AppError::Internal(e.into()).into_response(),
    }
}

pub async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FlashQuery>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    let meta = state
        .store
        .token_meta(&session.sub)
        .await
        .map_err(AppError::Internal)?;
    let (has_token, updated_at) = match meta {
        Some(m) => (true, m.updated_at.to_string()),
        None => (false, String::new()),
    };
    let tpl = DashboardTemplate {
        login: session.login,
        has_token,
        updated_at,
        flash: q.flash,
        mcp_url: format!("{}/mcp", state.config.public_url.trim_end_matches('/')),
    };
    let html = tpl.render().map_err(|e| AppError::Internal(e.into()))?;
    Ok(Html(html).into_response())
}

pub async fn set_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    let token = f.token.trim();
    if token.is_empty() {
        return Ok(flash_redirect("Token cannot be empty"));
    }
    state
        .store
        .set_token(&session.sub, token)
        .await
        .map_err(AppError::Internal)?;
    Ok(flash_redirect("Token saved"))
}

pub async fn delete_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    state
        .store
        .delete_token(&session.sub)
        .await
        .map_err(AppError::Internal)?;
    Ok(flash_redirect("Token deleted"))
}

/// Test the stored token by running the JMAP session handshake with it.
pub async fn test_token(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    let token = match state
        .store
        .get_token(&session.sub)
        .await
        .map_err(AppError::Internal)?
    {
        Some(t) => t,
        None => return Ok(flash_redirect("No token set")),
    };
    let mut client = fastmail_cli::jmap::JmapClient::new(token);
    match client.authenticate().await {
        Ok(_) => Ok(flash_redirect("Connection OK — token is valid")),
        Err(e) => Ok(flash_redirect(&format!("Connection failed: {e}"))),
    }
}
