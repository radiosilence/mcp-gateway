//! Thin client for Hydra's admin API — the login/consent handshake.
//!
//! Hydra is headless: it drives the OAuth protocol but delegates "log the user
//! in" and "does the user consent" back to us. We accept both via these admin
//! calls after GitHub has vouched for the user. The admin API is
//! cluster-internal and must never be exposed publicly.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HydraAdmin {
    base: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
pub struct ConsentRequest {
    #[serde(default)]
    pub requested_scope: Vec<String>,
    #[serde(default)]
    pub requested_access_token_audience: Vec<String>,
}

#[derive(Deserialize)]
struct RedirectTo {
    redirect_to: String,
}

#[derive(Deserialize)]
struct Introspection {
    active: bool,
    sub: Option<String>,
}

#[derive(Serialize)]
struct AcceptLogin<'a> {
    subject: &'a str,
    remember: bool,
    remember_for: u64,
}

#[derive(Serialize)]
struct AcceptConsent {
    grant_scope: Vec<String>,
    grant_access_token_audience: Vec<String>,
    remember: bool,
    remember_for: u64,
}

impl HydraAdmin {
    pub fn new(admin_url: &str, http: reqwest::Client) -> Self {
        Self {
            base: admin_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Accept a login challenge for `subject`, returning where to send the browser.
    pub async fn accept_login(&self, login_challenge: &str, subject: &str) -> Result<String> {
        let url = format!("{}/admin/oauth2/auth/requests/login/accept", self.base);
        let resp: RedirectTo = self
            .http
            .put(&url)
            .query(&[("login_challenge", login_challenge)])
            .json(&AcceptLogin {
                subject,
                remember: true,
                remember_for: 3600,
            })
            .send()
            .await
            .context("hydra accept_login")?
            .error_for_status()
            .context("hydra accept_login status")?
            .json()
            .await?;
        Ok(resp.redirect_to)
    }

    /// Introspect an opaque access token. Returns the subject if the token is
    /// active. This is how we resolve Claude's bearer token to a user without
    /// JWTs — the token stays an opaque reference and can be revoked at Hydra.
    pub async fn introspect(&self, token: &str) -> Result<Option<String>> {
        let url = format!("{}/admin/oauth2/introspect", self.base);
        let resp: Introspection = self
            .http
            .post(&url)
            .form(&[("token", token)])
            .send()
            .await
            .context("hydra introspect")?
            .error_for_status()
            .context("hydra introspect status")?
            .json()
            .await?;
        Ok(if resp.active { resp.sub } else { None })
    }

    /// Create an OAuth2 client via Hydra's admin API. Used by our DCR proxy
    /// (`/register`): Hydra OSS doesn't serve public DCR, and Claude rejects
    /// Hydra's DCR response shape anyway, so we create the client here and
    /// return a clean response ourselves.
    pub async fn create_client(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}/admin/clients", self.base);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .context("hydra create_client")?
            .error_for_status()
            .context("hydra create_client status")?
            .json()
            .await?;
        Ok(resp)
    }

    pub async fn get_consent(&self, consent_challenge: &str) -> Result<ConsentRequest> {
        let url = format!("{}/admin/oauth2/auth/requests/consent", self.base);
        let resp = self
            .http
            .get(&url)
            .query(&[("consent_challenge", consent_challenge)])
            .send()
            .await
            .context("hydra get_consent")?
            .error_for_status()
            .context("hydra get_consent status")?
            .json()
            .await?;
        Ok(resp)
    }

    /// Auto-grant the requested scopes/audience (single-tenant: no consent UI).
    pub async fn accept_consent(
        &self,
        consent_challenge: &str,
        req: &ConsentRequest,
    ) -> Result<String> {
        let url = format!("{}/admin/oauth2/auth/requests/consent/accept", self.base);
        let resp: RedirectTo = self
            .http
            .put(&url)
            .query(&[("consent_challenge", consent_challenge)])
            .json(&AcceptConsent {
                grant_scope: req.requested_scope.clone(),
                grant_access_token_audience: req.requested_access_token_audience.clone(),
                remember: true,
                remember_for: 3600,
            })
            .send()
            .await
            .context("hydra accept_consent")?
            .error_for_status()
            .context("hydra accept_consent status")?
            .json()
            .await?;
        Ok(resp.redirect_to)
    }
}
