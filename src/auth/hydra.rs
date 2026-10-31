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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

/// How long a client's registration is trusted before it is read again. A
/// client whose audience is removed keeps working this long at most.
const CLIENT_TTL: Duration = Duration::from_secs(300);
/// How long a refusal is remembered: long enough to spare Hydra a lookup per
/// request from a refused client, short enough that a client given the
/// audience is accepted soon after.
const REFUSAL_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct HydraAdmin {
    base: reqwest::Url,
    /// This gateway's public URL. Hydra serves one issuer for every relying
    /// party, so a live token says nothing about which service it was meant
    /// for; a token is accepted only when its client was registered with this
    /// as an audience.
    audience: String,
    http: reqwest::Client,
    clients: Arc<Mutex<HashMap<String, (bool, Instant)>>>,
}

/// Hydra writes an empty audience as `null` or `[]` depending on how the
/// client was created, so both read as empty.
#[derive(Deserialize)]
struct Client {
    #[serde(default)]
    audience: Option<Vec<String>>,
}

fn ttl(allowed: bool) -> Duration {
    if allowed { CLIENT_TTL } else { REFUSAL_TTL }
}

fn normalise(url: &str) -> &str {
    url.trim_end_matches('/')
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
    /// A token that names no client is refused too: the client is what decides
    /// whether the token was issued for this gateway, and Hydra always names it.
    ///
    /// The refusals are logged, since a live token being refused means a
    /// client is presenting something the issuer should not have given it.
    fn subject(self) -> Option<(String, String)> {
        let sub = self.sub.filter(|_| self.active)?;
        if self.token_use.as_deref() != Some("access_token") {
            tracing::warn!(
                client_id = ?self.client_id,
                token_use = ?self.token_use,
                "refused a live token that is not an access token"
            );
            return None;
        }
        let Some(client_id) = self.client_id else {
            tracing::warn!(subject = %sub, "refused a token that names no client");
            return None;
        };
        if client_id == sub {
            tracing::warn!(client_id = %sub, "refused a token that acts for no person");
            return None;
        }
        Some((sub, client_id))
    }
}

impl HydraAdmin {
    pub fn new(admin_url: &str, audience: &str, http: reqwest::Client) -> Result<Self> {
        let base = reqwest::Url::parse(admin_url).context("HYDRA_ADMIN_URL is not a URL")?;
        if base.cannot_be_a_base() {
            anyhow::bail!("HYDRA_ADMIN_URL cannot take a path");
        }
        Ok(Self {
            base,
            audience: normalise(audience).to_string(),
            http,
            clients: Arc::default(),
        })
    }

    fn url(&self, segments: &[&str]) -> reqwest::Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("checked in new")
            .pop_if_empty()
            .extend(segments);
        url
    }

    /// The subject a token belongs to, or `None` if it is not a live access
    /// token acting for a person through a client registered for this gateway.
    ///
    /// `None` and an error are deliberately different: a token the server says
    /// is inactive is a rejection, and a server that cannot be reached is not —
    /// treating the second as the first would fail every request open or closed
    /// on an outage rather than reporting one.
    pub async fn introspect(&self, token: &str) -> Result<Option<String>> {
        let introspection: Introspection = self
            .http
            .post(self.url(&["admin", "oauth2", "introspect"]))
            .form(&[("token", token)])
            .send()
            .await
            .context("calling Hydra's introspection endpoint")?
            .error_for_status()
            .context("Hydra refused the introspection")?
            .json()
            .await
            .context("Hydra's introspection response was not JSON")?;

        let Some((sub, client_id)) = introspection.subject() else {
            return Ok(None);
        };
        if !self.client_allowed(&client_id).await? {
            tracing::warn!(%client_id, "refused a token issued to a client not registered for this gateway");
            return Ok(None);
        }
        Ok(Some(sub))
    }

    /// Whether a client was registered with this gateway's audience. Answers
    /// are cached, a refusal for less time than an acceptance; a lookup that
    /// fails is an error, not a refusal, for the same reason as in
    /// `introspect`.
    async fn client_allowed(&self, client_id: &str) -> Result<bool> {
        // A URL path drops these segments, which would turn the lookup into a
        // listing. Hydra generates the ids of self-registered clients, so no
        // legitimate client has one.
        if client_id.is_empty() || client_id == "." || client_id == ".." {
            return Ok(false);
        }
        let now = Instant::now();
        if let Some((allowed, at)) = self.clients.lock().unwrap().get(client_id)
            && now.duration_since(*at) < ttl(*allowed)
        {
            return Ok(*allowed);
        }

        let response = self
            .http
            .get(self.url(&["admin", "clients", client_id]))
            .send()
            .await
            .context("reading the client from Hydra")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("reading Hydra's client response")?;
        let allowed = client_answer(status, &body, &self.audience)?;

        let mut clients = self.clients.lock().unwrap();
        clients.retain(|_, (allowed, at)| now.duration_since(*at) < ttl(*allowed));
        clients.insert(client_id.to_string(), (allowed, now));
        Ok(allowed)
    }
}

