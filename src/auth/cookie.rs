//! Stateless signed cookies (HS256 over `SESSION_SECRET`).
//!
//! No server-side session store, so any pod can validate any cookie — correct
//! for the many-replica deployment. Two cookies:
//! - `fmmcp_session`: dashboard login (`sub`).
//! - `fmmcp_oauth`: short-lived CSRF/state across the GitHub redirect.

use anyhow::{Context, Result};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

pub const SESSION_COOKIE: &str = "fmmcp_session";
pub const OAUTH_COOKIE: &str = "fmmcp_oauth";

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionClaims {
    pub sub: String,
    /// GitHub login, for a friendly dashboard greeting (not security-bearing).
    #[serde(default)]
    pub login: String,
    pub exp: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OAuthStateClaims {
    /// Random CSRF value, must match the `state` GitHub echoes back.
    pub csrf: String,
    /// Present when this GitHub round-trip is servicing a Hydra login flow.
    pub login_challenge: Option<String>,
    pub exp: usize,
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub fn sign<T: Serialize>(secret: &str, claims: &T) -> Result<String> {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .context("signing cookie")
}

pub fn verify<T: for<'de> Deserialize<'de>>(secret: &str, token: &str) -> Result<T> {
    let mut v = Validation::new(Algorithm::HS256);
    v.validate_exp = true;
    v.required_spec_claims.clear();
    v.required_spec_claims.insert("exp".to_string());
    let data = decode::<T>(token, &DecodingKey::from_secret(secret.as_bytes()), &v)
        .context("verifying cookie")?;
    Ok(data.claims)
}

pub fn session_exp(ttl_hours: i64) -> usize {
    (now() + ttl_hours * 3600) as usize
}

pub fn short_exp(ttl_secs: i64) -> usize {
    (now() + ttl_secs) as usize
}

/// Build a hardened `Set-Cookie` value.
pub fn set_cookie(name: &str, value: &str, max_age_secs: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}")
}

/// Build a `Set-Cookie` that clears the named cookie.
pub fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
}

/// Pull a cookie value out of a raw `Cookie` header.
pub fn read_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == name).then(|| v.trim())
    })
}
