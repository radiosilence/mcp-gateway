//! What the page is made of: the shape each template renders from, and the one
//! constructor behind both the whole page and a single re-rendered section.

use std::collections::HashMap;

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use serde_json::Value;

use crate::config::Mcp;
use crate::dashboard::options::{Verified, options_template, verify};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::Session;

#[derive(Template)]
#[template(path = "index.html")]
pub(super) struct IndexTemplate;

/// One choice offered for a backend-sourced setting.
pub(super) struct OptionView {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) is_default: bool,
    pub(super) selected: bool,
}

#[derive(Template)]
#[template(path = "mcp_section.html")]
pub(super) struct McpSectionTemplate {
    pub(super) m: McpView,
}

#[derive(Template)]
#[template(path = "field_options.html")]
pub(super) struct FieldOptionsTemplate {
    pub(super) mcp_id: String,
    pub(super) field_id: String,
    /// Label of the backend's own default, named in the placeholder so the
    /// empty choice says what it will actually do.
    pub(super) account_default: Option<String>,
    pub(super) options: Vec<OptionView>,
}

#[derive(Template)]
#[template(path = "field_options_error.html")]
pub(super) struct FieldOptionsErrorTemplate {
    pub(super) error: String,
}

#[derive(Template)]
#[template(path = "field_status.html")]
pub(super) struct FieldStatusTemplate {
    pub(super) message: String,
    pub(super) bad: bool,
}

/// One input in an MCP's credential form.
pub(super) struct FieldView {
    pub(super) id: String,
    pub(super) label: String,
    /// `password` or `text` — secrets are never rendered back into the page.
    pub(super) input_type: String,
    pub(super) secret: bool,
    /// Whether something is stored for this field. Only shown for secrets: a
    /// visible field displays its own value, but a password box looks the same
    /// whether one is held or not.
    pub(super) is_set: bool,
    pub(super) placeholder: String,
    /// Prefilled with the stored value when the field is not a secret (so a
    /// server URL can be edited, not retyped). Empty for secrets.
    pub(super) value: String,
    pub(super) required: bool,
    /// This field's choices come from the backend, so it is offered only after
    /// the account is connected — before that there is nobody to ask.
    pub(super) from_backend: bool,
    /// Endpoint serving those choices. Empty until credentials exist.
    pub(super) options_url: String,
    /// The rendered control, when its choices arrived in time to put them in
    /// the page. Empty means the page ships a placeholder and htmx fills it.
    pub(super) options_html: String,
}

/// What a mutation wants to say about itself. Carried on the section because
/// that is what gets re-rendered — there is no page load left to hang a flash
/// message on.
#[derive(Default)]
pub(super) struct Notice {
    pub(super) text: String,
    pub(super) bad: bool,
}

impl Notice {
    pub(super) fn none() -> Self {
        Self::default()
    }

    pub(super) fn said(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bad: false,
        }
    }

    pub(super) fn wrong(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bad: true,
        }
    }
}

pub(super) struct McpView {
    /// Green claims a backend confirmed the credentials, so it is only ever set
    /// when one actually did.
    pub(super) verified: bool,
    pub(super) rejected: bool,
    pub(super) notice: Notice,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) has_credential: bool,
    /// Takes no credentials — connected on login, with no form to fill in and
    /// nothing to disconnect.
    pub(super) public: bool,
    /// Whether any field is a setting rather than a credential — shown outside
    /// the credential form, since that is not what it is.
    pub(super) has_settings: bool,
    pub(super) updated_at: String,
    /// Said in words, because the server has no idea what timezone the reader
    /// is in — no request header carries one — and an exact time in the wrong
    /// zone is worse than none. The precise value stays in the element for
    /// anyone who wants it.
    pub(super) updated_ago: String,
    pub(super) connector_url: String,
    pub(super) claude_code_cmd: String,
    pub(super) key_help_url: String,
    pub(super) fields: Vec<FieldView>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub(super) struct DashboardTemplate {
    pub(super) login: String,
    pub(super) mcps: Vec<McpView>,
}

/// The credential form posts one input per configured field, so the shape
/// isn't known at compile time.
///
/// A plain map, not a newtype around one: `serde_urlencoded` presents a form
/// body as a map and has no `deserialize_newtype_struct` to unwrap it, so a
/// wrapper is rejected at the extractor with "invalid type: map".
pub type CredentialForm = HashMap<String, String>;

impl McpView {
    /// One MCP as the page shows it. Shared because a mutation re-renders a
    /// single section, and a second construction path would be a second place
    /// for the badge, the timestamp and the settings to disagree.
    pub(super) async fn build(
        state: &AppState,
        session: &Session,
        m: &Mcp,
        prefetched: &HashMap<(String, String), (Value, String)>,
    ) -> AppResult<Self> {
        let base = state.config.public_url.trim_end_matches('/');
        let meta = state
            .store
            .credential_meta(&session.sub, &m.id)
            .await
            .map_err(AppError::Internal)?;
        // A public MCP is connected the moment the user has logged in — there
        // is nothing to store, so nothing to wait for.
        let (has_credential, updated_at, updated_ago) = match (m.is_public(), meta) {
            (true, _) => (true, String::new(), String::new()),
            (false, Some(meta)) => (true, meta.updated_at_rfc3339(), meta.updated_ago()),
            (false, None) => (false, String::new(), String::new()),
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
                options_html: prefetched
                    .get(&(m.id.clone(), f.id.clone()))
                    .map(|(options, current)| {
                        render_to_string(options_template(options, current, &m.id, &f.id))
                    })
                    .unwrap_or_default(),
                value: match f.secret {
                    true => String::new(),
                    false => stored
                        .as_ref()
                        .and_then(|s| s.get(&f.id))
                        .cloned()
                        .unwrap_or_default(),
                },
                is_set: stored.as_ref().is_some_and(|s| s.contains_key(&f.id)),
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

        // Only worth asking when there is something to ask about.
        let checked = match has_credential && !m.is_public() {
            true => verify(state, &session.sub, m).await,
            false => Verified::Unknown,
        };

        let connector_url = format!("{base}/{}", m.id);
        Ok(McpView {
            claude_code_cmd: format!(
                "claude mcp add --transport http --scope user {} {}",
                m.id, connector_url
            ),
            has_settings: fields.iter().any(|f| !f.options_url.is_empty()),
            id: m.id.clone(),
            name: m.name.clone(),
            has_credential,
            verified: checked == Verified::Yes,
            rejected: checked == Verified::Rejected,
            public: m.is_public(),
            updated_at,
            updated_ago,
            connector_url,
            key_help_url: m.key_help_url.clone().unwrap_or_default(),
            fields,
            notice: Notice::none(),
        })
    }
}

/// Empty on failure: a fragment that will not render is a template bug, and the
/// page is more use with a placeholder in that slot than not at all.
/// The one place an unknown MCP id is turned away.
pub(super) fn mcp_or_404<'a>(state: &'a AppState, id: &str) -> AppResult<&'a Mcp> {
    state
        .config
        .mcp(id)
        .ok_or_else(|| AppError::BadRequest("unknown mcp".into()))
}

pub(super) fn render_to_string<T: Template>(template: T) -> String {
    template.render().unwrap_or_else(|e| {
        tracing::error!(error = %e, "fragment render failed");
        String::new()
    })
}

pub(super) fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "fragment render failed");
            Html("<span data-status>Could not render</span>").into_response()
        }
    }
}
