//! Fetch and cache Hydra's JWKS so we can verify JWT access tokens locally
//! (no per-request round-trip to Hydra). Keys are cached by `kid`; an unknown
//! `kid` triggers one refetch (handles Hydra key rotation).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::JwkSet;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct JwksCache {
    jwks_url: String,
    http: reqwest::Client,
    keys: Arc<RwLock<HashMap<String, Arc<DecodingKey>>>>,
}

impl JwksCache {
    pub fn new(hydra_public_url: &str, http: reqwest::Client) -> Self {
        let jwks_url = format!(
            "{}/.well-known/jwks.json",
            hydra_public_url.trim_end_matches('/')
        );
        Self {
            jwks_url,
            http,
            keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return the decoding key for `kid`, refetching once if not cached.
    pub async fn key(&self, kid: &str) -> Result<Arc<DecodingKey>> {
        if let Some(k) = self.keys.read().await.get(kid) {
            return Ok(k.clone());
        }
        self.refresh().await?;
        self.keys
            .read()
            .await
            .get(kid)
            .cloned()
            .with_context(|| format!("no JWKS key for kid {kid} after refresh"))
    }

    async fn refresh(&self) -> Result<()> {
        let set: JwkSet = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .context("fetching JWKS")?
            .error_for_status()
            .context("JWKS endpoint returned error")?
            .json()
            .await
            .context("parsing JWKS")?;

        let mut map = HashMap::new();
        for jwk in &set.keys {
            if let Some(kid) = jwk.common.key_id.clone()
                && let Ok(key) = DecodingKey::from_jwk(jwk)
            {
                map.insert(kid, Arc::new(key));
            }
        }
        *self.keys.write().await = map;
        Ok(())
    }
}
