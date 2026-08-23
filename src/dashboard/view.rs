//! What the page is made of: the shape each template renders from, and the one
//! constructor behind both the whole page and a single re-rendered section.
//!
//! Nothing here talks to a backend. `build` reads this gateway's own database
//! and returns, so what a page costs is what our Postgres costs; everything
//! that has to ask somebody else — a setting's choices, whether a credential
//! still works — is an endpoint htmx calls once the page is already up.

use std::collections::HashMap;

use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::config::Mcp;
use crate::dashboard::options::Verified;
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

/// The connection badge on its own, which is what the status endpoint answers
/// with. Same template the section includes, so the pill htmx swaps in cannot
/// look like a different thing from the one it replaced.
#[derive(Template)]
#[template(path = "mcp_badge.html")]
pub(super) struct McpBadgeTemplate {
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
    /// when one actually did — or when there is no check declared, which is the
    /// one case the answer is known without asking.
    pub(super) verified: bool,
    pub(super) rejected: bool,
    /// Endpoint that will say which of those two it is. Non-empty exactly when
    /// a backend has to be asked, which is what the badge keys off: the page
    /// renders a pending pill and htmx replaces it, and every other MCP gets
    /// its final badge in the first response.
    pub(super) status_url: String,
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
    pub(super) async fn build(state: &AppState, session: &Session, m: &Mcp) -> AppResult<Self> {
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
        // Fetched whenever anything is stored, not only when a visible field
        // wants prefilling: a secret needs it too, to say that it is set.
        let stored = if has_credential {
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

        // Somebody has to be asked exactly when there is a check declared and a
        // credential to run it against. Everything else the badge needs is
        // already here, so only these MCPs pay for a round trip — and they pay
        // for it after the page is on screen rather than in front of it.
        let needs_check = has_credential && !m.is_public() && m.verify.is_some();

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
            // A backend with no check declared is taken at its word. Withholding
            // green would imply a doubt we have no grounds for — we never asked.
            verified: m.verify.is_none(),
            rejected: false,
            status_url: match needs_check {
                true => format!("/dashboard/{}/status", m.id),
                false => String::new(),
            },
            public: m.is_public(),
            updated_at,
            updated_ago,
            connector_url,
            key_help_url: m.key_help_url.clone().unwrap_or_default(),
            fields,
            notice: Notice::none(),
        })
    }

    /// Settle the badge with what a backend actually said.
    ///
    /// Clearing `status_url` is what stops the swapped-in pill asking again:
    /// the fragment renders from the same template as the placeholder, so a
    /// still-set URL would give it another `hx-trigger="load"` and it would
    /// fetch itself forever.
    pub(super) fn checked(mut self, verdict: Verified) -> Self {
        self.verified = verdict == Verified::Yes;
        self.rejected = verdict == Verified::Rejected;
        self.status_url = String::new();
        self
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

pub(super) fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "fragment render failed");
            Html("<span data-status>Could not render</span>").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connected, with a check declared and nobody asked yet.
    fn pending() -> McpView {
        McpView {
            verified: false,
            rejected: false,
            status_url: "/dashboard/caldav/status".into(),
            notice: Notice::none(),
            id: "caldav".into(),
            name: "CalDAV".into(),
            has_credential: true,
            public: false,
            has_settings: false,
            updated_at: String::new(),
            updated_ago: String::new(),
            connector_url: String::new(),
            claude_code_cmd: String::new(),
            key_help_url: String::new(),
            fields: Vec::new(),
        }
    }

    fn badge(m: McpView) -> String {
        McpBadgeTemplate { m }.render().unwrap()
    }

    /// One backend-sourced setting, which is what puts a skeleton on the page.
    fn with_a_setting() -> McpView {
        McpView {
            has_settings: true,
            fields: vec![FieldView {
                id: "calendar".into(),
                label: "Calendar".into(),
                input_type: "text".into(),
                secret: false,
                is_set: false,
                placeholder: String::new(),
                value: String::new(),
                required: false,
                from_backend: true,
                options_url: "/dashboard/caldav/options/calendar".into(),
            }],
            ..pending()
        }
    }

    /// The classes on the first `<select>`, which is the skeleton in a section
    /// and the real control in an options fragment.
    fn select_classes(html: &str) -> std::collections::BTreeSet<String> {
        let tag = html[html.find("<select").expect("a select")..]
            .split('>')
            .next()
            .expect("a closing bracket");
        let attr = &tag[tag.find("class=\"").expect("a class attribute") + 7..];
        attr[..attr.find('"').expect("a closing quote")]
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    /// The one way this shape can go wrong: the answer renders from the same
    /// template as the question, so an answer that kept its URL would carry
    /// another `hx-trigger="load"` and fetch itself for as long as the tab is
    /// open — a backend hammered by every idle dashboard.
    #[test]
    fn a_settled_badge_does_not_ask_again() {
        assert!(badge(pending()).contains("hx-get=\"/dashboard/caldav/status\""));
        for verdict in [Verified::Yes, Verified::Rejected, Verified::Unknown] {
            assert!(
                !badge(pending().checked(verdict)).contains("hx-get"),
                "{verdict:?} left the badge fetching itself"
            );
        }
    }

    #[test]
    fn a_verdict_reads_as_the_badge_it_should() {
        assert!(badge(pending().checked(Verified::Yes)).contains("Connected"));
        assert!(badge(pending().checked(Verified::Rejected)).contains("Credentials rejected"));
        // Asked, and the backend could not say. Not green: that would be a
        // claim we have no grounds for.
        assert!(badge(pending().checked(Verified::Unknown)).contains("Configured"));
    }

    /// The pending pill is only ever shown for a credential we hold, so it must
    /// win over the badge for one we don't — otherwise a check that is still in
    /// flight reads as "Not configured".
    #[test]
    fn pending_is_not_mistaken_for_unconfigured() {
        assert!(!badge(pending()).contains("Not configured"));
    }

    /// The skeleton and the control that replaces it may differ in how they
    /// look and in nothing else. A class that changes the box on one and not
    /// the other moves the page when htmx swaps them, which is a bug this
    /// pairing has already had once — so the assertion is on the whole
    /// difference between them, not on a list of classes to keep in step.
    #[test]
    fn a_loading_field_differs_from_a_loaded_one_only_in_surface() {
        let skeleton = select_classes(
            &McpSectionTemplate {
                m: with_a_setting(),
            }
            .render()
            .unwrap(),
        );
        let loaded = select_classes(
            &FieldOptionsTemplate {
                mcp_id: "caldav".into(),
                field_id: "calendar".into(),
                account_default: None,
                options: Vec::new(),
            }
            .render()
            .unwrap(),
        );

        let only_skeleton: Vec<_> = skeleton.difference(&loaded).cloned().collect();
        let only_loaded: Vec<_> = loaded.difference(&skeleton).cloned().collect();
        assert_eq!(
            only_skeleton,
            [
                "animate-pulse",
                "bg-slate-200",
                "dark:bg-slate-700",
                "pointer-events-none",
                "text-transparent",
            ]
        );
        assert_eq!(
            only_loaded,
            [
                "bg-slate-100",
                "dark:bg-slate-800",
                "focus:ring-2",
                "focus:ring-indigo-500",
            ]
        );
    }

    /// Both full pages, because the whole use of a version in the corner is
    /// being able to read it off whichever page is in front of you — and the
    /// login page is the one you are looking at when the dashboard won't load.
    #[test]
    fn every_page_says_which_build_it_is() {
        let want = format!("v{}", crate::VERSION);
        assert!(IndexTemplate.render().unwrap().contains(&want));
        assert!(
            DashboardTemplate {
                login: "somebody".into(),
                mcps: Vec::new(),
            }
            .render()
            .unwrap()
            .contains(&want)
        );
    }
}

