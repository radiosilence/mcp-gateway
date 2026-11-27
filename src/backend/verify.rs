//! Whether a backend accepts the credentials stored for it.

use serde_json::Value;

use crate::config::{Mcp, Verify};
use crate::state::AppState;
use crate::store::CredentialSet;

/// What the gateway can say about a backend's credentials.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verified {
    /// The backend confirmed them.
    Yes,
    /// The backend said they are wrong. Only from an explicit match, never
    /// inferred from an error.
    Rejected,
    /// No check declared, or one that could not be completed.
    Unknown,
}

/// Ask a backend whether it accepts the stored credentials. A `no_auth` MCP has
/// none, so it is asked with none: the answer is then whether the backend's
/// own account works.
pub async fn verify(state: &AppState, mcp: &Mcp, credentials: Option<&CredentialSet>) -> Verified {
    let none = CredentialSet::new();
    let credentials = match (credentials, mcp.no_auth) {
        (Some(credentials), _) => credentials,
        (None, true) => &none,
        (None, false) => return Verified::Unknown,
    };
    let Some(check) = &mcp.verify else {
        return Verified::Unknown;
    };
    match super::graphql(state, mcp, credentials, &check.query, None).await {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

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
}
