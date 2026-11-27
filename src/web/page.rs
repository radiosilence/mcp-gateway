//! The pages, one module each, named as the UI names them. Every page has the
//! same shape ([`Page`]): its title, what it reads from the request, and its
//! body. [`serve`] draws any of them: a browser asking for the address gets
//! the layout around the body; the page itself asking (a link or form that
//! loads in place) gets the body alone, morphed into `main`, with the address
//! it now shows.

use std::future::Future;

use askama::Template;
use askama_datastar::{Html, PatchSignals};
use axum::extract::{FromRequestParts, OriginalUri, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use koan_kit_web as kit;

use super::{Draw, assets};
use crate::error::AppResult;
use crate::state::AppState;

pub(super) mod dashboard;
pub(super) mod sign_in;

/// A page: what it is called, what it reads from the request, and how its
/// body is drawn.
pub(super) trait Page: Send + 'static {
    /// The document's title.
    const TITLE: &'static str;
    type Args: FromRequestParts<AppState, Rejection: IntoResponse> + Send;
    fn body(s: &AppState, args: Self::Args) -> impl Future<Output = AppResult<Body>> + Send;
}

/// A page's body as its module draws it, or where the address now leads.
pub(super) enum Body {
    Drawn(Html),
    /// The address shows another page: go there instead.
    Elsewhere(&'static str),
}

impl Body {
    pub(super) fn of(view: &impl Template) -> Self {
        Self::Drawn(view.draw())
    }
}

/// The document every page is drawn in, around the page's own body.
#[derive(Template)]
#[template(path = "layout.html")]
struct Layout {
    title: Html,
    /// The page's starting signals.
    signals: String,
    main: Html,
}

impl Layout {
    fn css(&self) -> String {
        assets::css_url()
    }

    fn version(&self) -> &'static str {
        crate::VERSION
    }

    fn flash(&self) -> kit::Flash<'static> {
        flash_region()
    }
}

/// The document's title, which a page loaded in place replaces.
#[derive(Template)]
#[template(path = "layout/title.html")]
struct Title {
    name: &'static str,
}

/// The page's own region, which a page loaded in place replaces whole.
#[derive(Template)]
#[template(path = "layout/main.html")]
struct Main {
    body: Html,
}

/// Draw page `P`: whole for a browser, or in place for the page itself.
pub(super) async fn serve<P: Page>(
    State(s): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    args: P::Args,
) -> Response {
    let url = askama_datastar::address(&uri);
    let body = match P::body(&s, args).await {
        Ok(Body::Drawn(body)) => body,
        Ok(Body::Elsewhere(to)) if askama_datastar::is_datastar(&headers) => {
            return askama_datastar::events([askama_datastar::go(to).into()]);
        }
        Ok(Body::Elsewhere(to)) => return Redirect::to(to).into_response(),
        Err(e) => return e.into_response(),
    };
    let title = Title { name: P::TITLE }.draw();
    let main = Main { body }.draw();
    let signals = serde_json::json!({ "url": url }).to_string();
    if askama_datastar::is_datastar(&headers) {
        return askama_datastar::events([
            super::patch(Html::from_iter([main, title, flash_region().html()])),
            PatchSignals::new(signals).into(),
        ]);
    }
    match Html::render(&Layout {
        title,
        signals,
        main,
    }) {
        Ok(html) => html.into_response(),
        Err(e) => crate::error::AppError::Internal(e.into()).into_response(),
    }
}

/// The page's flash region, which also says a request that failed and sends a
/// session that has ended to the sign-in.
fn flash_region() -> kit::Flash<'static> {
    kit::Flash::region()
        .failed("Something went wrong. Try again.")
        .unreachable("The gateway could not be reached. Try again.")
        .sign_in("/login")
}
