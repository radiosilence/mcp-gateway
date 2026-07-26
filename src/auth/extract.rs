//! Signed-in-ness as an extractor, so a handler cannot be written without it.
//!
//! Every handler used to open by fetching the session and branching on `None`.
//! Four copies of a guard, and the failure mode for a fifth handler that forgot
//! one is an unauthenticated endpoint on a credential vault — not something to
//! leave to remembering.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};

use crate::auth::routes::current_session;
use crate::error::AppError;
use crate::state::AppState;
use crate::store::Session;

/// For handlers a browser navigates to. Not being signed in is answered with
/// the login page, which is what the person actually needs.
pub struct PageSession(pub Session);

impl FromRequestParts<AppState> for PageSession {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match current_session(state, &parts.headers).await {
            Some(session) => Ok(Self(session)),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}

/// For the fragment endpoints htmx calls. A redirect here would be followed and
/// the login page swapped into whatever the request targeted, so an expired
/// session has to arrive as an error instead.
pub struct FragmentSession(pub Session);

impl FromRequestParts<AppState> for FragmentSession {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        current_session(state, &parts.headers)
            .await
            .map(Self)
            .ok_or_else(|| AppError::BadRequest("not signed in".into()))
    }
}
