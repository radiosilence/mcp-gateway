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

/// What the gateway can honestly say about a backend's credentials.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Verified {
    /// The backend confirmed them.
    Yes,
    /// The backend said they are wrong. Only ever from an explicit match, never
    /// inferred from an error — a server having a bad day is not a bad password.
    Rejected,
    /// No check declared, or one that could not be completed. The gateway makes
    /// no claim rather than a flattering guess.
    Unknown,
}

/// Ask a backend whether it accepts what we hold.
pub(super) async fn verify(state: &AppState, sub: &str, mcp: &crate::config::Mcp) -> Verified {
    let Some(check) = &mcp.verify else {
        return Verified::Unknown;
    };
    let Ok(Some(credentials)) = state
        .store
        .get_credentials(sub, &mcp.id, mcp.primary_field())
        .await
    else {
        return Verified::Unknown;
    };
    let Ok(data) = crate::backend::graphql(state, mcp, &credentials, &check.query, None).await
    else {
        // Could be a revoked credential, could be the server. Saying which
        // without being told is how a user ends up rotating a working password.
        return Verified::Unknown;
    };

    read_verdict(check, &data)
}

/// Read the answer out of the response. Separated from asking so the part that
/// decides what a backend said can be tested without one.
fn read_verdict(check: &crate::config::Verify, data: &Value) -> Verified {
    let Some(path) = &check.path else {
        // Nothing named to read: the backend raises on bad auth, so having got
        // an answer at all is the answer.
        return Verified::Yes;
    };
    let Some(value) = path.split('.').try_fold(data, |v, key| v.get(key)) else {
        return Verified::Unknown;
    };
    let text = value.as_str();

    // Only an explicit match calls a credential wrong. Everything unrecognised
    // is unknown, because "not the value meaning success" covers a server that
    // is merely unwell.
    if check.rejected.is_some() && text == check.rejected.as_deref() {
        return Verified::Rejected;
    }
    let ok = match &check.ok {
        Some(expected) => text == Some(expected.as_str()),
        None => value
            .as_bool()
            .unwrap_or(!text.unwrap_or_default().is_empty()),
    };
    match ok {
        true => Verified::Yes,
        false => Verified::Unknown,
    }
}

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

    fn check(
        path: Option<&str>,
        ok: Option<&str>,
        rejected: Option<&str>,
    ) -> crate::config::Verify {
        crate::config::Verify {
            query: "{ viewer { status } }".into(),
            path: path.map(str::to_string),
            ok: ok.map(str::to_string),
            rejected: rejected.map(str::to_string),
        }
    }

    /// caldav-cli's shape: a status enum that never raises, where one value
    /// means working, one means the credential is wrong, and the rest mean the
    /// server could not say.
    #[test]
    fn a_status_field_gives_all_three_answers() {
        let c = check(
            Some("viewer.status"),
            Some("CONNECTED"),
            Some("INVALID_CREDENTIALS"),
        );
        let at = |s: &str| read_verdict(&c, &json!({"viewer": {"status": s}}));

        assert!(at("CONNECTED") == Verified::Yes);
        assert!(at("INVALID_CREDENTIALS") == Verified::Rejected);
        // Not a verdict on the credential: the server is unwell, and saying
        // "rejected" here sends someone off to rotate a working password.
        assert!(at("UNREACHABLE") == Verified::Unknown);
        assert!(at("SOMETHING_NEW") == Verified::Unknown);
    }

    /// fastmail-cli's shape: bad auth raises, so reaching a response at all is
    /// the whole answer and there is nothing to read.
    #[test]
    fn no_path_means_answering_at_all_is_the_answer() {
        let verdict = read_verdict(&check(None, None, None), &json!({"session": {}}));
        assert!(verdict == Verified::Yes);
    }

    #[test]
    fn a_truthy_value_passes_when_no_expected_value_is_named() {
        let c = check(Some("session.username"), None, None);
        assert!(read_verdict(&c, &json!({"session": {"username": "jc"}})) == Verified::Yes);
        assert!(read_verdict(&c, &json!({"session": {"username": ""}})) == Verified::Unknown);
        assert!(read_verdict(&c, &json!({"session": {"ok": true}})) == Verified::Unknown);
    }

    /// A backend that changed shape must not read as a rejection.
    #[test]
    fn a_missing_path_is_unknown_not_rejected() {
        let c = check(
            Some("viewer.status"),
            Some("CONNECTED"),
            Some("INVALID_CREDENTIALS"),
        );
        assert!(read_verdict(&c, &json!({})) == Verified::Unknown);
        assert!(read_verdict(&c, &json!({"viewer": {}})) == Verified::Unknown);
    }

    /// Nothing is ever called rejected unless the registry named the value that
    /// means it.
    #[test]
    fn without_a_rejected_value_nothing_is_rejected() {
        let c = check(Some("viewer.status"), Some("CONNECTED"), None);
        assert!(
            read_verdict(&c, &json!({"viewer": {"status": "INVALID_CREDENTIALS"}}))
                == Verified::Unknown
        );
    }

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
