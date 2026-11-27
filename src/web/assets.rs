//! The page's stylesheet and script, embedded in the binary and served under
//! fingerprinted addresses, so a browser keeps each for good and a page from a
//! new build fetches the new copy at once. Datastar is askama_datastar's copy
//! and the typeface the kit's, each served from its crate.
//!
//! Embedded rather than loaded from a CDN: credentials are typed into these
//! pages, so no third-party script gets DOM access to them.

use askama_datastar::{Asset, script};
use axum::Router;
use axum::routing::get;
use koan_kit_web as kit;

const CSS_PATH: &str = "/assets/app.css";
const JS_PATH: &str = "/assets/app.js";
const DATASTAR_PATH: &str = "/assets/datastar.js";

/// The stylesheet `just css` compiles from `styles/` and the kit.
const CSS_SOURCE: &str = include_str!("../../assets/app.css");
/// The page's own script.
const JS_SOURCE: &str = include_str!("../../assets/app.js");

static CSS: Asset = Asset::new(CSS_SOURCE.as_bytes(), "text/css; charset=utf-8");
static JS: Asset = Asset::new(JS_SOURCE.as_bytes(), "text/javascript; charset=utf-8");

/// The stylesheet's address, for the page's head.
pub fn css_url() -> String {
    CSS.url(CSS_PATH)
}

/// The script's address, for the page's head.
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

/// The typeface's route, beside the stylesheet that asks for it. WOFF2 is
/// already compressed, so it is kept apart from the routes a compression
/// layer covers.
pub fn font<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/assets/geist-mono.woff2", get(kit::font::serve))
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

    /// The compiled stylesheet is committed; a truncated build would still
    /// compile and only show up as a broken page.
    #[test]
    fn the_embedded_stylesheet_is_real() {
        assert!(CSS_SOURCE.contains("box-sizing:border-box"), "no preflight");
        assert!(CSS_SOURCE.contains("max-w-2xl"), "utilities missing");
        assert!(CSS_SOURCE.contains("Geist Mono"), "the kit is missing");
    }

    /// The templates under `dir`, at any depth.
    fn templates(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).expect("templates/") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                found.extend(templates(&path));
            } else {
                let body = std::fs::read_to_string(&path).expect("readable");
                found.push((path.display().to_string(), body));
            }
        }
        found
    }

    /// An attribute naming no Datastar plugin is ignored silently (there is no
    /// `data-on-load`, only `data-init`). Names come from askama_datastar's
    /// copy, so an upgrade that renames one fails here.
    #[test]
    fn every_datastar_attribute_is_one_datastar_matches() {
        let registered = plugins_in(script::SOURCE);
        assert!(
            registered.contains(&"init") && registered.contains(&"on"),
            "Datastar registers neither `init` nor `on`; the scan is wrong, not the templates"
        );

        // Read by expressions; Datastar ignores them.
        const OURS: &[&str] = &["url"];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        for (name, body) in templates(&dir) {
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

    /// With eval allowed, escaping is what keeps backend-supplied strings
    /// (option labels, error text) from executing, so no template opts out.
    #[test]
    fn no_template_opts_out_of_escaping() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let mut found = Vec::new();
        for (name, body) in templates(&dir) {
            for line in body.lines() {
                // `safe-area-inset` in a Tailwind class is not the filter.
                if line.contains("|safe") || line.contains("escape(") {
                    found.push(format!("{name}: {}", line.trim()));
                }
            }
        }
        assert_eq!(found, Vec::<String>::new());
    }
}
