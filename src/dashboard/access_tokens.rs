//! The access-token section: list a user's tokens, make one, revoke one.
//!
//! A new token is rendered into the section that the create request answers
//! with, and nowhere else. Nothing stores it, so reloading the page is enough
//! to lose it, which is the point.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;

use super::view::{Notice, render};
use crate::auth::access_token;
use crate::auth::extract::PageSession;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::store::{Session, ago};

const MAX_NAME: usize = 64;

pub(super) struct AccessTokenView {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) scope: String,
    pub(super) created_ago: String,
    pub(super) last_used: String,
}

pub(super) struct McpChoice {
    pub(super) id: String,
    pub(super) name: String,
}

pub(super) struct AccessTokensView {
    pub(super) tokens: Vec<AccessTokenView>,
    pub(super) mcps: Vec<McpChoice>,
    /// The token just created, shown once. Empty on every other render.
    pub(super) revealed: String,
    pub(super) notice: Notice,
}

#[derive(Template)]
#[template(path = "access_tokens.html")]
pub(super) struct AccessTokensTemplate {
    pub(super) access: AccessTokensView,
}

impl AccessTokensView {
    pub(super) async fn build(state: &AppState, session: &Session) -> AppResult<Self> {
        let names = |ids: &[String]| {
            ids.iter()
                .map(|id| {
                    state
                        .config
                        .mcp(id)
                        .map_or(id.as_str(), |m| m.name.as_str())
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let tokens = state
            .store
            .list_access_tokens(&session.sub)
            .await
            .map_err(AppError::Internal)?
            .into_iter()
            .map(|t| AccessTokenView {
                scope: t
                    .mcp_ids
                    .as_deref()
                    .map_or_else(|| "All MCPs".into(), names),
                created_ago: ago(t.created_at),
                last_used: t
                    .last_used_at
                    .map_or_else(|| "never used".into(), |at| format!("used {}", ago(at))),
                id: t.id,
                name: t.name,
            })
            .collect();
        let mcps = state
            .config
            .mcps
            .iter()
            .map(|m| McpChoice {
                id: m.id.clone(),
                name: m.name.clone(),
            })
            .collect();
        Ok(Self {
            tokens,
            mcps,
            revealed: String::new(),
            notice: Notice::none(),
        })
    }
}

async fn section(
    state: &AppState,
    session: &Session,
    revealed: String,
    notice: Notice,
) -> AppResult<Response> {
    let mut access = AccessTokensView::build(state, session).await?;
    access.revealed = revealed;
    access.notice = notice;
    Ok(render(AccessTokensTemplate { access }))
}

/// Parsed by hand because the scope arrives as one `mcp` field per ticked
/// box, and `serde_urlencoded` cannot collect repeated keys into a list.
fn parse_create(body: &[u8]) -> (String, Vec<String>) {
    let mut name = String::new();
    let mut scope = Vec::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        match key.as_ref() {
            "name" => name = value.trim().to_string(),
            "mcp" => scope.push(value.into_owned()),
            _ => {}
        }
    }
    (name, scope)
}

pub async fn create(
    State(state): State<AppState>,
    PageSession(session): PageSession,
    body: Bytes,
) -> AppResult<Response> {
    let (name, scope) = parse_create(&body);
    if name.is_empty() || name.chars().count() > MAX_NAME {
        let complaint = format!("Give the token a name of up to {MAX_NAME} characters.");
        return section(&state, &session, String::new(), Notice::wrong(complaint)).await;
    }
    if let Some(unknown) = scope.iter().find(|id| state.config.mcp(id).is_none()) {
        let complaint = format!("There is no MCP called {unknown}.");
        return section(&state, &session, String::new(), Notice::wrong(complaint)).await;
    }

    let (token, hash) = access_token::generate();
    state
        .store
        .create_access_token(
            &session.sub,
            &name,
            &hash,
            (!scope.is_empty()).then_some(scope.as_slice()),
        )
        .await
        .map_err(AppError::Internal)?;

    let notice =
        Notice::said("Created. Copy it now: it is not stored and will not be shown again.");
    section(&state, &session, token, notice).await
}

pub async fn revoke(
    State(state): State<AppState>,
    PageSession(session): PageSession,
    Path(token_id): Path<String>,
) -> AppResult<Response> {
    state
        .store
        .delete_access_token(&session.sub, &token_id)
        .await
        .map_err(AppError::Internal)?;
    section(&state, &session, String::new(), Notice::said("Revoked")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticked_box_per_mcp_becomes_the_scope() {
        let (name, scope) = parse_create(b"name=+phone+call+&mcp=caldav&mcp=tfl");
        assert_eq!(name, "phone call");
        assert_eq!(scope, ["caldav", "tfl"]);
    }

    #[test]
    fn no_ticked_boxes_is_an_empty_scope() {
        let (_, scope) = parse_create(b"name=cron");
        assert!(scope.is_empty());
    }
}
