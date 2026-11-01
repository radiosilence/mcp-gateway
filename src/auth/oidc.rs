//! The dashboard's authorization-code login against Hydra, as an ordinary
//! relying party. PKCE is used although the client holds a secret, so a leaked
//! authorization code is not enough on its own.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::Config;

/// A fresh PKCE verifier and its S256 challenge.
pub fn pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Where to send the browser to have somebody signed in.
pub fn authorize_url(config: &Config, csrf: &str, challenge: &str) -> String {
    let mut url =
        Url::parse(&format!("{}/oauth2/auth", config.oidc_issuer())).expect("the issuer is a URL");
    url.query_pairs_mut()
        .append_pair("client_id", &config.oidc_client_id)
        .append_pair("redirect_uri", &config.oidc_redirect_uri())
        .append_pair("response_type", "code")
        .append_pair("scope", "openid profile")
        .append_pair("state", csrf)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
}

/// Who the issuer says this is: the subject, and a name to show.
pub struct Identity {
    pub subject: String,
    pub login: String,
}

/// Exchange the code and read the identity out of the ID token. The tokens
/// themselves are discarded.
pub async fn exchange_code(
    config: &Config,
    http: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<Identity> {
    let response = http
        .post(format!("{}/oauth2/token", config.oidc_issuer()))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &config.oidc_redirect_uri()),
            ("client_id", &config.oidc_client_id),
            ("client_secret", &config.oidc_client_secret),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .context("calling the token endpoint")?;

    // Never repeat the body back: it can carry token material.
    if !response.status().is_success() {
        bail!("the token endpoint returned {}", response.status());
    }

    let token: TokenResponse = response
        .json()
        .await
        .context("the token response was not JSON")?;

    // Not signature-checked: it came straight from the issuer over TLS in the
    // back channel, with no untrusted party in the path (OIDC Core §3.1.3.7).
    let id_token = token
        .id_token
        .context("the token response carried no id_token")?;
    let payload = id_token
        .split('.')
        .nth(1)
        .context("the id_token is not a JWT")?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .context("the id_token payload is not base64")?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).context("the id_token payload is not JSON")?;

    let subject = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .context("the id_token carried no sub")?
        .to_string();
    // The provider attaches these at consent; the subject is the fallback.
    let login = claims
        .get("preferred_username")
        .or_else(|| claims.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or(&subject)
        .to_string();

    Ok(Identity { subject, login })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_is_the_sha256_of_the_verifier() {
        let (verifier, challenge) = pkce();
        assert_eq!(
            challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
    }

    #[test]
    fn each_verifier_is_fresh() {
        assert_ne!(pkce().0, pkce().0);
    }
}
