//! Dashboard: manage the credentials for each registered MCP. An MCP declares
//! the fields it needs and the form is built from those. Mutations answer with
//! the section they changed, which Datastar morphs into the page.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};

mod options;
mod view;

use view::*;

use options::{fetch_choices, options_template, verify};

use crate::auth::extract::{FragmentSession, PageSession};
use crate::auth::routes::current_session;
use crate::config::{CredentialField, Mcp};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{CredentialSet, Session};

fn mcp<'a>(state: &'a AppState, id: &str) -> AppResult<&'a Mcp> {
    state.config.mcp(id).ok_or(AppError::NotFound)
}

/// One MCP's section, re-read from the store: the answer to every mutation.
async fn section(
    state: &AppState,
    session: &Session,
    mcp: &Mcp,
    notice: Notice,
) -> AppResult<Response> {
    let stored = state
        .store
        .get_credentials_with_meta(&session.sub, &mcp.id, mcp.primary_field())
        .await?;
    let mut view = McpView::new(&state.config, mcp, stored);
    view.notice = notice;
    Ok(render(McpSectionTemplate { m: view }))
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if current_session(&state, &headers).await?.is_some() {
        return Ok(Redirect::to("/dashboard").into_response());
    }
    Ok(Html(IndexTemplate.render().map_err(anyhow::Error::from)?).into_response())
}

/// Built from the gateway's own database only. Anything that has to ask a
/// backend is fetched by the browser once the page is up, so a slow backend
/// cannot hold the page.
pub async fn dashboard(
    State(state): State<AppState>,
    PageSession(session): PageSession,
) -> AppResult<Response> {
    let mut stored = state
        .store
        .credentials_for(&session.sub, &state.config.mcps)
        .await?;
    let mcps = state
        .config
        .mcps
        .iter()
        .map(|m| McpView::new(&state.config, m, stored.remove(&m.id)))
        .collect();
    let html = DashboardTemplate {
        login: session.login,
        mcps,
    }
    .render()
    .map_err(anyhow::Error::from)?;
    Ok(Html(html).into_response())
}

pub async fn set_credential(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
    Form(form): Form<CredentialForm>,
) -> AppResult<Response> {
    let mcp = mcp(&state, &mcp_id)?;
    let saved = state
        .store
        .update_credentials(&session.sub, &mcp_id, mcp.primary_field(), |stored| {
            let mut values = CredentialSet::new();
            for field in &mcp.fields {
                let submitted = form.get(&field.id).map(String::as_str);
                let stored_value = stored.and_then(|s| s.get(&field.id)).map(String::as_str);
                if let Some(value) = resolve_field(field, submitted, stored_value)? {
                    values.insert(field.id.clone(), value);
                }
            }
            Ok::<_, String>(values)
        })
        .await?;
    let notice = match saved {
        Err(complaint) => Notice::wrong(complaint),
        Ok((stored, values)) => match sync_upstream(&state, mcp, &values, stored.as_ref()).await {
            None => Notice::said("Saved"),
            // Stored either way: the proxy injects our value regardless.
            Some(complaint) => Notice::wrong(format!("Saved. {complaint}")),
        },
    };
    section(&state, &session, mcp, notice).await
}

/// Tell the backend what the user picked, for fields that declare a
/// `sync_mutation`, but only for values that changed. Returns what to tell the
/// user when a backend refused.
async fn sync_upstream(
    state: &AppState,
    mcp: &Mcp,
    values: &CredentialSet,
    stored: Option<&CredentialSet>,
) -> Option<String> {
    for field in &mcp.fields {
        let Some(mutation) = &field.sync_mutation else {
            continue;
        };
        let Some(value) = values.get(&field.id).filter(|v| !v.is_empty()) else {
            continue;
        };
        if stored.and_then(|s| s.get(&field.id)) == Some(value) {
            continue;
        }
        let variables = json!({ "value": value });
        let refusal =
            match crate::backend::graphql(state, mcp, values, mutation, Some(variables)).await {
                Err(e) => Some(e.to_string()),
                Ok(data) => refused(&data),
            };
        if let Some(reason) = refusal {
            tracing::debug!(reason, mcp = %mcp.id, field = %field.id, "upstream sync refused");
            return Some(format!(
                "{} is set here, but the backend wouldn't take it: {reason}",
                field.label
            ));
        }
    }
    None
}

