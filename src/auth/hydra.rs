//! Thin client for Hydra's admin API.
//!
//! One call is left: introspecting the bearer token an MCP client presents.
//! Access tokens are opaque by design, so the only way to learn whether one is
//! live and whose it is, is to ask the authorization server.
//!
//! This used to also accept login and consent challenges, back when this
//! gateway was the login provider for every client of that Hydra. That moved to
//! its own service, and with it the reason for this file to know anything about
//! how a person signs in.
//!
//! The admin API is cluster-internal and must never be exposed publicly: it
//! answers for any token without authenticating the caller.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone)]
pub struct HydraAdmin {
    base: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Introspection {
    active: bool,
    sub: Option<String>,
    client_id: Option<String>,
    token_use: Option<String>,
}

impl Introspection {
    /// The person a live access token acts for.
    ///
    /// Hydra's introspection answers for refresh tokens as well, so `token_use`
    /// must say `access_token`: a refresh token lives for weeks, and one
    /// presented as a bearer is never exchanged, so Hydra's rotation and reuse
    /// detection never see it.
    ///
    /// A `client_credentials` token is issued to a client acting as itself, so
    /// Hydra sets its subject to the client's own ID. Such a token stands for
    /// no person and has no credentials behind it, and every client
    /// registration is assumed hostile, so it is refused here rather than
    /// relying on the issuer never granting that flow.
    ///
    /// Both refusals are logged, since a live token being refused means a
    /// client is presenting something the issuer should not have given it.
    fn subject(self) -> Option<String> {
        let sub = self.sub.filter(|_| self.active)?;
        if self.token_use.as_deref() != Some("access_token") {
            tracing::warn!(
                client_id = ?self.client_id,
                token_use = ?self.token_use,
                "refused a live token that is not an access token"
            );
            return None;
        }
        if self.client_id.as_deref() == Some(sub.as_str()) {
            tracing::warn!(client_id = %sub, "refused a token that acts for no person");
            return None;
        }
        Some(sub)
    }
}

impl HydraAdmin {
    pub fn new(admin_url: &str, http: reqwest::Client) -> Self {
        Self {
            base: admin_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// The subject a token belongs to, or `None` if it is not a live access
    /// token acting for a person.
    ///
    /// `None` and an error are deliberately different: a token the server says
    /// is inactive is a rejection, and a server that cannot be reached is not —
    /// treating the second as the first would fail every request open or closed
    /// on an outage rather than reporting one.
    pub async fn introspect(&self, token: &str) -> Result<Option<String>> {
        let introspection: Introspection = self
            .http
            .post(format!("{}/admin/oauth2/introspect", self.base))
            .form(&[("token", token)])
            .send()
            .await
            .context("calling Hydra's introspection endpoint")?
            .error_for_status()
            .context("Hydra refused the introspection")?
            .json()
            .await
            .context("Hydra's introspection response was not JSON")?;

        Ok(introspection.subject())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(json: &str) -> Option<String> {
        serde_json::from_str::<Introspection>(json)
            .unwrap()
            .subject()
    }

    #[test]
    fn a_live_user_token_yields_its_subject() {
        assert_eq!(
            subject(
                r#"{"active":true,"sub":"github:1","client_id":"claude","token_use":"access_token"}"#
            )
            .as_deref(),
            Some("github:1")
        );
    }

    #[test]
    fn a_user_token_without_a_client_id_yields_its_subject() {
        assert_eq!(
            subject(r#"{"active":true,"sub":"github:1","token_use":"access_token"}"#).as_deref(),
            Some("github:1")
        );
    }

    #[test]
    fn an_inactive_token_yields_nothing() {
        assert_eq!(
            subject(
                r#"{"active":false,"sub":"github:1","client_id":"claude","token_use":"access_token"}"#
            ),
            None
        );
        assert_eq!(subject(r#"{"active":false}"#), None);
    }

    #[test]
    fn a_live_token_without_a_subject_yields_nothing() {
        assert_eq!(
            subject(r#"{"active":true,"client_id":"claude","token_use":"access_token"}"#),
            None
        );
    }

    #[test]
    fn a_client_credentials_token_yields_nothing() {
        assert_eq!(
            subject(
                r#"{"active":true,"sub":"claude","client_id":"claude","token_use":"access_token"}"#
            ),
            None
        );
    }

    #[test]
    fn a_refresh_token_yields_nothing() {
        assert_eq!(
            subject(
                r#"{"active":true,"sub":"github:1","client_id":"claude","token_use":"refresh_token"}"#
            ),
            None
        );
    }

    #[test]
    fn a_token_that_does_not_say_what_it_is_yields_nothing() {
        assert_eq!(
            subject(r#"{"active":true,"sub":"github:1","client_id":"claude"}"#),
            None
        );
    }
}
