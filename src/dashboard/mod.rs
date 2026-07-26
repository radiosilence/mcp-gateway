//! Dashboard: log in with our OAuth (GitHub upstream), then manage the
//! credentials for each registered MCP. An MCP declares the fields it needs
//! (one for a bearer token, three for CalDAV), and the form is built from
//! those. Mutations answer with the section they changed, so the page never
//! reloads and there is no flash to carry across one.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};

mod options;
mod view;

use view::*;

use options::{fetch_choices, options_fragment, prefetch_options};

use crate::auth::extract::{FragmentSession, PageSession};
use crate::auth::routes::current_session;
use crate::config::Mcp;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{CredentialSet, Session};

/// Re-render one MCP's section, which is the answer to every mutation on it:
/// the badge, the timestamp and the settings all come from one place, so they
/// cannot disagree about what just happened.
async fn section(
    state: &AppState,
    session: &Session,
    mcp: &Mcp,
    notice: Notice,
) -> AppResult<Response> {
    // Only this MCP's settings, and only after the change — a new password
    // means a different set of choices.
    let prefetched = prefetch_options(state, &session.sub, Some(&mcp.id)).await;
    let mut view = McpView::build(state, session, mcp, &prefetched).await?;
    view.notice = notice;
    Ok(render(McpSectionTemplate { m: view }))
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
    PageSession(session): PageSession,
) -> AppResult<Response> {
    let prefetched = prefetch_options(&state, &session.sub, None).await;

    let mut mcps = Vec::new();
    for m in &state.config.mcps {
        mcps.push(McpView::build(&state, &session, m, &prefetched).await?);
    }

    let tpl = DashboardTemplate {
        login: session.login,
        mcps,
    };
    let html = tpl.render().map_err(|e| AppError::Internal(e.into()))?;
    Ok(Html(html).into_response())
}

pub async fn set_credential(
    State(state): State<AppState>,
    PageSession(session): PageSession,
    Path(mcp_id): Path<String>,
    Form(form): Form<CredentialForm>,
) -> AppResult<Response> {
    let Some(mcp) = state.config.mcp(&mcp_id) else {
        return Err(AppError::BadRequest("unknown mcp".into()));
    };

    let stored = state
        .store
        .get_credentials(&session.sub, &mcp_id, mcp.primary_field())
        .await
        .map_err(AppError::Internal)?;

    let mut values = CredentialSet::new();
    for field in &mcp.fields {
        let submitted = form.get(&field.id).map(String::as_str);
        let stored_value = stored
            .as_ref()
            .and_then(|s| s.get(&field.id))
            .map(String::as_str);
        match resolve_field(field, submitted, stored_value) {
            Ok(Some(value)) => {
                values.insert(field.id.clone(), value);
            }
            Ok(None) => {}
            Err(complaint) => {
                return section(&state, &session, mcp, Notice::wrong(complaint)).await;
            }
        }
    }

    state
        .store
        .set_credentials(&session.sub, &mcp_id, &values)
        .await
        .map_err(AppError::Internal)?;

    match sync_upstream(&state, mcp, &values, stored.as_ref()).await {
        None => section(&state, &session, mcp, Notice::said("Saved")).await,
        // Stored either way — the proxy injects our value on every request, so
        // the gateway behaves as asked whatever the backend thinks of it.
        Some(complaint) => {
            section(
                &state,
                &session,
                mcp,
                Notice::wrong(format!("Saved. {complaint}")),
            )
            .await
        }
    }
}

/// Tell the backend what the user picked, for fields that declare a
/// `sync_mutation` — so a calendar chosen here is also the one their phone
/// uses. Returns what to tell the user when a backend wouldn't take it.
///
/// Only for values that actually changed: saving the form again shouldn't
/// re-issue a write to someone's account.
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
        // Passed as a variable, never interpolated: a calendar named `"` would
        // otherwise rewrite the mutation.
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

/// Why a sync mutation didn't take, if it didn't.
///
/// A backend that declines an otherwise valid write answers with a payload
/// saying so rather than a GraphQL error — `caldav-cli` returns
/// `{ success: false, error }` for a server with nowhere to keep the property.
/// So a mutation used for syncing has to surface those two fields.
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

