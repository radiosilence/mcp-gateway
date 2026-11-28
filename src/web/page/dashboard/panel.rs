//! One MCP's panel: its badge, how to add it to Claude, its settings, and the
//! form for its credentials. Saving or disconnecting answers with the panel
//! redrawn, since nothing else draws it again.

use std::collections::HashMap;

use askama::Template;
use askama_datastar::{ElementPatchMode, Html};
use axum::extract::{Form, Path, State};
use axum::response::Response;
use axum::response::sse::Event;
use koan_kit_web as kit;

use super::badge::Badge;
use super::setting::Setting;
use crate::auth::extract::FragmentSession;
use crate::backend::sync::sync;
use crate::config::{CredentialField, Mcp};
use crate::error::AppResult;
use crate::state::AppState;
use crate::store::{CredentialSet, TokenMeta};
use crate::web::{Draw, flash};

/// Where the credential form saves.
pub(in crate::web) const CREDENTIALS: &str = "/dashboard/{mcp}/credentials";
/// Where Disconnect posts.
pub(in crate::web) const DISCONNECT: &str = "/dashboard/{mcp}/disconnect";

#[derive(Template)]
#[template(path = "dashboard/panel.html")]
pub(super) struct Panel {
    mcp: String,
    name: String,
    badge: Html,
    connection: Connection,
}

enum Connection {
    /// Nothing stored yet: the form, and where to get what it asks for.
    Unconfigured { form: Html, help: Option<String> },
    /// Takes no credentials, so it is connected as the person signs in.
    Open {
        connector: Html,
        settings: Vec<Html>,
    },
    /// Credentials stored.
    Configured {
        /// When the credentials were last saved, in Unix milliseconds.
        updated_ms: i64,
        connector: Html,
        settings: Vec<Html>,
        form: Html,
    },
}

/// How to add the MCP to Claude: its connector URL, and the command that adds
/// it to Claude Code.
#[derive(Template)]
#[template(path = "dashboard/connector.html")]
struct Connector {
    mcp: String,
    url: String,
    command: String,
}

impl Connector {
    fn of(s: &AppState, mcp: &Mcp) -> Self {
        let url = format!("{}/{}", s.config.public_url.trim_end_matches('/'), mcp.id);
        Self {
            mcp: mcp.id.clone(),
            command: format!(
                "claude mcp add --transport http --scope user {} {url}",
                mcp.id
            ),
            url,
        }
    }

    fn url_id(&self) -> String {
        format!("connector-{}", self.mcp)
    }

    fn command_id(&self) -> String {
        format!("command-{}", self.mcp)
    }
}

/// The credential form, for connecting and for editing once connected.
/// Settings are not in it: they are saved on their own.
#[derive(Template)]
#[template(path = "dashboard/credentials.html")]
struct Credentials {
    mcp: String,
    url: String,
    inputs: Vec<Input>,
    connected: bool,
}

impl Credentials {
    /// The form's id, which its fields' ids are built on: the same form is on
    /// the page once per MCP.
    fn id(&self) -> String {
        format!("credentials-{}", self.mcp)
    }

    fn submit(&self) -> &'static str {
        match self.connected {
            true => "Save",
            false => "Connect",
        }
    }
}

/// One input of the credential form.
struct Input {
    name: String,
    label: String,
    kind: kit::InputKind,
    secret: bool,
    /// Whether something is stored; a secret's box is empty either way.
    stored: bool,
    /// The stored value, for a field that is not secret.
    value: Option<String>,
    placeholder: String,
    autocomplete: String,
    required: bool,
}

impl Input {
    fn of(f: &CredentialField, stored: Option<&CredentialSet>) -> Self {
        let value = stored.and_then(|s| s.get(&f.id));
        Self {
            name: f.id.clone(),
            label: match f.required {
                true => f.label.clone(),
                false => format!("{} (optional)", f.label),
            },
            // A password manager reads a password input as a password,
            // whatever its token, so a secret with a token is plain text.
            kind: match f.secret && f.autocomplete.is_none() {
                true => kit::InputKind::Password,
                false => kit::InputKind::Text,
            },
            secret: f.secret,
            stored: value.is_some(),
            // Secrets are never drawn back into the page.
            value: value.filter(|_| !f.secret).cloned(),
            // A default is a placeholder, not a prefill, so the box reads as
            // "leave it alone".
            placeholder: match (&f.hint, &f.default) {
                (Some(hint), _) => hint.clone(),
                (None, Some(default)) => format!("Default: {default}"),
                (None, None) => f.label.to_lowercase(),
            },
            autocomplete: f.autocomplete.clone().unwrap_or_else(|| "off".into()),
            required: f.required,
        }
    }

