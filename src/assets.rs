//! The page's stylesheet, script and typeface, embedded in the binary and
//! served under fingerprinted addresses, so a browser keeps each for good and
//! a page from a new build fetches the new copy at once.
//!
//! Embedded rather than loaded from a CDN: credentials are typed into these
//! pages, so no third-party script gets DOM access to them.

use std::sync::LazyLock;

use askama_datastar::{Asset, script};
use axum::Router;
use axum::routing::get;

const CSS_PATH: &str = "/assets/app.css";
const JS_PATH: &str = "/assets/app.js";
const FONT_PATH: &str = "/assets/geist-mono.woff2";
const DATASTAR_PATH: &str = "/assets/datastar.js";

/// The stylesheet `mise run web` builds.
const CSS_SOURCE: &str = include_str!("../assets/app.css");
/// `web/app.ts`, bundled (`mise run web`). It imports Datastar as `datastar`,
/// which the bundle leaves to the page.
const JS_SOURCE: &str = include_str!("../assets/app.js");

/// The theme's typeface, which the stylesheet's `@font-face` loads.
static FONT: Asset = Asset::new(include_bytes!("../assets/geist-mono.woff2"), "font/woff2");

/// app.css as served: its `@font-face` names the typeface by its fingerprinted
/// address rather than relative to the stylesheet.
static CSS_BODY: LazyLock<&'static str> = LazyLock::new(|| {
    CSS_SOURCE
        .replace(
            "url(geist-mono.woff2)",
            &format!("url({})", FONT.url(FONT_PATH)),
        )
        .leak()
});

/// app.js as served: its `datastar` import names the address of
/// askama_datastar's copy, so the page runs the one Datastar the crate pins.
static JS_BODY: LazyLock<&'static str> = LazyLock::new(|| {
    JS_SOURCE
        .replace(
            "\"datastar\"",
            &format!("\"{}\"", script::url(DATASTAR_PATH)),
        )
        .leak()
});

static CSS: LazyLock<Asset> =
    LazyLock::new(|| Asset::new(CSS_BODY.as_bytes(), "text/css; charset=utf-8"));

static JS: LazyLock<Asset> =
    LazyLock::new(|| Asset::new(JS_BODY.as_bytes(), "text/javascript; charset=utf-8"));

/// The stylesheet's address, for the page's `<link>`.
pub fn css_url() -> String {
    CSS.url(CSS_PATH)
}

/// The script's address, for the page's `<script>`.
pub fn js_url() -> String {
    JS.url(JS_PATH)
}

/// The routes serving the stylesheet and the scripts.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(CSS_PATH, get(CSS.serve()))
        .route(JS_PATH, get(JS.serve()))
        .route(DATASTAR_PATH, get(script::serve))
}

/// The typeface's route. WOFF2 is already compressed, so it is kept apart from
/// the routes a compression layer covers.
pub fn font<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route(FONT_PATH, get(FONT.serve()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plugin names a script registers, whichever quote the minifier chose.
    fn plugins_in(script: &'static str) -> Vec<&'static str> {
        let names: Vec<&str> = script
            .match_indices("name:")
            .filter_map(|(i, m)| {
                let rest = &script[i + m.len()..];
                let quote = rest
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '"' | '\'' | '`'))?;
                let rest = &rest[1..];
                rest.find(quote).map(|end| &rest[..end])
            })
            .collect();
        assert!(!names.is_empty(), "found no plugin names in the script");
        names
    }

    /// Every plugin the page has: Datastar's own, and the app's.
    fn registered_plugins() -> Vec<&'static str> {
        let mut names = plugins_in(script::SOURCE);
        names.extend(plugins_in(JS_SOURCE));
        names
    }

    /// The generated assets are committed; a truncated build would still
    /// compile and only show up as a broken page.
    #[test]
    fn the_embedded_assets_are_real() {
        assert!(CSS_SOURCE.contains("box-sizing:border-box"), "no preflight");
        assert!(CSS_SOURCE.contains("max-w-2xl"), "utilities missing");
        assert!(CSS_SOURCE.contains("Geist Mono"), "the theme is missing");
        assert!(
            include_bytes!("../assets/geist-mono.woff2").starts_with(b"wOF2"),
            "geist-mono.woff2 is not a WOFF2 font"
        );
        assert!(
            JS_SOURCE.contains("datastar-fetch") && plugins_in(JS_SOURCE).contains(&"copy"),
            "app.js is not the bundle"
        );
        assert!(
            !JS_SOURCE.contains("datastar-prop-change"),
            "app.js bundles a Datastar of its own"
        );
    }

    /// The stylesheet loads the typeface by the address it is cached under.
    #[test]
    fn the_stylesheet_names_the_fingerprinted_typeface() {
        assert!(
            CSS_SOURCE.contains("url(geist-mono.woff2)"),
            "the stylesheet no longer names the typeface this way; the rewrite in CSS_BODY finds nothing"
        );
        assert!(CSS_BODY.contains(&format!("url({})", FONT.url(FONT_PATH))));
        assert!(!CSS_BODY.contains("url(geist-mono.woff2)"));
    }

    /// The page's one Datastar is the crate's: app.js imports it by the address
    /// that copy is served at, not by a name the browser cannot resolve.
    #[test]
    fn app_js_imports_the_datastar_the_page_is_served() {
        assert!(
            JS_SOURCE.contains("\"datastar\""),
            "the bundle no longer imports `datastar`; the rewrite in JS_BODY finds nothing"
        );
        assert!(JS_BODY.contains(&format!("\"{}\"", script::url(DATASTAR_PATH))));
        assert!(!JS_BODY.contains("\"datastar\""));
    }

    /// An attribute naming no Datastar plugin is ignored silently (there is no
    /// `data-on-load`, only `data-init`). Names come from askama_datastar's
    /// copy and the app's bundle, so an upgrade that renames one fails here.
    #[test]
    fn every_datastar_attribute_is_one_datastar_matches() {
        let registered = registered_plugins();
        assert!(
            registered.contains(&"init") && registered.contains(&"on"),
            "the bundle registers neither `init` nor `on`; the scan is wrong, not the templates"
        );

        // Read by expressions; Datastar ignores them.
        const OURS: &[&str] = &["confirm", "url"];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        for entry in std::fs::read_dir(&dir).expect("templates/") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let body = std::fs::read_to_string(&path).expect("readable");
            for chunk in body.split(" data-").skip(1) {
                let attr: String = chunk
                    .chars()
                    .take_while(|c| !matches!(c, '=' | ' ' | '"' | '>' | '\n'))
                    .collect();
                // `data-on:submit__prevent` names the `on` plugin; the key and
                // the modifiers are its arguments.
                let plugin = attr
                    .split(&[':', '_'][..])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    registered.contains(&plugin.as_str()) || OURS.contains(&plugin.as_str()),
                    "{name}: `data-{attr}` names no Datastar plugin — it will be ignored silently"
                );
            }
        }
    }
}