/// What to store for one field, given what the form sent and what we already
/// hold. `Ok(None)` stores nothing; `Err` is the complaint to show the user.
///
/// The distinction that matters is absent versus empty. A field the form never
/// carried — the calendar picker saves itself, so it isn't in the credential
/// form — must keep its stored value; treating that as "cleared" would wipe it
/// on every unrelated save.
fn resolve_field(
    field: &crate::config::CredentialField,
    submitted: Option<&str>,
    stored: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(value) = submitted.map(str::trim) else {
        return Ok(stored.map(str::to_string));
    };

    if !value.is_empty() {
        // These become header values downstream. Reject control characters
        // here, where we can tell the user, rather than dropping them silently
        // at proxy time.
        if value.chars().any(|c| c.is_control()) {
            return Err(format!(
                "{} contains characters that can't be sent",
                field.label
            ));
        }
        return Ok(Some(value.to_string()));
    }

    // A blank secret means "leave it alone" — it is never rendered back, so the
    // user couldn't have retyped it to change one of the other fields. A blank
    // non-secret was visibly cleared on purpose.
    if field.secret
        && let Some(prev) = stored.filter(|p| !p.is_empty())
    {
        return Ok(Some(prev.to_string()));
    }
    // An optional field left blank falls back to the configured default, if
    // there is one, and is otherwise simply not stored.
    match (field.required, field.default.as_deref()) {
        (true, _) => Err(format!("{} cannot be empty", field.label)),
        (false, Some(default)) => Ok(Some(default.to_string())),
        (false, None) => Ok(None),
    }
}

/// Patch one field, then sync it upstream.
///
/// A setting is not a credential: changing which calendar new events go to
/// shouldn't demand the rest of the form back, least of all a password the
/// page never shows. Restricted to non-secret fields for that reason — a
/// secret can only be set where the user typed it.
pub async fn set_field(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
    Form(update): Form<FieldUpdate>,
) -> AppResult<Response> {
    let Some(mcp) = state.config.mcp(&mcp_id) else {
        return Err(AppError::BadRequest("unknown mcp".into()));
    };
    let Some(field) = mcp.fields.iter().find(|f| f.id == field_id) else {
        return Err(AppError::BadRequest("unknown field".into()));
    };
    if field.secret {
        return Err(AppError::BadRequest("not settable on its own".into()));
    }
    let value = update.value.trim();
    if value.chars().any(|c| c.is_control()) {
        return Err(AppError::BadRequest(
            "value contains control characters".into(),
        ));
    }

    // Merge into what's stored: everything else stays as it was.
    let Some(stored) = state
        .store
        .get_credentials(&session.sub, &mcp_id, mcp.primary_field())
        .await
        .map_err(AppError::Internal)?
    else {
        return Err(AppError::BadRequest("no credentials stored".into()));
    };
    let mut values = stored.clone();
    match value.is_empty() {
        true => {
            values.remove(&field_id);
        }
        false => {
            values.insert(field_id.clone(), value.to_string());
        }
    }
    state
        .store
        .set_credentials(&session.sub, &mcp_id, &values)
        .await
        .map_err(AppError::Internal)?;

    // A backend that refused the change is worth saying out loud; our stored
    // value is authoritative either way, so the save still stands.
    let refused = sync_upstream(&state, mcp, &values, Some(&stored)).await;
    Ok(status_fragment(
        refused.clone().unwrap_or_else(|| "Saved".into()),
        refused.is_some(),
    ))
}

