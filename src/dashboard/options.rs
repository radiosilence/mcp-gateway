//! The choices a setting offers, which live in a backend rather than here.
//!
//! Fetched, filtered and shaped in one place: the page prefetches them, the
//! htmx endpoint serves whatever missed that, and both render the same control.

use std::collections::HashMap;

use axum::response::Response;
use serde_json::{Value, json};

/// How long the page will wait for every connected setting's choices before
/// giving up on the stragglers.
///
/// They come from the backends, not from us, so this is a network call to
/// somebody else's calendar server on the way to rendering. Worth doing —
/// arriving complete beats a control that fills in afterwards — but not worth
/// the page for. Whatever misses the budget falls back to loading over htmx,
/// which is also what happens when a backend is refusing outright, so the slow
/// path is the same path and gets exercised.
const OPTIONS_BUDGET: std::time::Duration = std::time::Duration::from_millis(600);

use crate::dashboard::{FieldOptionsTemplate, OptionView, render};
use crate::state::AppState;

/// One setting's choices, and which of them is stored.
pub(super) struct Choices {
    pub(super) options: Value,
    pub(super) current: String,
}

/// Ask one backend, with the user's own credentials. Shared so the page's
/// prefetch and the endpoint htmx calls cannot ask a different question or read
/// the answer differently — they render the same control, so they had better.
pub(super) async fn fetch_choices(
    state: &AppState,
    sub: &str,
    mcp: &crate::config::Mcp,
    field_id: &str,
    query: &str,
) -> anyhow::Result<Choices> {
    let credentials = state
        .store
        .get_credentials(sub, &mcp.id, mcp.primary_field())
        .await?
        .ok_or_else(|| anyhow::anyhow!("no credentials stored"))?;
    let data = crate::backend::graphql(state, mcp, &credentials, query, None).await?;
    Ok(Choices {
        current: credentials.get(field_id).cloned().unwrap_or_default(),
        options: options_of(&data),
    })
}

/// Every connected setting's choices, fetched concurrently, keyed by MCP and
/// field. Missing simply means it did not arrive in time.
pub(super) async fn prefetch_options(
    state: &AppState,
    sub: &str,
    only: Option<&str>,
) -> HashMap<(String, String), (Value, String)> {
    let mut tasks = tokio::task::JoinSet::new();
    for m in state
        .config
        .mcps
        .iter()
        .filter(|m| only.is_none_or(|id| id == m.id))
    {
        for f in &m.fields {
            let Some(query) = f.options_query.clone() else {
                continue;
            };
            let (state, sub) = (state.clone(), sub.to_string());
            let (mcp_id, field_id) = (m.id.clone(), f.id.clone());
            tasks.spawn(async move {
                let started = std::time::Instant::now();
                let mcp = state.config.mcp(&mcp_id)?;
                let choices = fetch_choices(&state, &sub, mcp, &field_id, &query)
                    .await
                    .ok()?;
                // Only the ones that beat the budget report from here; the rest
                // are abandoned, and the endpoint that then serves them times
                // them instead. Between the two, every fetch is accounted for.
                tracing::debug!(
                    mcp = %mcp_id, field = %field_id, ms = started.elapsed().as_millis(),
                    "prefetched options"
                );
                Some(((mcp_id, field_id), (choices.options, choices.current)))
            });
        }
    }

    let wanted = tasks.len();
    let deadline = tokio::time::Instant::now() + OPTIONS_BUDGET;
    let mut out = HashMap::new();
    while let Ok(Some(finished)) = tokio::time::timeout_at(deadline, tasks.join_next()).await {
        if let Ok(Some((key, value))) = finished {
            out.insert(key, value);
        }
    }

    // Worth saying out loud rather than leaving as a silently emptier page: it
    // is the signal that the budget wants revisiting, or that a backend does.
    if out.len() < wanted {
        tracing::info!(
            missed = wanted - out.len(),
            of = wanted,
            budget_ms = OPTIONS_BUDGET.as_millis(),
            "some settings missed the render budget and will load over htmx"
        );
    }
    out
}

/// The `<option>` list for a setting, with the entries the backend marked
/// unusable dropped and the stored value preselected.
pub(super) fn options_fragment(
    options: &Value,
    current: &str,
    mcp_id: &str,
    field_id: &str,
) -> Response {
    render(options_template(options, current, mcp_id, field_id))
}