    fn control(&self) -> kit::Input<'_> {
        let input = kit::Input::new(self.kind, &self.name)
            .placeholder(&self.placeholder)
            .attr("autocomplete", &self.autocomplete);
        let input = match &self.value {
            Some(value) => input.value(value),
            None => input,
        };
        // A blank secret keeps the stored one, so it is not required then.
        match self.required && !(self.secret && self.stored) {
            true => input.required(),
            false => input,
        }
    }

    fn hint(&self) -> &'static str {
        match self.secret && self.stored {
            true => "Stored. Leave it blank to keep it.",
            false => "",
        }
    }
}

impl Panel {
    pub(super) fn id_of(mcp: &str) -> String {
        format!("panel-{mcp}")
    }

    /// One MCP as the page shows it, from what is stored for it.
    pub(super) fn of(
        s: &AppState,
        sub: &str,
        mcp: &Mcp,
        stored: Option<(CredentialSet, TokenMeta)>,
    ) -> Self {
        let (values, meta) = match &stored {
            Some((values, meta)) => (Some(values), Some(meta)),
            None => (None, None),
        };
        let connector = || Connector::of(s, mcp).draw();
        let settings = || {
            mcp.fields
                .iter()
                .filter(|f| f.options_query.is_some())
                .map(|f| Setting::of(s, sub, mcp, f, values.zip(meta)).draw())
                .collect()
        };
        let form = || {
            Credentials {
                mcp: mcp.id.clone(),
                url: CREDENTIALS.replace("{mcp}", &mcp.id),
                inputs: mcp
                    .fields
                    .iter()
                    .filter(|f| f.options_query.is_none())
                    .map(|f| Input::of(f, values))
                    .collect(),
                connected: meta.is_some(),
            }
            .draw()
        };
        let connection = match (mcp.no_auth, meta) {
            (true, _) => Connection::Open {
                connector: connector(),
                settings: settings(),
            },
            (false, Some(meta)) => Connection::Configured {
                updated_ms: meta.updated_at.unix_timestamp() * 1000
                    + i64::from(meta.updated_at.millisecond()),
                connector: connector(),
                settings: settings(),
                form: form(),
            },
            (false, None) => Connection::Unconfigured {
                form: form(),
                help: mcp.key_help_url.clone(),
            },
        };
        Self {
            mcp: mcp.id.clone(),
            name: mcp.name.clone(),
            badge: Badge::of(s, sub, mcp, meta).draw(),
            connection,
        }
    }

    fn id(&self) -> String {
        Self::id_of(&self.mcp)
    }

    fn disconnect_url(&self) -> String {
        DISCONNECT.replace("{mcp}", &self.mcp)
    }

    fn disconnect_question(&self) -> String {
        format!(
            "Disconnect {}? This wipes your stored credentials.",
            self.name
        )
    }
}

/// The panel re-read from the store, as the answer to every change to it.
/// It replaces the panel rather than morphing into it, since a morph keeps
/// what was typed into an input the server left empty, a secret included.
async fn redrawn(s: &AppState, sub: &str, mcp: &Mcp) -> AppResult<Event> {
    let stored = s
        .store
        .get_credentials_with_meta(sub, &mcp.id, mcp.primary_field())
        .await?;
    Ok(Panel::of(s, sub, mcp, stored)
        .draw()
        .patch()
        .mode(ElementPatchMode::Replace)
        .into())
}

/// One input per field. A plain map: `serde_urlencoded` cannot deserialize a
/// newtype around one.
pub(in crate::web) type CredentialForm = HashMap<String, String>;

/// Save the credential form, then tell the backend of any setting it
/// changed. A complaint leaves the form as the person typed it.
pub(in crate::web) async fn save(
    State(s): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
    Form(form): Form<CredentialForm>,
) -> AppResult<Response> {
    let mcp = super::mcp(&s, &mcp_id)?;
    let saved = s
        .store
        .update_credentials(&session.sub, &mcp.id, mcp.primary_field(), |stored| {
            let mut values = CredentialSet::new();
            for field in &mcp.fields {
                let submitted = form.get(&field.id).map(String::as_str);
                let stored_value = stored.and_then(|s| s.get(&field.id)).map(String::as_str);
                if let Some(value) = resolve(field, submitted, stored_value)? {
                    values.insert(field.id.clone(), value);
                }
            }
            Ok::<_, String>(values)
        })
        .await?;
    let (stored, values) = match saved {
        Ok(saved) => saved,
        Err(complaint) => return Ok(askama_datastar::events([flash(Err(&complaint))])),
    };
    let said = match sync(&s, mcp, &values, stored.as_ref()).await {
        // Stored either way: the proxy injects the gateway's value regardless.
        Some(complaint) => Err(format!("Saved. {complaint}")),
        None => Ok("Saved".to_string()),
    };
    Ok(askama_datastar::events([
        redrawn(&s, &session.sub, mcp).await?,
        flash(said.as_deref().map_err(String::as_str)),
    ]))
}