/// What a client lookup answer means: a client Hydra does not know is
/// refused, any other failure is an error, and a client is accepted when
/// its audience names this gateway.
fn client_answer(status: reqwest::StatusCode, body: &[u8], audience: &str) -> Result<bool> {
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !status.is_success() {
        anyhow::bail!("Hydra answered the client lookup with {status}");
    }
    let client: Client =
        serde_json::from_slice(body).context("Hydra's client response was not a client")?;
    Ok(registered_for(
        client.audience.as_deref().unwrap_or_default(),
        audience,
    ))
}

/// Exact match on the entry the identity provider writes, rather than Hydra's
/// own prefix matching: nothing else writes these, so exact is enough.
fn registered_for(client_audience: &[String], audience: &str) -> bool {
    client_audience.iter().any(|a| normalise(a) == audience)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(json: &str) -> Option<String> {
        serde_json::from_str::<Introspection>(json)
            .unwrap()
            .subject()
            .map(|(sub, _)| sub)
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
    fn a_user_token_without_a_client_id_yields_nothing() {
        assert_eq!(
            subject(r#"{"active":true,"sub":"github:1","token_use":"access_token"}"#),
            None
        );
    }

    #[test]
    fn a_client_lookup_maps_status_and_audience() {
        use reqwest::StatusCode;
        let gateway = "https://mcp.example";
        let ok = |body: &str| client_answer(StatusCode::OK, body.as_bytes(), gateway);
        assert!(ok(r#"{"audience":["https://mcp.example"]}"#).unwrap());
        assert!(!ok(r#"{"audience":[]}"#).unwrap());
        assert!(!ok(r#"{"audience":null}"#).unwrap());
        assert!(!ok(r#"{}"#).unwrap());
        assert!(!ok("[]").unwrap_or(false));
        assert!(!ok(r#"[{"audience":["https://mcp.example"]}]"#).unwrap_or(false));
        assert!(!client_answer(StatusCode::NOT_FOUND, b"", gateway).unwrap());
        assert!(client_answer(StatusCode::INTERNAL_SERVER_ERROR, b"", gateway).is_err());
        assert!(client_answer(StatusCode::FORBIDDEN, b"", gateway).is_err());
    }

    #[test]
    fn a_client_is_registered_for_the_gateway_only_by_an_exact_audience() {
        let gateway = "https://mcp.example";
        assert!(registered_for(&["https://mcp.example/".into()], gateway));
        assert!(registered_for(
            &["https://other.example".into(), "https://mcp.example".into()],
            gateway
        ));
        assert!(!registered_for(&[], gateway));
        assert!(!registered_for(
            &["https://mcp.example.evil".into()],
            gateway
        ));
        assert!(!registered_for(&["https://mcp.example/x".into()], gateway));
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
