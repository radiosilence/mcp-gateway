//! Telling a backend what the user picked, for fields that declare a
//! `sync_mutation`, so the backend's own setting agrees with the gateway's.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Mcp;
use crate::state::AppState;
use crate::store::CredentialSet;

/// What a sync mutation answers: a backend that declines a valid write says
/// `{ success: false, error }` rather than raising a GraphQL error.
#[derive(Deserialize)]
struct Reply {
    success: Option<bool>,
    error: Option<String>,
}

/// Run each field's sync mutation for a value that changed from `stored`.
/// Returns what to tell the user when a backend refused; the value is stored
/// either way, since the proxy injects the gateway's copy regardless.
pub async fn sync(
    state: &AppState,
    mcp: &Mcp,
    values: &CredentialSet,
    stored: Option<&CredentialSet>,
) -> Option<String> {
    for field in &mcp.fields {
        let Some(mutation) = &field.sync_mutation else {
            continue;
        };
        let Some(value) = values.get(&field.id).filter(|v| !v.is_empty()) else {
            continue;
        };
        if stored.and_then(|s| s.get(&field.id)) == Some(value) {
            continue;
        }
        let variables = json!({ "value": value });
        let refusal = match super::graphql(state, mcp, values, mutation, Some(variables)).await {
            Err(e) => Some(e.to_string()),
            Ok(data) => refused(&data),
        };
        if let Some(reason) = refusal {
            tracing::debug!(reason, mcp = %mcp.id, field = %field.id, "upstream sync refused");
            return Some(format!(
                "{} is set here, but the backend wouldn't take it: {reason}",
                field.label
            ));
        }
    }
    None
}

/// Why a sync mutation didn't take, if it didn't: the first field of `data`
/// read as a [`Reply`].
fn refused(data: &Value) -> Option<String> {
    let payload = data.as_object()?.values().next()?;
    let reply = Reply::deserialize(payload).ok()?;
    match reply.success {
        Some(false) => Some(
            reply
                .error
                .unwrap_or_else(|| "the backend declined it".into()),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declined_sync_is_read_out_of_the_payload() {
        let data =
            json!({"setDefaultCalendar": {"success": false, "error": "no scheduling inbox"}});
        assert_eq!(refused(&data).as_deref(), Some("no scheduling inbox"));

        let bare = json!({"setDefaultCalendar": {"success": false}});
        assert_eq!(refused(&bare).as_deref(), Some("the backend declined it"));
    }

    #[test]
    fn a_sync_that_took_says_nothing() {
        let data = json!({"setDefaultCalendar": {"success": true, "error": null}});
        assert_eq!(refused(&data), None);
        assert_eq!(refused(&json!({"setDefaultCalendar": {}})), None);
        assert_eq!(refused(&json!({})), None);
    }
}
