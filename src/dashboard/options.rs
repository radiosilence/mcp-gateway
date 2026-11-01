//! What the dashboard asks backends: whether the stored credentials work, and
//! which choices a setting offers.

use serde_json::{Value, json};

use crate::config::{Mcp, Verify};
use crate::dashboard::{FieldOptionsTemplate, OptionView};
use crate::state::AppState;
use crate::store::CredentialSet;

/// What the gateway can say about a backend's credentials.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Verified {
    /// The backend confirmed them.
    Yes,
    /// The backend said they are wrong. Only from an explicit match, never
    /// inferred from an error.
    Rejected,
    /// No check declared, or one that could not be completed.
    Unknown,
}

/// Ask a backend whether it accepts the stored credentials.
pub(super) async fn verify(
    state: &AppState,
    mcp: &Mcp,
    credentials: Option<&CredentialSet>,
) -> Verified {
    let (Some(check), Some(credentials)) = (&mcp.verify, credentials) else {
        return Verified::Unknown;
    };
    match crate::backend::graphql(state, mcp, credentials, &check.query, None).await {
        Ok(data) => read_verdict(check, &data),
        // A revoked credential or an unwell server: not ours to guess which.
        Err(e) => {
            tracing::info!(error = %e, mcp = %mcp.id, "credential check could not be completed");
            Verified::Unknown
        }
    }
}

fn read_verdict(check: &Verify, data: &Value) -> Verified {
    // No path: the backend raises on bad auth, so any answer is a yes.
    let Some(path) = &check.path else {
        return Verified::Yes;
    };
    let Some(value) = path.split('.').try_fold(data, |v, key| v.get(key)) else {
        return Verified::Unknown;
    };
    let text = value.as_str();

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

pub(super) async fn fetch_choices(
    state: &AppState,
    sub: &str,
    mcp: &Mcp,
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

/// The `<option>` list for a setting: entries the backend marked unusable are
/// dropped and the stored value preselected.
pub(super) fn options_template(
    options: &Value,
    current: &str,
    mcp_id: &str,
    field_id: &str,
) -> FieldOptionsTemplate {
    let entries = options.as_array().map(Vec::as_slice).unwrap_or_default();
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

/// The options a registry query returned, as a flat array: either a list
/// aliased to `options`, or a Relay connection (`nodes` or `edges`) there.
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

    fn check(path: Option<&str>, ok: Option<&str>, rejected: Option<&str>) -> Verify {
        Verify {
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

    #[test]
    fn options_are_filtered_labelled_and_preselected() {
        let data = json!([
            {"value": "work", "label": "Work", "isDefault": true},
            {"value": "ro", "label": "Read only", "disabled": true},
            {"value": "tasks", "label": "Tasks", "supportsEvents": false},
            {"value": "home", "label": "Home"},
        ]);
        let t = options_template(&data, "home", "caldav", "calendar");

        assert_eq!(t.account_default.as_deref(), Some("Work"));

        let offered: Vec<&str> = t.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(offered, ["work", "home"], "read-only and task-only dropped");

        assert!(t.options[0].is_default);
        assert!(!t.options[0].selected);
        assert!(t.options[1].selected, "the stored value is preselected");

        let html = t.render().unwrap();
        assert!(html.contains("— account default"));
        assert!(html.contains(r#"value="home" selected"#));
        assert!(html.contains(r#"action="/dashboard/caldav/field/calendar""#));
        assert!(html.contains(r#"id="options-caldav-calendar""#));
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
