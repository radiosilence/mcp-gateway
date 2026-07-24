//! Dashboard: log in with our OAuth (GitHub upstream), then manage a credential
//! per registered MCP. Post-Redirect-Get with a `flash` query param.

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::routes::current_session;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

struct McpView {
    id: String,
    name: String,
    has_credential: bool,
    updated_at: String,
    connector_url: String,
    claude_code_cmd: String,
    key_help_url: String,
    key_hint: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    login: String,
    flash: String,
    mcps: Vec<McpView>,
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
    let base = state.config.public_url.trim_end_matches('/');

    let mut mcps = Vec::new();
    for m in &state.config.mcps {
        let meta = state
            .store
            .credential_meta(&session.sub, &m.id)
            .await
            .map_err(AppError::Internal)?;
        let (has_credential, updated_at) = match meta {
            Some(meta) => (true, meta.updated_at.to_string()),
            None => (false, String::new()),
        };
        let connector_url = format!("{base}/{}", m.id);
        mcps.push(McpView {
            claude_code_cmd: format!(
                "claude mcp add --transport http --scope user {} {}",
                m.id, connector_url
            ),
            id: m.id.clone(),
            name: m.name.clone(),
            has_credential,
            updated_at,
            connector_url,
            key_help_url: m.key_help_url.clone().unwrap_or_default(),
            key_hint: m.key_hint.clone().unwrap_or_else(|| "paste key…".into()),
        });
    }

    let tpl = DashboardTemplate {
        login: session.login,
        flash: q.flash,
        mcps,
    };
    let html = tpl.render().map_err(|e| AppError::Internal(e.into()))?;
    Ok(Html(html).into_response())
}

pub async fn set_credential(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(mcp_id): Path<String>,
    Form(f): Form<TokenForm>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    if state.config.mcp(&mcp_id).is_none() {
        return Err(AppError::BadRequest("unknown mcp".into()));
    }
    let token = f.token.trim();
    if token.is_empty() {
        return Ok(flash_redirect("Key cannot be empty"));
    }
    state
        .store
        .set_credential(&session.sub, &mcp_id, token)
        .await
        .map_err(AppError::Internal)?;
    Ok(flash_redirect("Key saved"))
}

pub async fn delete_credential(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    state
        .store
        .delete_credential(&session.sub, &mcp_id)
        .await
        .map_err(AppError::Internal)?;
    Ok(flash_redirect("Key deleted"))
}
