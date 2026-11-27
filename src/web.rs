//! The web UI: the sign-in page and the dashboard, server-rendered and patched
//! by Datastar.
//!
//! Each page is a module under [`page`] drawn by its own template directory
//! around the shared layout. A page names a few regions by id, and Datastar
//! morphs a region whole when the server sends it again: an action's answer,
//! or a region that loads itself once the page is up. Markup comes from
//! templates and the kit's components alone, so every value in it is escaped
//! exactly once, where a template writes it. ARCHITECTURE.md describes the
//! layout.

use askama::Template;
use askama_datastar::Html;
use axum::Router;
use axum::response::Response;
use axum::response::sse::Event;
use axum::routing::{get, post};
use koan_kit_web as kit;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_http::compression::CompressionLayer;

use crate::state::AppState;

mod asked;
pub mod assets;
#[cfg(test)]
mod escaping;
mod page;

pub use asked::Questions;

/// Every route the browser uses, in one place: the pages, the actions they
/// post to and the regions they load, the sign-in round trip, and the assets.
pub fn routes() -> Router<AppState> {
    use page::dashboard::{self, Dashboard, badge, panel, setting};
    use page::sign_in::SignIn;

    let pages = Router::new()
        .route("/", get(page::serve::<SignIn>))
        .route("/dashboard", get(page::serve::<Dashboard>));
    let actions = Router::new()
        .route("/logout", post(dashboard::sign_out))
        .route(panel::CREDENTIALS, post(panel::save))
        .route(panel::DISCONNECT, post(panel::disconnect))
        .route(badge::STATUS, get(badge::serve))
        .route(setting::SETTING, get(setting::serve).post(setting::save))
        .layer(axum::middleware::from_fn(
            askama_datastar::require_datastar_on_post,
        ));

    // Only the sign-in round trip is rate limited, keyed on the client IP from
    // the ingress's forwarding headers: `/login` writes a row per request.
    let governor = GovernorConfigBuilder::default()
        .per_second(2)
        .burst_size(10)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("valid governor config for the sign-in routes");
    let limiter = governor.limiter().clone();
    tokio::spawn(async move {
        // The keyed limiter keeps an entry per IP until told to drop idle ones.
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            limiter.retain_recent();
        }
    });
    let sign_in = Router::new()
        .route("/login", get(crate::auth::routes::login))
        .route("/auth/callback", get(crate::auth::routes::callback))
        .layer(GovernorLayer::new(governor));

    // The typeface is already compressed, so it stays outside the layer.
    Router::new()
        .merge(pages)
        .merge(actions)
        .merge(assets::routes())
        .layer(CompressionLayer::new())
        .merge(assets::font())
        .merge(sign_in)
}

/// A view rendered as markup, which another template embeds as it is:
/// everything in it was escaped as the view was rendered. A view that fails
/// to render is logged and drawn as nothing.
trait Draw: Template + Sized {
    fn draw(&self) -> Html {
        Html::render(self).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not render a view");
            Html::new()
        })
    }
}

impl<T: Template> Draw for T {}

/// The regions in `html` as a patch, each morphed into the page by its id.
fn patch(html: Html) -> Event {
    html.patch().into()
}

/// One region as the whole answer.
fn one(html: Html) -> Response {
    askama_datastar::events([patch(html)])
}

/// What an action says about itself, over the page.
fn flash(ok: Result<&str, &str>) -> Event {
    let flash = match ok {
        Ok(said) => kit::Flash::ok(said),
        Err(wrong) => kit::Flash::bad(wrong),
    };
    patch(flash.html())
}
