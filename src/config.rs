//! Runtime configuration, entirely from environment variables.
//!
//! Nothing here is secret-bearing beyond what the process needs to boot; the
//! master encryption key and OAuth client secret arrive via env (k8s Secret in
//! production) and are never logged.

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct Config {
    /// Address to bind the HTTP server to, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// This service's public base URL, e.g. `https://mail.radiosilence.dev`.
    /// Used to build OAuth protected-resource metadata and redirect URIs.
    pub public_url: String,
    /// Postgres connection string.
    pub database_url: String,
    /// 32-byte key for XChaCha20-Poly1305, base64 (standard) encoded.
    pub token_enc_key: Vec<u8>,
    /// Hydra's public issuer URL — what Claude uses to reach the AS, advertised
    /// in the protected-resource metadata, e.g. `https://auth.radiosilence.dev`.
    pub hydra_issuer: String,
    /// Hydra admin API base, cluster-internal only, e.g. `http://hydra-admin:4445`.
    /// Used for token introspection and the login/consent handshake.
    pub hydra_admin_url: String,
    /// GitHub OAuth app credentials (upstream identity).
    pub github_client_id: String,
    pub github_client_secret: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token_enc_key = {
            use base64::Engine;
            let raw = env("TOKEN_ENC_KEY")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .context("TOKEN_ENC_KEY must be base64")?;
            anyhow::ensure!(
                bytes.len() == 32,
                "TOKEN_ENC_KEY must decode to exactly 32 bytes (got {})",
                bytes.len()
            );
            bytes
        };

        Ok(Self {
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            public_url: env("PUBLIC_URL")?,
            database_url: env("DATABASE_URL")?,
            token_enc_key,
            hydra_issuer: env("HYDRA_ISSUER")?,
            hydra_admin_url: env("HYDRA_ADMIN_URL")?,
            github_client_id: env("GITHUB_CLIENT_ID")?,
            github_client_secret: env("GITHUB_CLIENT_SECRET")?,
        })
    }

    /// The GitHub OAuth callback URL registered with the GitHub app.
    pub fn github_redirect_uri(&self) -> String {
        format!(
            "{}/auth/github/callback",
            self.public_url.trim_end_matches('/')
        )
    }
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
