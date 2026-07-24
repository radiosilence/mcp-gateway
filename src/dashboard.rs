//! Dashboard: log in with our OAuth (GitHub upstream), then manage the
//! credentials for each registered MCP. An MCP declares the fields it needs
//! (one for a bearer token, three for CalDAV), and the form is built from
//! those. Post-Redirect-Get with a `flash` query param.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth::routes::current_session;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::CredentialSet;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

/// One input in an MCP's credential form.
struct FieldView {
    id: String,
    label: String,
    /// `password` or `text` — secrets are never rendered back into the page.
    input_type: String,
    placeholder: String,
    /// Prefilled value: a configured default, or the stored value when the
    /// field is not a secret (so a server URL can be edited, not retyped).
    value: String,
    required: bool,
}

struct McpView {
    id: String,
    name: String,
    has_credential: bool,
    updated_at: String,
    connector_url: String,
    claude_code_cmd: String,
    key_help_url: String,
    fields: Vec<FieldView>,
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

/// The credential form posts one input per configured field, so the shape
/// isn't known at compile time.
#[derive(Deserialize)]
pub struct CredentialForm(HashMap<String, String>);

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

        // Non-secret values are read back so they can be edited in place;
        // secrets are never decrypted for rendering.
        let stored = if has_credential && m.fields.iter().any(|f| !f.secret) {
            state
                .store
                .get_credentials(&session.sub, &m.id, m.primary_field())
                .await
                .map_err(AppError::Internal)?
        } else {
            None
        };

        let fields = m
            .fields
            .iter()
            .map(|f| FieldView {
                value: match f.secret {
                    true => String::new(),
                    false => stored
                        .as_ref()
                        .and_then(|s| s.get(&f.id))
                        .cloned()
                        .or_else(|| f.default.clone())
                        .unwrap_or_default(),
                },
                input_type: if f.secret { "password" } else { "text" }.to_string(),
                placeholder: f
                    .hint
                    .clone()
                    .or_else(|| f.default.clone())
                    .unwrap_or_else(|| f.label.to_lowercase()),
                id: f.id.clone(),
                label: f.label.clone(),
                required: f.required,
            })
            .collect();

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
            fields,
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
    Form(form): Form<CredentialForm>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Ok(Redirect::to("/login").into_response());
    };
    let Some(mcp) = state.config.mcp(&mcp_id) else {
        return Err(AppError::BadRequest("unknown mcp".into()));
    };

    let mut values = CredentialSet::new();
    for field in &mcp.fields {
        let raw = form.0.get(&field.id).map(String::as_str).unwrap_or("");
        let value = raw.trim();
        if value.is_empty() {
            // An optional field left blank falls back to the configured
            // default, if there is one, and is otherwise simply not stored.
            match (field.required, field.default.as_deref()) {
                (true, _) => {
                    return Ok(flash_redirect(&format!("{} cannot be empty", field.label)));
                }
                (false, Some(default)) => {
                    values.insert(field.id.clone(), default.to_string());
                }
                (false, None) => {}
            }
            continue;
        }
        // These become header values downstream. Reject control characters
        // here, where we can tell the user, rather than dropping them silently
        // at proxy time.
        if value.chars().any(|c| c.is_control()) {
            return Ok(flash_redirect(&format!(
                "{} contains characters that can't be sent",
                field.label
            )));
        }
        values.insert(field.id.clone(), value.to_string());
    }

    state
        .store
        .set_credentials(&session.sub, &mcp_id, &values)
        .await
        .map_err(AppError::Internal)?;
    Ok(flash_redirect("Credentials saved"))
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
    Ok(flash_redirect("Credentials deleted"))
}