/// Suggestions for one field, fetched from the backend with the user's own
/// credentials — e.g. the calendars this account can write to.
///
/// Its own endpoint rather than part of the dashboard render: a backend that is
/// slow or down must not take the page with it, and the form still works as
/// free text if this fails.
pub async fn field_options(
    State(state): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path((mcp_id, field_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(mcp) = state.config.mcp(&mcp_id) else {
        return Err(AppError::BadRequest("unknown mcp".into()));
    };
    let Some(query) = mcp
        .fields
        .iter()
        .find(|f| f.id == field_id)
        .and_then(|f| f.options_query.as_deref())
    else {
        return Err(AppError::BadRequest("field has no options".into()));
    };
    // Reaching here means the page's budget was missed, so this is where the
    // slow half gets measured. The fast half reports from the prefetch.
    let started = std::time::Instant::now();
    match fetch_choices(&state, &session.sub, mcp, &field_id, query).await {
        Ok(choices) => {
            tracing::info!(
                mcp = %mcp_id, field = %field_id, ms = started.elapsed().as_millis(),
                "options loaded after the render budget"
            );
            Ok(options_fragment(
                &choices.options,
                &choices.current,
                &mcp_id,
                &field_id,
            ))
        }
        Err(e) => {
            // Wrong password, backend down, a calendar server having a bad day:
            // the user's own error text is more use than a 500. Returning the
            // status line rather than options leaves the placeholder in place.
            tracing::debug!(error = %e, mcp = %mcp_id, field = %field_id, "options lookup failed");
            Ok(render(FieldOptionsErrorTemplate {
                error: e.to_string(),
            }))
        }
    }
}

fn status_fragment(message: String, bad: bool) -> Response {
    render(FieldStatusTemplate { message, bad })
}

/// A rendering failure here is a bug in a template, not something the user can
/// act on, so it surfaces as text rather than taking the whole page down.
pub async fn delete_credential(
    State(state): State<AppState>,
    PageSession(session): PageSession,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    state
        .store
        .delete_credential(&session.sub, &mcp_id)
        .await
        .map_err(AppError::Internal)?;
    let mcp = mcp_or_404(&state, &mcp_id)?;
    section(&state, &session, mcp, Notice::said("Disconnected")).await
}

#[cfg(test)]
mod tests {

    /// Connecting an MCP whose fields are all optional stores no values, and
    /// must still count as connected — TfL works anonymously, and a key there
    /// only raises a rate limit. The empty set is the record that you asked
    /// for it.
    #[test]
    fn an_mcp_can_be_connected_while_holding_nothing() {
        let optional = field("app_key", true, false);
        assert!(matches!(resolve_field(&optional, Some(""), None), Ok(None)));

        let values = CredentialSet::new();
        assert!(values.is_empty(), "nothing to store, and that is the point");
    }
    use super::*;

    fn field(id: &str, secret: bool, required: bool) -> crate::config::CredentialField {
        serde_json::from_value(json!({
            "id": id, "label": id, "header": format!("X-{id}"),
            "secret": secret, "required": required,
        }))
        .unwrap()
    }

    #[test]
    fn a_field_the_form_never_carried_keeps_what_is_stored() {
        // The calendar picker saves itself, so it isn't in the credential form.
        // Reading absent as "cleared" would wipe it on every unrelated save.
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
        // Nothing stored to keep, and it's required: say so rather than store "".
        assert!(resolve_field(&password, Some(""), None).is_err());
    }

    #[test]
    fn a_visibly_cleared_setting_is_cleared() {
        // Non-secret and present-but-empty: the user emptied a box they could see.
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

    /// The picker's save, through the same extractor.
    #[tokio::test]
    async fn a_single_field_update_deserializes_from_a_urlencoded_body() {
        use axum::body::Body;
        use axum::extract::FromRequest;
        use axum::http::{Request, header};

        for (body, expected) in [("value=home", "home"), ("value=", "")] {
            let request = Request::builder()
                .method("POST")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap();
            let Form(update) = Form::<FieldUpdate>::from_request(request, &())
                .await
                .expect("a field update must deserialize");
            // An empty value is how "use the account's own default" arrives.
            assert_eq!(update.value, expected);
        }
    }

    /// The dashboard's own form, through the extractor that rejected it in
    /// production: a wrapper type here is a 422 on every multi-field save.
    #[tokio::test]
    async fn the_credential_form_deserializes_from_a_urlencoded_body() {
        use axum::body::Body;
        use axum::extract::FromRequest;
        use axum::http::{Request, header};

        let request = Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("username=jc%40blit.cc&password=pw&url="))
            .unwrap();

        let Form(form) = Form::<CredentialForm>::from_request(request, &())
            .await
            .expect("a form body must deserialize");
        assert_eq!(form.get("username").map(String::as_str), Some("jc@blit.cc"));
        assert_eq!(form.get("password").map(String::as_str), Some("pw"));
        // A cleared optional field arrives as an empty value, not as absent.
        assert_eq!(form.get("url").map(String::as_str), Some(""));
    }

    #[test]
    fn a_declined_sync_is_read_out_of_the_payload() {
        // Not a GraphQL error — the query succeeded, the write didn't.
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
        // A mutation that reports neither field is taken at its word.
        assert_eq!(refused(&json!({"setDefaultCalendar": {}})), None);
        assert_eq!(refused(&json!({})), None);
    }
}
