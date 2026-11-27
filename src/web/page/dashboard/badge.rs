//! One MCP's connection badge: whether its backend accepts what is stored.
//! The panel draws it checking, and it loads its own answer once the page is
//! up, so a slow backend cannot hold the page.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use koan_kit_web as kit;

use super::stamp;
use crate::auth::extract::FragmentSession;
use crate::backend::verify::{Verified, verify};
use crate::config::Mcp;
use crate::error::AppResult;
use crate::state::AppState;
use crate::store::TokenMeta;
use crate::web::{Draw, one};

/// Where a badge loads its answer from.
pub(in crate::web) const STATUS: &str = "/dashboard/{mcp}/status";

#[derive(Template)]
#[template(path = "dashboard/badge.html")]
pub(super) struct Badge {
    mcp: String,
    state: BadgeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BadgeState {
    /// The backend is being asked; the badge loads its answer.
    Checking,
    NotConfigured,
    /// The backend accepted the credentials, or no check is declared, so there
    /// is nothing to doubt.
    Connected,
    /// The backend said they are wrong. For an MCP that takes no credentials,
    /// the backend's own account is what it refused.
    Rejected {
        no_auth: bool,
    },
    /// Stored, or taking none, but the backend could not say whether it works.
    Unknown {
        no_auth: bool,
    },
}

impl Badge {
    pub(super) fn id_of(mcp: &str) -> String {
        format!("mcp-{mcp}-status")
    }

    /// The badge for `mcp` as the page draws it: settled when there is nothing
    /// to ask, or the backend's answer stands, and checking otherwise.
    pub(super) fn of(s: &AppState, sub: &str, mcp: &Mcp, stored: Option<&TokenMeta>) -> Self {
        let state = match (mcp.no_auth || stored.is_some(), mcp.verify.is_some()) {
            (false, _) => BadgeState::NotConfigured,
            (true, false) => BadgeState::Connected,
            (true, true) => s
                .questions
                .verdicts
                .peek(&key(sub, mcp, stored))
                .map_or(BadgeState::Checking, |v| BadgeState::settled(mcp, v)),
        };
        Self {
            mcp: mcp.id.clone(),
            state,
        }
    }

    fn id(&self) -> String {
        Self::id_of(&self.mcp)
    }

    fn url(&self) -> String {
        STATUS.replace("{mcp}", &self.mcp)
    }

    fn checking(&self) -> bool {
        self.state == BadgeState::Checking
    }

    /// The badge once settled.
    fn settled(&self) -> kit::Status<'static> {
        match self.state {
            BadgeState::Checking => kit::Status::wait("Checking…"),
            BadgeState::NotConfigured => kit::Status::warn("Not configured"),
            BadgeState::Connected => kit::Status::ok("Connected"),
            BadgeState::Rejected { no_auth: true } => kit::Status::bad("Backend signed out"),
            BadgeState::Rejected { no_auth: false } => kit::Status::bad("Credentials rejected"),
            // Not `ok`, which would claim it works, and not `wait`, which reads
            // as still checking.
            BadgeState::Unknown { no_auth: true } => {
                kit::Status::wait("Status unknown").tone_class("unknown")
            }
            BadgeState::Unknown { no_auth: false } => {
                kit::Status::wait("Configured").tone_class("unknown")
            }
        }
    }
}

impl BadgeState {
    fn settled(mcp: &Mcp, verdict: Verified) -> Self {
        let no_auth = mcp.no_auth;
        match verdict {
            Verified::Yes => Self::Connected,
            Verified::Rejected => Self::Rejected { no_auth },
            Verified::Unknown => Self::Unknown { no_auth },
        }
    }
}

/// The question a badge asks: whether these credentials, as last saved, work.
fn key(sub: &str, mcp: &Mcp, stored: Option<&TokenMeta>) -> String {
    format!("{sub}\n{}\n{}", mcp.id, stamp(stored))
}

/// The badge's answer, asked of the backend through the server's questions.
pub(in crate::web) async fn serve(
    State(s): State<AppState>,
    FragmentSession(session): FragmentSession,
    Path(mcp_id): Path<String>,
) -> AppResult<Response> {
    let mcp = super::mcp(&s, &mcp_id)?;
    let stored = s
        .store
        .get_credentials_with_meta(&session.sub, &mcp.id, mcp.primary_field())
        .await?;
    let (values, meta) = stored.unzip();
    let key = key(&session.sub, mcp, meta.as_ref());
    let verdict = {
        let (s, mcp) = (s.clone(), mcp.clone());
        s.questions
            .verdicts
            .clone()
            .answer(key, async move { verify(&s, &mcp, values.as_ref()).await })
            .await
            .unwrap_or(Verified::Unknown)
    };
    let state = match mcp.no_auth || meta.is_some() {
        true => BadgeState::settled(mcp, verdict),
        false => BadgeState::NotConfigured,
    };
    Ok(one(Badge {
        mcp: mcp.id.clone(),
        state,
    }
    .draw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(state: BadgeState) -> String {
        Badge {
            mcp: "caldav".into(),
            state,
        }
        .render()
        .unwrap()
    }

    const SETTLED: [BadgeState; 6] = [
        BadgeState::NotConfigured,
        BadgeState::Connected,
        BadgeState::Rejected { no_auth: false },
        BadgeState::Rejected { no_auth: true },
        BadgeState::Unknown { no_auth: false },
        BadgeState::Unknown { no_auth: true },
    ];

    /// An answer that kept its URL would fetch itself for as long as the tab
    /// is open.
    #[test]
    fn only_a_checking_badge_loads() {
        assert!(badge(BadgeState::Checking).contains(r#"data-url="/dashboard/caldav/status""#));
        for state in SETTLED {
            assert!(
                !badge(state).contains("data-init"),
                "{state:?} loads itself"
            );
        }
    }

    /// A patch aimed at an id nothing carries fails silently.
    #[test]
    fn every_state_carries_the_id_the_panel_places() {
        let id = format!(r#"id="{}""#, Badge::id_of("caldav"));
        for state in SETTLED.into_iter().chain([BadgeState::Checking]) {
            assert!(badge(state).contains(&id), "{state:?}");
        }
    }

    #[test]
    fn a_verdict_reads_as_the_badge_it_should() {
        assert!(badge(BadgeState::Connected).contains("Connected"));
        assert!(badge(BadgeState::Rejected { no_auth: false }).contains("Credentials rejected"));
        // Asked, and the backend could not say: not green, which would be a
        // claim there are no grounds for.
        let unknown = badge(BadgeState::Unknown { no_auth: false });
        assert!(unknown.contains("Configured") && unknown.contains("unknown"));
        assert!(!badge(BadgeState::Checking).contains("Not configured"));
    }

    /// A refusal for an MCP that takes no credentials names the backend, not
    /// the user.
    #[test]
    fn a_no_auth_check_reads_as_the_backend() {
        assert!(badge(BadgeState::Rejected { no_auth: true }).contains("Backend signed out"));
        let unknown = badge(BadgeState::Unknown { no_auth: true });
        assert!(unknown.contains("Status unknown") && !unknown.contains("Connected"));
    }
}
