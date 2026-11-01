//! Signed-in-ness as an extractor, so a handler cannot be written without it.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};

use crate::auth::routes::current_session;
use crate::error::AppError;
use crate::state::AppState;
use crate::store::Session;

/// For pages a browser navigates to: not signed in redirects to the login.
pub struct PageSession(pub Session);

impl FromRequestParts<AppState> for PageSession {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        match current_session(state, &parts.headers).await {
            Ok(Some(session)) => Ok(Self(session)),
            Ok(None) => Err(Redirect::to("/login").into_response()),
            Err(e) => Err(e.into_response()),
        }
    }
}

/// For the endpoints Datastar fetches. A redirect would be followed by the
/// fetch, so an expired session answers 401 instead.
///
/// Requests must carry `Datastar-Request`. A cross-site form cannot set a
/// custom header, and SameSite=Lax alone lets sibling subdomains post with the
/// session cookie.
pub struct FragmentSession(pub Session);

impl FromRequestParts<AppState> for FragmentSession {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        if !parts.headers.contains_key("datastar-request") {
            return Err(AppError::Forbidden);
        }
        current_session(state, &parts.headers)
            .await?
            .map(Self)
            .ok_or(AppError::Unauthorized)
    }
}