/// Wipe the stored credentials, once the page's Confirm has asked.
pub(in crate::web) async fn disconnect(
    State(s): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    let mcp = super::mcp(&s, &mcp_id)?;
    s.store.delete_credential(&session.sub, &mcp.id).await?;
    Ok(askama_datastar::events([
        redrawn(&s, &session.sub, mcp).await?,
        flash(Ok("Disconnected")),
        kit::Confirm::close().into(),
    ]))
}

/// What to store for one field, given what the form sent and what is stored.
/// `Ok(None)` stores nothing; `Err` is the complaint to show the person.
///
/// Absent and empty differ: a field the form never carried (a setting, saved
/// on its own) keeps its stored value.
fn resolve(
    field: &CredentialField,
    submitted: Option<&str>,
    stored: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(value) = submitted.map(str::trim) else {
        return Ok(stored.map(str::to_string));
    };

    if !value.is_empty() {
        // These become header values, so refuse them here where the person
        // can be told rather than dropping them at proxy time.
        if value.chars().any(char::is_control) {
            return Err(format!(
                "{} contains characters that can't be sent",
                field.label
            ));
        }
        return Ok(Some(value.to_string()));
    }

    // A secret is never drawn back, so blank means "keep it". A blank
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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::{Request, header};
    use serde_json::json;

    use super::*;

    fn field(id: &str, secret: bool, required: bool) -> CredentialField {
        serde_json::from_value(json!({
            "id": id, "label": id, "header": format!("X-{id}"),
            "secret": secret, "required": required,
        }))
        .unwrap()
    }

    #[test]
    fn an_optional_field_left_blank_stores_nothing() {
        let optional = field("app_key", true, false);
        assert_eq!(resolve(&optional, Some(""), None), Ok(None));
    }

    #[test]
    fn a_field_the_form_never_carried_keeps_what_is_stored() {
        let setting = field("calendar", false, false);
        assert_eq!(
            resolve(&setting, None, Some("home")),
            Ok(Some("home".into()))
        );
        assert_eq!(resolve(&setting, None, None), Ok(None));
    }

    #[test]
    fn a_blank_secret_keeps_the_stored_one() {
        let password = field("password", true, true);
        assert_eq!(
            resolve(&password, Some(""), Some("hunter2")),
            Ok(Some("hunter2".into()))
        );
        assert!(resolve(&password, Some(""), None).is_err());
    }

    #[test]
    fn a_visibly_cleared_setting_is_cleared() {
        let setting = field("calendar", false, false);
        assert_eq!(resolve(&setting, Some(""), Some("home")), Ok(None));
    }

    #[test]
    fn a_cleared_optional_falls_back_to_its_default() {
        let mut url = field("url", false, false);
        url.default = Some("https://caldav.icloud.com".into());
        assert_eq!(
            resolve(&url, Some(""), Some("https://other.test")),
            Ok(Some("https://caldav.icloud.com".into()))
        );
    }

    #[test]
    fn a_value_that_could_not_be_sent_as_a_header_is_refused() {
        let setting = field("calendar", false, false);
        assert!(resolve(&setting, Some("we\nird"), None).is_err());
    }

    /// A wrapper type around the map is rejected by the extractor.
    #[tokio::test]
    async fn the_credential_form_deserializes_from_a_urlencoded_body() {
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
        assert_eq!(form.get("url").map(String::as_str), Some(""));
    }

    /// A secret is never drawn back, and one that is stored need not be typed
    /// again.
    #[test]
    fn a_stored_secret_is_neither_shown_nor_required() {
        let stored = CredentialSet::from([("password".into(), "hunter2".into())]);
        let input = Input::of(&field("password", true, true), Some(&stored));
        let html = kit::Field::new(&input.label, input.control())
            .hint(input.hint())
            .to_string();
        assert!(
            !html.contains("hunter2") && !html.contains("required"),
            "{html}"
        );
        assert!(html.contains("Leave it blank"), "{html}");
    }
}
