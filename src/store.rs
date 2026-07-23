//! Persistence: the `sub → encrypted Fastmail token` mapping.
//!
//! This is the only genuinely sensitive state the service holds. Tokens are
//! encrypted (see [`crate::crypto`]) before they ever touch a row, so the plain
//! token exists only in memory while a request is in flight. State is
//! deliberately disposable — losing the DB just means users re-paste their
//! token.

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::crypto::Cipher;

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

    /// Fetch and decrypt the Fastmail token for `sub`, if one is stored.
    pub async fn get_token(&self, sub: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT enc_token FROM fastmail_tokens WHERE sub = $1")
            .bind(sub)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let sealed: String = row.get("enc_token");
                Ok(Some(self.cipher.open(&sealed)?))
            }
            None => Ok(None),
        }
    }

    /// Encrypt and upsert the Fastmail token for `sub`.
    pub async fn set_token(&self, sub: &str, token: &str) -> Result<()> {
        let sealed = self.cipher.seal(token)?;
        sqlx::query(
            "INSERT INTO fastmail_tokens (sub, enc_token, updated_at)
             VALUES ($1, $2, now())
             ON CONFLICT (sub) DO UPDATE SET enc_token = EXCLUDED.enc_token, updated_at = now()",
        )
        .bind(sub)
        .bind(&sealed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_token(&self, sub: &str) -> Result<()> {
        sqlx::query("DELETE FROM fastmail_tokens WHERE sub = $1")
            .bind(sub)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Non-secret metadata for the dashboard.
    pub async fn token_meta(&self, sub: &str) -> Result<Option<TokenMeta>> {
        let row = sqlx::query("SELECT updated_at FROM fastmail_tokens WHERE sub = $1")
            .bind(sub)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| TokenMeta {
            updated_at: row.get("updated_at"),
        }))
    }
}
