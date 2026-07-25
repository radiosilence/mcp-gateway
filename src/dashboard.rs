//! Dashboard: log in with our OAuth (GitHub upstream), then manage the
//! credentials for each registered MCP. An MCP declares the fields it needs
//! (one for a bearer token, three for CalDAV), and the form is built from
//! those. Post-Redirect-Get with a `flash` query param.

use std::collections::HashMap;

use askama::Template;
use axum::Json;
use axum::extract::{Form, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::routes::current_session;
use crate::config::Mcp;
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
    secret: bool,
    placeholder: String,
    /// Prefilled with the stored value when the field is not a secret (so a
    /// server URL can be edited, not retyped). Empty for secrets.
    value: String,
    required: bool,
    /// This field's choices come from the backend, so it is offered only after
    /// the account is connected — before that there is nobody to ask.
    from_backend: bool,
    /// Endpoint serving those choices. Empty until credentials exist.
    options_url: String,
}

struct McpView {
    id: String,
    name: String,
    has_credential: bool,
    /// Takes no credentials — connected on login, with no form to fill in and
    /// nothing to disconnect.
    public: bool,
    /// Whether any field is a setting rather than a credential — shown outside
    /// the credential form, since that is not what it is.
    has_settings: bool,
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
///
/// A plain map, not a newtype around one: `serde_urlencoded` presents a form
/// body as a map and has no `deserialize_newtype_struct` to unwrap it, so a
/// wrapper is rejected at the extractor with "invalid type: map".
pub type CredentialForm = HashMap<String, String>;

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
        // A public MCP is connected the moment the user has logged in — there
        // is nothing to store, so nothing to wait for.
        let (has_credential, updated_at) = match (m.is_public(), meta) {
            (true, _) => (true, String::new()),
            (false, Some(meta)) => (true, meta.updated_at.to_string()),
            (false, None) => (false, String::new()),
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

        let fields: Vec<FieldView> = m
            .fields
            .iter()
            .map(|f| FieldView {
                value: match f.secret {
                    true => String::new(),
                    false => stored
                        .as_ref()
                        .and_then(|s| s.get(&f.id))
                        .cloned()
                        .unwrap_or_default(),
                },
                input_type: if f.secret { "password" } else { "text" }.to_string(),
                secret: f.secret,
                // A default is advertised as a placeholder, not prefilled: the
                // box stays visibly empty so it reads as "leave it alone".
                placeholder: match (&f.hint, &f.default) {
                    (Some(hint), _) => hint.clone(),
                    (None, Some(default)) => format!("Default: {default}"),
                    (None, None) => f.label.to_lowercase(),
                },
                from_backend: f.options_query.is_some(),
                options_url: match (has_credential, &f.options_query) {
                    (true, Some(_)) => format!("/dashboard/{}/options/{}", m.id, f.id),
                    _ => String::new(),
                },
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
            has_settings: fields.iter().any(|f| !f.options_url.is_empty()),
            id: m.id.clone(),
            name: m.name.clone(),
            has_credential,
            public: m.is_public(),
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
            Err(complaint) => return Ok(flash_redirect(&complaint)),
        }
    }

    state
        .store
        .set_credentials(&session.sub, &mcp_id, &values)
        .await
        .map_err(AppError::Internal)?;

    match sync_upstream(&state, mcp, &values, stored.as_ref()).await {
        None => Ok(flash_redirect("Credentials saved")),
        // Stored either way — the proxy injects our value on every request, so
        // the gateway behaves as asked whatever the backend thinks of it.
        Some(complaint) => Ok(flash_redirect(&format!("Credentials saved. {complaint}"))),
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
    headers: HeaderMap,
    Path((mcp_id, field_id)): Path<(String, String)>,
    Form(update): Form<FieldUpdate>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Err(AppError::BadRequest("not signed in".into()));
    };
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

    let message = sync_upstream(&state, mcp, &values, Some(&stored)).await;
    Ok(Json(json!({"saved": true, "message": message})).into_response())
}

/// Suggestions for one field, fetched from the backend with the user's own
/// credentials — e.g. the calendars this account can write to.
///
/// Its own endpoint rather than part of the dashboard render: a backend that is
/// slow or down must not take the page with it, and the form still works as
/// free text if this fails.
pub async fn field_options(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((mcp_id, field_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let Some(session) = current_session(&state, &headers).await else {
        return Err(AppError::BadRequest("not signed in".into()));
    };
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
    let Some(credentials) = state
        .store
        .get_credentials(&session.sub, &mcp_id, mcp.primary_field())
        .await
        .map_err(AppError::Internal)?
    else {
        return Err(AppError::BadRequest("no credentials stored".into()));
    };

    match crate::backend::graphql(&state, mcp, &credentials, query, None).await {
        Ok(data) => Ok(Json(options_of(&data)).into_response()),
        Err(e) => {
            // Wrong password, backend down, a calendar server having a bad day:
            // the user's own error text is more use than a 500.
            tracing::debug!(error = %e, mcp = %mcp_id, field = %field_id, "options lookup failed");
            Ok(Json(json!({"error": e.to_string()})).into_response())
        }
    }
}

/// The options a registry query returned, as a flat array.
///
/// A query may alias a plain list straight to `options`, or land on a Relay
/// connection — `caldav-cli` returns one for every collection — in which case
/// the rows are a level down. Both are unwrapped here so the registry stays a
/// query and nothing else.
fn options_of(data: &Value) -> Value {
    let options = match data.get("options") {
        Some(options) => options,
        None => return json!([]),
    };
    if options.is_array() {
        return options.clone();
    }
    if let Some(nodes) = options.get("nodes").filter(|n| n.is_array()) {
        return nodes.clone();
    }
    if let Some(edges) = options.get("edges").and_then(Value::as_array) {
        return edges
            .iter()
            .filter_map(|e| e.get("node").cloned())
            .collect();
    }
    json!([])
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

#[cfg(test)]
mod tests {
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

    #[test]
    fn takes_a_plain_aliased_list_as_it_comes() {
        let data = json!({"options": [{"value": "a"}]});
        assert_eq!(options_of(&data), json!([{"value": "a"}]));
    }

    #[test]
    fn unwraps_a_relay_connection() {
        let data = json!({"options": {"totalCount": 1, "nodes": [{"value": "Home"}]}});
        assert_eq!(options_of(&data), json!([{"value": "Home"}]));

        let edges = json!({"options": {"edges": [{"node": {"value": "Home"}}]}});
        assert_eq!(options_of(&edges), json!([{"value": "Home"}]));
    }

    #[test]
    fn anything_else_is_no_options_rather_than_an_error() {
        assert_eq!(options_of(&json!({})), json!([]));
        assert_eq!(
            options_of(&json!({"options": {"totalCount": 0}})),
            json!([])
        );
    }
}