/// Why a sync mutation didn't take, if it didn't. A backend that declines a
/// valid write answers `{ success: false, error }` rather than a GraphQL error.
fn refused(data: &Value) -> Option<String> {
    let payload = data.as_object()?.values().next()?;
    match payload.get("success").and_then(Value::as_bool) {
        Some(false) => Some(
            payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the backend declined it")
                .to_string(),
        ),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct FieldUpdate {
    value: String,
}

/// What to store for one field, given what the form sent and what is stored.
/// `Ok(None)` stores nothing; `Err` is the complaint to show the user.
///
/// Absent and empty differ: a field the form never carried (a setting saved on
/// its own) keeps its stored value.
fn resolve_field(
    field: &CredentialField,
    submitted: Option<&str>,
    stored: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(value) = submitted.map(str::trim) else {
        return Ok(stored.map(str::to_string));
    };

    if !value.is_empty() {
        // These become header values, so refuse them here where the user can
        // be told rather than dropping them at proxy time.
        if value.chars().any(char::is_control) {
            return Err(format!(
                "{} contains characters that can't be sent",
                field.label
            ));
        }
        return Ok(Some(value.to_string()));
    }

    // A secret is never rendered back, so blank means "keep it". A blank
    // non-secret was visibly cleared.
    if field.secret
        && let Some(prev) = stored.filter(|p| !p.is_empty())
    {
        return Ok(Some(prev.to_string()));
    }
    match (field.required, field.default.as_deref()) {
        (true, _) => Err(format!("{} cannot be empty", field.label)),
        (false, Some(default)) => Ok(Some(default.to_string())),
        (false, None) => Ok(None),
    }
}

/// Patch one non-secret setting, then sync it upstream. A secret can only be
/// set through the credential form.
pub async fn set_field(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
    Form(update): Form<FieldUpdate>,
) -> AppResult<Response> {
    let mcp = mcp(&state, &mcp_id)?;
    let field = mcp
        .fields
        .iter()
        .find(|f| f.id == field_id)
        .ok_or(AppError::NotFound)?;
    if field.secret {
        return Err(AppError::BadRequest("not settable on its own".into()));
    }
    // Datastar discards the body of a non-200 response, so anything the user
    // should read comes back as a 200 status line.
    let status = |message: String, bad: bool| {
        render(FieldStatusTemplate {
            id: format!("field-status-{mcp_id}-{field_id}"),
            message,
            bad,
        })
    };
    let value = update.value.trim();
    if value.chars().any(char::is_control) {
        return Ok(status(
            format!("{} contains characters that can't be sent", field.label),
            true,
        ));
    }

    let saved = state
        .store
        .update_credentials(&session.sub, &mcp_id, mcp.primary_field(), |stored| {
            let mut values = stored.cloned().ok_or("Connect the account first")?;
            match value.is_empty() {
                true => values.remove(&field_id),
                false => values.insert(field_id.clone(), value.to_string()),
            };
            Ok::<_, &str>(values)
        })
        .await?;
    let (stored, values) = match saved {
        Ok(saved) => saved,
        Err(complaint) => return Ok(status(complaint.into(), true)),
    };

    Ok(
        match sync_upstream(&state, mcp, &values, stored.as_ref()).await {
            Some(refused) => status(refused, true),
            None => status("Saved".into(), false),
        },
    )
}

/// A setting's choices, fetched from the backend with the user's credentials.
/// Failure answers with the backend's error text in the control's place, and
/// the rest of the page is unaffected.
pub async fn field_options(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let mcp = mcp(&state, &mcp_id)?;
    let query = mcp
        .fields
        .iter()
        .find(|f| f.id == field_id)
        .and_then(|f| f.options_query.as_deref())
        .ok_or(AppError::NotFound)?;
    let started = std::time::Instant::now();
    match fetch_choices(&state, &session.sub, mcp, &field_id, query).await {
        Ok(choices) => {
            tracing::debug!(
                mcp = %mcp_id, field = %field_id, ms = started.elapsed().as_millis(),
                "options loaded"
            );
            Ok(render(options_template(
                &choices.options,
                &choices.current,
                &mcp_id,
                &field_id,
            )))
        }
        Err(e) => {
            tracing::debug!(error = %e, mcp = %mcp_id, field = %field_id, "options lookup failed");
            Ok(render(FieldOptionsErrorTemplate {
                mcp_id,
                field_id,
                error: e.to_string(),
            }))
        }
    }
}

/// Whether the backend accepts the stored credentials, as the badge.
pub async fn mcp_status(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    let mcp = mcp(&state, &mcp_id)?;
    let stored = state
        .store
        .get_credentials_with_meta(&session.sub, &mcp.id, mcp.primary_field())
        .await?;
    let started = std::time::Instant::now();
    let verdict = verify(&state, mcp, stored.as_ref().map(|(values, _)| values)).await;
    tracing::debug!(
        mcp = %mcp_id, ms = started.elapsed().as_millis(), ?verdict,
        "credentials checked"
    );
    let view = McpView::new(&state.config, mcp, stored).checked(verdict);
    Ok(render(McpBadgeTemplate { m: view }))
}

pub async fn delete_credential(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    let mcp = mcp(&state, &mcp_id)?;
    state.store.delete_credential(&session.sub, &mcp_id).await?;
    section(&state, &session, mcp, Notice::said("Disconnected")).await
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::{Request, header};

    use super::*;

    fn field(id: &str, secret: bool, required: bool) -> CredentialField {
        serde_json::from_value(json!({
            "id": id, "label": id, "header": format!("X-{id}"),
            "secret": secret, "required": required,
        }))
        .unwrap()
    }

    fn form_request(body: &'static str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    }

    #[test]
    fn an_optional_field_left_blank_stores_nothing() {
        let optional = field("app_key", true, false);
        assert_eq!(resolve_field(&optional, Some(""), None), Ok(None));
    }

    #[test]
    fn a_field_the_form_never_carried_keeps_what_is_stored() {
        let setting = field("calendar", false, false);
        assert_eq!(
            resolve_field(&setting, None, Some("home")),
            Ok(Some("home".into()))
        );
        assert_eq!(resolve_field(&setting, None, None), Ok(None));
    }

    #[test]
    fn a_blank_secret_keeps_the_stored_one() {
        let password = field("password", true, true);
        assert_eq!(
            resolve_field(&password, Some(""), Some("hunter2")),
            Ok(Some("hunter2".into()))
        );
        assert!(resolve_field(&password, Some(""), None).is_err());
    }

    #[test]
    fn a_visibly_cleared_setting_is_cleared() {
        let setting = field("calendar", false, false);
        assert_eq!(resolve_field(&setting, Some(""), Some("home")), Ok(None));
    }

    #[test]
    fn a_cleared_optional_falls_back_to_its_default() {
        let mut url = field("url", false, false);
        url.default = Some("https://caldav.icloud.com".into());
        assert_eq!(
            resolve_field(&url, Some(""), Some("https://other.test")),
            Ok(Some("https://caldav.icloud.com".into()))
        );
    }

    #[test]
    fn a_value_that_could_not_be_sent_as_a_header_is_refused() {
        let setting = field("calendar", false, false);
        assert!(resolve_field(&setting, Some("we\nird"), None).is_err());
    }

    #[tokio::test]
    async fn a_single_field_update_deserializes_from_a_urlencoded_body() {
        for (body, expected) in [("value=home", "home"), ("value=", "")] {
            let Form(update) = Form::<FieldUpdate>::from_request(form_request(body), &())
                .await
                .expect("a field update must deserialize");
            assert_eq!(update.value, expected);
        }
    }

    /// A wrapper type around the map is rejected by the extractor.
    #[tokio::test]
    async fn the_credential_form_deserializes_from_a_urlencoded_body() {
        let Form(form) = Form::<CredentialForm>::from_request(
            form_request("username=jc%40blit.cc&password=pw&url="),
            &(),
        )
        .await
        .expect("a form body must deserialize");
        assert_eq!(form.get("username").map(String::as_str), Some("jc@blit.cc"));
        assert_eq!(form.get("password").map(String::as_str), Some("pw"));
        assert_eq!(form.get("url").map(String::as_str), Some(""));
    }

    #[test]
    fn a_declined_sync_is_read_out_of_the_payload() {
        let data =
            json!({"setDefaultCalendar": {"success": false, "error": "no scheduling inbox"}});
        assert_eq!(refused(&data).as_deref(), Some("no scheduling inbox"));

        let bare = json!({"setDefaultCalendar": {"success": false}});
        assert_eq!(refused(&bare).as_deref(), Some("the backend declined it"));
    }

    #[test]
    fn a_sync_that_took_says_nothing() {
        let data = json!({"setDefaultCalendar": {"success": true, "error": null}});
        assert_eq!(refused(&data), None);
        assert_eq!(refused(&json!({"setDefaultCalendar": {}})), None);
        assert_eq!(refused(&json!({})), None);
    }
}
