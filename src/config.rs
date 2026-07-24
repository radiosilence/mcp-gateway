//! Runtime configuration, entirely from environment variables.
//!
//! Nothing here is secret-bearing beyond what the process needs to boot; the
//! master encryption key and OAuth client secret arrive via env (k8s Secret in
//! production) and are never logged.

use anyhow::{Context, Result};
use serde::Deserialize;

/// A backend MCP the gateway fronts. Registry is loaded from a JSON file
/// (`MCP_REGISTRY`, default `mcps.json`), so adding an MCP is config, not code.
#[derive(Clone, Debug, Deserialize)]
pub struct Mcp {
    /// URL slug + storage key, e.g. "fastmail" → routed at `/mcp/fastmail`.
    pub id: String,
    /// Human name for the dashboard.
    pub name: String,
    /// Backend MCP endpoint to proxy to, e.g. `http://fastmail-mcp:8080/mcp`.
    pub backend: String,
    /// Header the backend expects the per-user secret in, e.g. `X-Fastmail-Token`.
    pub credential_header: String,
    /// Optional link shown in the dashboard for where to get the key.
    #[serde(default)]
    pub key_help_url: Option<String>,
    /// Optional hint text for the key input.
    #[serde(default)]
    pub key_hint: Option<String>,
}

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
    /// Allowlist of GitHub logins (lowercased) permitted to authenticate. Empty
    /// = allow anyone (dev only). On a public deployment this MUST be set, or
    /// anyone with a GitHub account can log in and store their own credentials.
    pub github_allowlist: Vec<String>,
    /// Registry of backend MCPs this gateway fronts.
    pub mcps: Vec<Mcp>,
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
            github_client_id: env("GH_CLIENT_ID")?,
            github_client_secret: env("GH_CLIENT_SECRET")?,
            github_allowlist: env_or("GH_ALLOWED", "")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            mcps: load_registry(&env_or("MCP_REGISTRY", "mcps.json"))?,
        })
    }

    /// Whether a GitHub login may authenticate. Empty allowlist ⇒ anyone (dev).
    pub fn github_login_allowed(&self, login: &str) -> bool {
        self.github_allowlist.is_empty()
            || self.github_allowlist.contains(&login.to_lowercase())
    }

    /// The GitHub OAuth callback URL registered with the GitHub app.
    pub fn github_redirect_uri(&self) -> String {
        format!(
            "{}/auth/github/callback",
            self.public_url.trim_end_matches('/')
        )
    }

    pub fn mcp(&self, id: &str) -> Option<&Mcp> {
        self.mcps.iter().find(|m| m.id == id)
    }
}

/// Path segments the gateway serves itself — an MCP id here would shadow a
/// gateway route (MCPs are mounted at `/{id}`).
const RESERVED_IDS: &[&str] = &[
    "register",
    "login",
    "logout",
    "dashboard",
    "healthz",
    "auth",
    ".well-known",
];

fn load_registry(path: &str) -> Result<Vec<Mcp>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading MCP registry at {path}"))?;
    let mcps: Vec<Mcp> = serde_json::from_str(&raw)
        .with_context(|| format!("parsing MCP registry at {path}"))?;
    for m in &mcps {
        anyhow::ensure!(
            !m.id.is_empty() && !m.id.contains('/'),
            "invalid MCP id {:?}: must be a non-empty single path segment",
            m.id
        );
        anyhow::ensure!(
            !RESERVED_IDS.contains(&m.id.as_str()),
            "MCP id {:?} is reserved (shadows a gateway route)",
            m.id
        );
    }
    Ok(mcps)
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
