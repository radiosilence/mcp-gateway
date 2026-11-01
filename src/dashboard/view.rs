//! The views templates render from. Nothing here talks to a backend.

use std::collections::HashMap;

use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::config::{Config, Mcp};
use crate::dashboard::options::Verified;
use crate::error::AppError;
use crate::store::{CredentialSet, TokenMeta};

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

/// The connection badge on its own: the status endpoint's answer.
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
    /// Label of the backend's own default, named in the empty choice.
    pub(super) account_default: Option<String>,
    pub(super) options: Vec<OptionView>,
}

#[derive(Template)]
#[template(path = "field_options_error.html")]
pub(super) struct FieldOptionsErrorTemplate {
    pub(super) mcp_id: String,
    pub(super) field_id: String,
    pub(super) error: String,
}

#[derive(Template)]
#[template(path = "field_status.html")]
pub(super) struct FieldStatusTemplate {
    pub(super) id: String,
    pub(super) message: String,
    pub(super) bad: bool,
}

/// One input in an MCP's credential form.
pub(super) struct FieldView {
    pub(super) id: String,
    pub(super) label: String,
    /// `password` or `text`.
    pub(super) input_type: String,
    pub(super) secret: bool,
    /// Whether something is stored; shown for secrets, whose box is empty.
    pub(super) is_set: bool,
    pub(super) placeholder: String,
    /// The stored value, for non-secrets only.
    pub(super) value: String,
    pub(super) required: bool,
    /// Choices come from the backend, so it is offered only once connected.
    pub(super) from_backend: bool,
    /// Endpoint serving those choices. Empty until credentials exist.
    pub(super) options_url: String,
}

/// What a mutation says about itself, shown at the top of its section.
#[derive(Default)]
pub(super) struct Notice {
    pub(super) text: String,
    pub(super) bad: bool,
}

impl Notice {
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
    /// A backend confirmed the credentials, or none declares a check.
    pub(super) verified: bool,
    pub(super) rejected: bool,
    /// Non-empty while a backend has yet to be asked: the badge renders as
    /// pending and fetches its replacement from here.
    pub(super) status_url: String,
    pub(super) notice: Notice,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) has_credential: bool,
    /// Takes no credentials: connected on login, nothing to disconnect.
    pub(super) public: bool,
    /// Whether any field is a backend-sourced setting, shown outside the form.
    pub(super) has_settings: bool,
    pub(super) updated_at: String,
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

/// One input per configured field. A plain map: `serde_urlencoded` cannot
/// deserialize a newtype around one.
pub type CredentialForm = HashMap<String, String>;

impl McpView {
    /// One MCP as the page shows it, from what is stored for it. The page and
    /// a re-rendered section both come through here.
    pub(super) fn new(
        config: &Config,
        m: &Mcp,
        stored: Option<(CredentialSet, TokenMeta)>,
    ) -> Self {
        let (stored, meta) = stored.unzip();
        let has_credential = m.public || meta.is_some();
        let (updated_at, updated_ago) = meta
            .filter(|_| !m.public)
            .map(|meta| (meta.updated_at_rfc3339(), meta.updated_ago()))
            .unwrap_or_default();

        let fields: Vec<FieldView> = m
            .fields
            .iter()
            .map(|f| FieldView {
                // Secrets are never rendered back into the page.
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
                // A default is a placeholder, not a prefill, so the box reads as
                // "leave it alone".
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

        let needs_check = has_credential && !m.public && m.verify.is_some();
        let connector_url = format!("{}/{}", config.public_url.trim_end_matches('/'), m.id);
        McpView {
            claude_code_cmd: format!(
                "claude mcp add --transport http --scope user {} {}",
                m.id, connector_url
            ),
            has_settings: fields.iter().any(|f| !f.options_url.is_empty()),
            id: m.id.clone(),
            name: m.name.clone(),
            has_credential,
            // With no check declared there is nothing to doubt.
            verified: m.verify.is_none(),
            rejected: false,
            status_url: match needs_check {
                true => format!("/dashboard/{}/status", m.id),
                false => String::new(),
            },
            public: m.public,
            updated_at,
            updated_ago,
            connector_url,
            key_help_url: m.key_help_url.clone().unwrap_or_default(),
            fields,
            notice: Notice::default(),
        }
    }

    /// The badge's element id, which the status patch targets.
    pub(super) fn status_id(&self) -> String {
        format!("status-{}", self.id)
    }

    /// Settle the badge with what a backend said. Clearing `status_url` stops
    /// the replacement badge from fetching itself again.
    pub(super) fn checked(mut self, verdict: Verified) -> Self {
        self.verified = verdict == Verified::Yes;
        self.rejected = verdict == Verified::Rejected;
        self.status_url = String::new();
        self
    }
}

/// A fragment as `text/html`, which Datastar morphs into the page by the ids of
/// its top-level elements.
pub(super) fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => AppError::Internal(e.into()).into_response(),
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
            notice: Notice::default(),
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

    /// An answer that kept its URL would fetch itself for as long as the tab is
    /// open.
    #[test]
    fn a_settled_badge_does_not_ask_again() {
        assert!(badge(pending()).contains("data-init=\"@get('/dashboard/caldav/status')\""));
        for verdict in [Verified::Yes, Verified::Rejected, Verified::Unknown] {
            assert!(
                !badge(pending().checked(verdict)).contains("data-init"),
                "{verdict:?} left the badge fetching itself"
            );
        }
    }

    /// A patch aimed at an id nothing carries fails silently.
    #[test]
    fn the_badge_carries_the_id_the_patch_will_look_for() {
        for verdict in [Verified::Yes, Verified::Rejected, Verified::Unknown] {
            assert!(badge(pending().checked(verdict)).contains(r#"id="status-caldav""#));
        }
        assert!(badge(pending()).contains(r#"id="status-caldav""#));
    }

    #[test]
    fn a_verdict_reads_as_the_badge_it_should() {
        assert!(badge(pending().checked(Verified::Yes)).contains("Connected"));
        assert!(badge(pending().checked(Verified::Rejected)).contains("Credentials rejected"));
        // Asked, and the backend could not say. Not green: that would be a
        // claim we have no grounds for.
        assert!(badge(pending().checked(Verified::Unknown)).contains("Configured"));
    }

    /// A check in flight must not read as "Not configured".
    #[test]
    fn pending_is_not_mistaken_for_unconfigured() {
        assert!(!badge(pending()).contains("Not configured"));
    }

    /// The skeleton and the control replacing it may differ only in surface; a
    /// box-model class on one alone makes the page jump on the swap.
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

    /// Both pages, since the login page is the one in front of you when the
    /// dashboard won't load.
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

/// The dashboard rendered from fixtures to a static file, every section state
/// on one page, so a layout change can be reviewed without a login.
/// `mise run preview`.
#[cfg(test)]
mod preview {
    use super::*;

    fn mcp(name: &str, id: &str) -> McpView {
        McpView {
            verified: false,
            rejected: false,
            status_url: String::new(),
            notice: Notice::default(),
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
        // Without Datastar the pending states stay on screen.
        let out = std::env::var("PREVIEW_OUT").expect("PREVIEW_OUT");
        std::fs::write(&out, html).unwrap_or_else(|e| panic!("writing {out}: {e}"));
    }
}
