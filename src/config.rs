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
    /// HS256 secret for signing our own session / state cookies.
    pub session_secret: String,
    /// Where *this service* reaches Hydra's public endpoints to fetch JWKS —
    /// may be a cluster-internal address, e.g. `http://hydra:4444`.
    pub hydra_public_url: String,
    /// The issuer string Hydra stamps into tokens (`iss`) and that browsers/
    /// Claude use to reach it, e.g. `https://auth.radiosilence.dev`. Equal to
    /// `hydra_public_url` in production; differs only in local docker where the
    /// service and the browser reach Hydra by different hostnames. Used for the
    /// `iss` check and the protected-resource metadata.
    pub hydra_issuer: String,
    /// Hydra admin API base, cluster-internal only, e.g. `http://hydra-admin:4445`.
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

        let hydra_public_url = env("HYDRA_PUBLIC_URL")?;
        let hydra_issuer = env_or("HYDRA_ISSUER", &hydra_public_url);

        Ok(Self {
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            public_url: env("PUBLIC_URL")?,
            database_url: env("DATABASE_URL")?,
            token_enc_key,
            session_secret: env("SESSION_SECRET")?,
            hydra_public_url,
            hydra_issuer,
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
