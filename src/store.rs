//! Persistence: the `sub → encrypted Fastmail token` mapping.
//!
//! This is the only genuinely sensitive state the service holds. Tokens are
//! encrypted (see [`crate::crypto`]) before they ever touch a row, so the plain
//! token exists only in memory while a request is in flight. State is
//! deliberately disposable — losing the DB just means users re-paste their
//! token.

use anyhow::Result;
use base64::Engine;
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

use crate::crypto::Cipher;

/// A resolved dashboard session.
pub struct Session {
    pub sub: String,
    pub login: String,
}

/// Transient OAuth flow state carried across the GitHub redirect.
pub struct OAuthFlow {
    pub csrf: String,
    pub login_challenge: Option<String>,
}

fn new_id() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    cipher: Cipher,
}

/// What we know about a stored token without decrypting it — enough for the
/// dashboard to show "a token is set, updated X" without exposing the secret.
pub struct TokenMeta {
    pub updated_at: time::OffsetDateTime,
}

impl Store {
    pub async fn connect(database_url: &str, cipher: Cipher) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await?;
        Ok(Self { pool, cipher })
    }

    /// Run embedded migrations (idempotent).
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Fetch and decrypt a user's credential for a given MCP, if stored.
    pub async fn get_credential(&self, sub: &str, mcp_id: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT enc_secret FROM credentials WHERE sub = $1 AND mcp_id = $2")
            .bind(sub)
            .bind(mcp_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let sealed: String = row.get("enc_secret");
                Ok(Some(self.cipher.open(&sealed)?))
            }
            None => Ok(None),
        }
    }

    /// Encrypt and upsert a user's credential for an MCP.
    pub async fn set_credential(&self, sub: &str, mcp_id: &str, secret: &str) -> Result<()> {
        let sealed = self.cipher.seal(secret)?;
        sqlx::query(
            "INSERT INTO credentials (sub, mcp_id, enc_secret, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (sub, mcp_id) DO UPDATE SET enc_secret = EXCLUDED.enc_secret, updated_at = now()",
        )
        .bind(sub)
        .bind(mcp_id)
        .bind(&sealed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_credential(&self, sub: &str, mcp_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM credentials WHERE sub = $1 AND mcp_id = $2")
            .bind(sub)
            .bind(mcp_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Non-secret metadata for the dashboard (is a credential set, and when).
    pub async fn credential_meta(&self, sub: &str, mcp_id: &str) -> Result<Option<TokenMeta>> {
        let row = sqlx::query("SELECT updated_at FROM credentials WHERE sub = $1 AND mcp_id = $2")
            .bind(sub)
            .bind(mcp_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| TokenMeta {
            updated_at: row.get("updated_at"),
        }))
    }

    // ---- Dashboard sessions (opaque id → server-side state) ----

    pub async fn create_session(&self, sub: &str, login: &str, ttl_secs: i64) -> Result<String> {
        let id = new_id();
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs);
        sqlx::query("INSERT INTO sessions (id, sub, login, expires_at) VALUES ($1, $2, $3, $4)")
            .bind(&id)
            .bind(sub)
            .bind(login)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row =
            sqlx::query("SELECT sub, login FROM sessions WHERE id = $1 AND expires_at > now()")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| Session {
            sub: row.get("sub"),
            login: row.get("login"),
        }))
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- Transient OAuth flow state (one-shot) ----

    pub async fn create_oauth_flow(
        &self,
        csrf: &str,
        login_challenge: Option<&str>,
        ttl_secs: i64,
    ) -> Result<String> {
        let id = new_id();
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs);
        sqlx::query(
            "INSERT INTO oauth_flows (id, csrf, login_challenge, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(csrf)
        .bind(login_challenge)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Consume an OAuth flow: return it and delete it in one shot (only if unexpired).
    pub async fn take_oauth_flow(&self, id: &str) -> Result<Option<OAuthFlow>> {
        let row = sqlx::query(
            "DELETE FROM oauth_flows WHERE id = $1 AND expires_at > now()
             RETURNING csrf, login_challenge",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| OAuthFlow {
            csrf: row.get("csrf"),
            login_challenge: row.get("login_challenge"),
        }))
    }
}
