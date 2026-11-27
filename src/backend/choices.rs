//! The choices a setting offers, read from the backend with the user's own
//! credentials through the field's `options_query`.

use serde::Deserialize;
use serde_json::Value;

use crate::config::Mcp;
use crate::state::AppState;
use crate::store::CredentialSet;

/// One choice as the registry's query aliases it: `{ value label isDefault
/// disabled supportsEvents }`, every key but `value` optional.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub value: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The backend's own default, which an empty setting falls back to.
    #[serde(default)]
    pub is_default: bool,
    /// Offered by the backend but unusable here, such as a read-only calendar.
    #[serde(default)]
    pub disabled: bool,
    /// False for a calendar that holds only tasks, which cannot take events.
    #[serde(default = "offered")]
    pub supports_events: bool,
}

fn offered() -> bool {
    true
}

impl Choice {
    /// What the person reads: the label, or the value when there is none.
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.value)
    }

    /// Whether a setting may be set to it.
    pub fn usable(&self) -> bool {
        !self.disabled && self.supports_events
    }
}

/// Where the registry's query put its list: aliased to `options` as a plain
/// list, or as a Relay connection (`nodes` or `edges { node }`).
#[derive(Deserialize)]
#[serde(untagged)]
enum Options {
    List(Vec<Choice>),
    Nodes { nodes: Vec<Choice> },
    Edges { edges: Vec<Edge> },
}

#[derive(Deserialize)]
struct Edge {
    node: Choice,
}

/// The choices in a query's `data`. Anything else, such as a connection with
/// only a count, is no choices rather than an error.
pub fn read(data: &Value) -> Vec<Choice> {
    let Some(options) = data.get("options") else {
        return Vec::new();
    };
    match Options::deserialize(options) {
        Ok(Options::List(list) | Options::Nodes { nodes: list }) => list,
        Ok(Options::Edges { edges }) => edges.into_iter().map(|e| e.node).collect(),
        Err(_) => Vec::new(),
    }
}

/// Ask `mcp` for a setting's choices with the user's credentials.
pub async fn fetch(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    query: &str,
) -> anyhow::Result<Vec<Choice>> {
    let data = super::graphql(state, mcp, credentials, query, None).await?;
    Ok(read(&data))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn values(data: Value) -> Vec<String> {
        read(&data).into_iter().map(|c| c.value).collect()
    }

    #[test]
    fn takes_a_plain_aliased_list_as_it_comes() {
        assert_eq!(values(json!({"options": [{"value": "a"}]})), ["a"]);
    }

    #[test]
    fn unwraps_a_relay_connection() {
        let nodes = json!({"options": {"totalCount": 1, "nodes": [{"value": "Home"}]}});
        assert_eq!(values(nodes), ["Home"]);
        let edges = json!({"options": {"edges": [{"node": {"value": "Home"}}]}});
        assert_eq!(values(edges), ["Home"]);
    }

    #[test]
    fn anything_else_is_no_choices_rather_than_an_error() {
        assert!(read(&json!({})).is_empty());
        assert!(read(&json!({"options": {"totalCount": 0}})).is_empty());
        assert!(read(&json!({"options": [{"label": "no value"}]})).is_empty());
    }

    #[test]
    fn reads_the_flags_the_registry_aliases() {
        let [work, ro, tasks]: [Choice; 3] = read(&json!({"options": [
            {"value": "work", "label": "Work", "isDefault": true},
            {"value": "ro", "disabled": true},
            {"value": "tasks", "supportsEvents": false},
        ]}))
        .try_into()
        .unwrap();
        assert!(work.is_default && work.usable());
        assert_eq!(work.label(), "Work");
        assert_eq!(
            ro.label(),
            "ro",
            "a choice with no label reads as its value"
        );
        assert!(!ro.usable() && !tasks.usable());
    }
}
