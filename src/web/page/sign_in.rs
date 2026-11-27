//! The page in front of the dashboard, `/`: what the gateway is, and the way
//! in. Someone already signed in goes straight to the dashboard.

use askama::Template;
use koan_kit_web as kit;

use super::{Body, Page};
use crate::auth::extract::MaybeSession;
use crate::error::AppResult;
use crate::state::AppState;

pub(in crate::web) struct SignIn;

#[derive(Template)]
#[template(path = "sign_in/page.html")]
struct SignInPage;

impl Page for SignIn {
    const TITLE: &'static str = "MCP Gateway";
    type Args = MaybeSession;

    async fn body(_: &AppState, MaybeSession(session): MaybeSession) -> AppResult<Body> {
        Ok(match session {
            Some(_) => Body::Elsewhere("/dashboard"),
            None => Body::of(&SignInPage),
        })
    }
}
