//! The dashboard, `/dashboard`: a panel per registered MCP, where the person
//! enters its credentials and finds how to add it to Claude.
//!
//! The page is drawn from the gateway's own database alone. What only a
//! backend knows (whether the credentials work, a setting's choices) is a
//! region the page loads once it is up, so a slow backend cannot hold the
//! page.

use askama::Template;
use askama_datastar::Html;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use koan_kit_web as kit;

use super::{Body, Page};
use crate::auth::extract::PageSession;
use crate::config::Mcp;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::TokenMeta;
use crate::web::Draw;

pub(in crate::web) mod badge;
pub(in crate::web) mod panel;
pub(in crate::web) mod setting;

pub(in crate::web) struct Dashboard;

#[derive(Template)]
#[template(path = "dashboard/page.html")]
struct DashboardPage {
    login: String,
    panels: Vec<Html>,
}

impl Page for Dashboard {
    const TITLE: &'static str = "Dashboard · MCP Gateway";
    type Args = PageSession;

    async fn body(s: &AppState, PageSession(session): PageSession) -> AppResult<Body> {
        let mut stored = s
            .store
            .credentials_for(&session.sub, &s.config.mcps)
            .await?;
        let panels = s
            .config
            .mcps
            .iter()
            .map(|m| panel::Panel::of(s, &session.sub, m, stored.remove(&m.id)).draw())
            .collect();
        Ok(Body::of(&DashboardPage {
            login: session.login,
            panels,
        }))
    }
}

/// End this gateway's session and go to the sign-in page. The issuer's own
/// session serves other relying parties, so it stays.
pub(in crate::web) async fn sign_out(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let cookies = crate::auth::routes::end_session(&s, &headers).await;
    let mut response = askama_datastar::events([askama_datastar::go("/").into()]);
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

fn mcp<'a>(s: &'a AppState, id: &str) -> AppResult<&'a Mcp> {
    s.config.mcp(id).ok_or(AppError::NotFound)
}

/// What a backend's answer about stored credentials depends on: when they
/// were last saved, or that there are none.
fn stamp(stored: Option<&TokenMeta>) -> i128 {
    stored.map_or(0, |meta| meta.updated_at.unix_timestamp_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_registry_says_so() {
        let html = DashboardPage {
            login: "somebody".into(),
            panels: Vec::new(),
        }
        .render()
        .unwrap();
        assert!(html.contains("No MCPs configured"), "{html}");
    }

    /// MCP ids may hold `-`, so ids that only joined parts with it collided:
    /// `cal`'s badge was the panel of an MCP called `cal-status`.
    #[test]
    fn no_two_regions_share_an_id() {
        let ids = [
            "cal",
            "cal-status",
            "cal-setting",
            "cal-setting-x",
            "x",
            "cal_x",
        ];
        let mut all = std::collections::HashSet::new();
        for mcp in ids {
            assert!(all.insert(panel::Panel::id_of(mcp)));
            assert!(all.insert(badge::Badge::id_of(mcp)));
            for field in ids {
                assert!(
                    all.insert(setting::Setting::id_of(mcp, field)),
                    "{mcp} {field}"
                );
            }
        }
    }
}
