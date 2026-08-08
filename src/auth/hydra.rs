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
}

impl HydraAdmin {
    pub fn new(admin_url: &str, http: reqwest::Client) -> Self {
        Self {
            base: admin_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// The subject a token belongs to, or `None` if it is not live.
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

        Ok(introspection.active.then_some(introspection.sub).flatten())
    }
}