/// The dashboard as a static file, so how it looks can be reviewed without a
/// login.
///
/// Standing the real thing up needs Hydra and the login provider, which is more
/// than a spacing change is worth — and the alternative is guessing, which is
/// how the loading field ended up indistinguishable from an empty one. Fixtures
/// rather than a database: every state the section has, on one page, including
/// the ones a given account may never be in.
///
/// `mise run preview`. Ignored by default because it writes a file and answers
/// a question CI is not asking.
#[cfg(test)]
mod preview {
    use super::*;

    fn mcp(name: &str, id: &str) -> McpView {
        McpView {
            verified: false,
            rejected: false,
            status_url: String::new(),
            notice: Notice::none(),
            id: id.into(),
            name: name.into(),
            has_credential: true,
            public: false,
            has_settings: false,
            updated_at: "2026-08-20T10:00:00Z".into(),
            updated_ago: "3 days ago".into(),
            connector_url: format!("https://mcp.blit.cc/{id}"),
            claude_code_cmd: format!(
                "claude mcp add --transport http --scope user {id} https://mcp.blit.cc/{id}"
            ),
            key_help_url: String::new(),
            fields: Vec::new(),
        }
    }

    fn setting(label: &str, id: &str) -> FieldView {
        FieldView {
            id: id.into(),
            label: label.into(),
            input_type: "text".into(),
            secret: false,
            is_set: false,
            placeholder: String::new(),
            value: String::new(),
            required: false,
            from_backend: true,
            options_url: format!("/dashboard/caldav/options/{id}"),
        }
    }

    fn secret(label: &str, id: &str) -> FieldView {
        FieldView {
            id: id.into(),
            label: label.into(),
            input_type: "password".into(),
            secret: true,
            is_set: true,
            placeholder: "fmu1-…".into(),
            value: String::new(),
            required: true,
            from_backend: false,
            options_url: String::new(),
        }
    }

    #[test]
    #[ignore = "writes a file; run it through `mise run preview`"]
    fn write_preview() {
        let mcps = vec![
            McpView {
                status_url: "/dashboard/fastmail/status".into(),
                fields: vec![secret("API token", "token")],
                ..mcp("Fastmail", "fastmail")
            },
            McpView {
                status_url: "/dashboard/caldav/status".into(),
                has_settings: true,
                fields: vec![
                    setting("Default calendar for new events", "calendar"),
                    secret("App password", "password"),
                ],
                ..mcp("CalDAV", "caldav")
            },
            McpView {
                public: true,
                verified: true,
                updated_at: String::new(),
                updated_ago: String::new(),
                ..mcp("Folk", "folk")
            },
            McpView {
                has_credential: false,
                key_help_url: "https://api-portal.tfl.gov.uk".into(),
                fields: vec![secret("App key", "key")],
                ..mcp("TfL", "tfl")
            },
        ];
        let html = DashboardTemplate {
            login: "radiosilence".into(),
            mcps,
        }
        .render()
        .unwrap();
        let css = include_str!("../../assets/app.css");
        let html = html
            .replace(
                r#"<link rel="stylesheet" href="/assets/app.css" />"#,
                &format!("<style>{css}</style>"),
            )
            .replace(
                r#"<script type="module" src="/assets/app.js"></script>"#,
                "",
            );
        // htmx is dropped along with the stylesheet link, so the page keeps
        // the states it was rendered in: the skeleton stays a skeleton and the
        // badge stays pending, which is the half that is otherwise hardest to
        // catch in the act.
        let out = std::env::var("PREVIEW_OUT").expect("PREVIEW_OUT");
        std::fs::write(&out, html).unwrap_or_else(|e| panic!("writing {out}: {e}"));
    }
}
