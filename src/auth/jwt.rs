//! Validate Hydra-issued JWT access tokens presented by MCP clients (Claude).

use anyhow::{Context, Result, anyhow};
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde::Deserialize;

use super::jwks::JwksCache;

#[derive(Debug, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    #[allow(dead_code)]
    pub iss: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub scp: Vec<String>,
    #[allow(dead_code)]
    pub exp: usize,
}

/// Verify signature (against Hydra's JWKS), issuer, and expiry. Returns the
/// subject on success.
///
/// SECURITY REVIEW: audience (`aud`) is intentionally not yet enforced — Hydra
/// sets it to the OAuth client, and we haven't pinned a resource identifier.
/// Before this is internet-facing, decide the resource audience and validate it
/// here so a token minted for a different resource can't be replayed.
pub async fn verify(token: &str, issuer: &str, jwks: &JwksCache) -> Result<AccessClaims> {
    let header = decode_header(token).context("malformed JWT header")?;
    let kid = header.kid.ok_or_else(|| anyhow!("JWT missing kid"))?;
    let key = jwks.key(&kid).await?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.validate_exp = true;
    // Not enforcing aud yet — see note above.
    validation.validate_aud = false;

    let data =
        decode::<AccessClaims>(token, &key, &validation).context("JWT verification failed")?;
    Ok(data.claims)
}