pub(super) fn options_template(
    options: &Value,
    current: &str,
    mcp_id: &str,
    field_id: &str,
) -> FieldOptionsTemplate {
    let entries = options.as_array().cloned().unwrap_or_default();
    let account_default = entries
        .iter()
        .find(|o| o["isDefault"].as_bool().unwrap_or(false))
        .and_then(|o| o["label"].as_str())
        .map(str::to_string);

    let options = entries
        .iter()
        .filter(|o| {
            !o["disabled"].as_bool().unwrap_or(false)
                && o["supportsEvents"].as_bool().unwrap_or(true)
        })
        .map(|o| {
            let value = o["value"].as_str().unwrap_or_default().to_string();
            let label = o["label"].as_str().unwrap_or_default().to_string();
            OptionView {
                selected: !current.is_empty() && (value == current || label == current),
                is_default: o["isDefault"].as_bool().unwrap_or(false),
                value,
                label,
            }
        })
        .collect();

    FieldOptionsTemplate {
        mcp_id: mcp_id.to_string(),
        field_id: field_id.to_string(),
        account_default,
        options,
    }
}

/// The options a registry query returned, as a flat array.
///
/// A query may alias a plain list straight to `options`, or land on a Relay
/// connection — `caldav-cli` returns one for every collection — in which case
/// the rows are a level down. Both are unwrapped here so the registry stays a
/// query and nothing else.
pub(super) fn options_of(data: &Value) -> Value {
    let options = match data.get("options") {
        Some(options) => options,
        None => return json!([]),
    };
    if options.is_array() {
        return options.clone();
    }
    if let Some(nodes) = options.get("nodes").filter(|n| n.is_array()) {
        return nodes.clone();
    }
    if let Some(edges) = options.get("edges").and_then(Value::as_array) {
        return edges
            .iter()
            .filter_map(|e| e.get("node").cloned())
            .collect();
    }
    json!([])
}

#[cfg(test)]
mod tests {
    use askama::Template;

    use super::*;

    /// The shaping that used to live in the browser: which entries are offered
    /// at all, which is marked as the account's own, and which is preselected.
    #[test]
    fn options_are_filtered_labelled_and_preselected() {
        let data = json!([
            {"value": "work", "label": "Work", "isDefault": true},
            {"value": "ro", "label": "Read only", "disabled": true},
            {"value": "tasks", "label": "Tasks", "supportsEvents": false},
            {"value": "home", "label": "Home"},
        ]);
        let t = options_template(&data, "home", "caldav", "calendar");

        // The placeholder names the backend's own default so the empty choice
        // says what picking it will actually do.
        assert_eq!(t.account_default.as_deref(), Some("Work"));

        let offered: Vec<&str> = t.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(offered, ["work", "home"], "read-only and task-only dropped");

        assert!(t.options[0].is_default);
        assert!(!t.options[0].selected);
        assert!(t.options[1].selected, "the stored value is preselected");

        // The fragment replaces the skeleton outright, so it carries the
        // control and must address the right endpoint itself.
        let html = t.render().unwrap();
        assert!(html.contains("— account default"));
        assert!(html.contains(r#"value="home" selected"#));
        assert!(html.contains(r#"hx-patch="/dashboard/caldav/field/calendar""#));
    }

    /// A stored value that no longer exists upstream must not silently select
    /// something else.
    #[test]
    fn nothing_is_preselected_when_the_stored_value_is_gone() {
        let data = json!([{"value": "home", "label": "Home"}]);
        let t = options_template(&data, "deleted-calendar", "caldav", "calendar");
        assert!(t.options.iter().all(|o| !o.selected));
    }

    #[test]
    fn takes_a_plain_aliased_list_as_it_comes() {
        let data = json!({"options": [{"value": "a"}]});
        assert_eq!(options_of(&data), json!([{"value": "a"}]));
    }

    #[test]
    fn unwraps_a_relay_connection() {
        let data = json!({"options": {"totalCount": 1, "nodes": [{"value": "Home"}]}});
        assert_eq!(options_of(&data), json!([{"value": "Home"}]));

        let edges = json!({"options": {"edges": [{"node": {"value": "Home"}}]}});
        assert_eq!(options_of(&edges), json!([{"value": "Home"}]));
    }

    #[test]
    fn anything_else_is_no_options_rather_than_an_error() {
        assert_eq!(options_of(&json!({})), json!([]));
        assert_eq!(
            options_of(&json!({"options": {"totalCount": 0}})),
            json!([])
        );
    }
}
