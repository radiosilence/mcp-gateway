//! Persistence: the `(sub, mcp) → encrypted credential set` mapping.
//!
//! This is the only genuinely sensitive state the service holds. Credentials
//! are encrypted (see [`crate::crypto`]) before they ever touch a row, so the
//! plaintext exists only in memory while a request is in flight. State is
//! deliberately disposable — losing the DB just means users re-enter their
//! credentials.

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

impl TokenMeta {
    /// RFC 3339 so the dashboard can drop it straight into a `<time
    /// datetime>` and let the browser localise it; `OffsetDateTime`'s
    /// `Display` is a debug-ish format not fit for showing to a user.
    pub fn updated_at_rfc3339(&self) -> String {
        self.updated_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| self.updated_at.to_string())
    }
}

/// A user's values for one MCP, keyed by [`crate::config::CredentialField::id`].
/// Ordered so the encrypted blob is stable across saves of the same values.
pub type CredentialSet = std::collections::BTreeMap<String, String>;

/// Serialise a credential set for encryption. Always JSON, so a set of any
/// size round-trips through the single `enc_secret` column — no schema change
/// was needed to go from one secret per MCP to several.
fn encode_secret(values: &CredentialSet) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a decrypted blob back into a credential set.
///
/// Rows written before multi-field support hold a bare secret rather than a
/// JSON object; those decode onto `primary_field`, so existing Fastmail tokens
/// keep working untouched. A stored value that happens to be a JSON object of
/// strings would be misread, which no API token in practice is.
fn decode_secret(plain: &str, primary_field: &str) -> CredentialSet {
    match serde_json::from_str::<CredentialSet>(plain) {
        Ok(values) => values,
        Err(_) => CredentialSet::from([(primary_field.to_string(), plain.to_string())]),
    }
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

    /// Fetch and decrypt a user's credential set for a given MCP.
    ///
    /// `primary_field` names the field a legacy single-secret row maps onto —
    /// see [`decode_secret`].
    pub async fn get_credentials(
        &self,
        sub: &str,
        mcp_id: &str,
        primary_field: &str,
    ) -> Result<Option<CredentialSet>> {
        let row = sqlx::query("SELECT enc_secret FROM credentials WHERE sub = $1 AND mcp_id = $2")
            .bind(sub)
            .bind(mcp_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => {
                let sealed: String = row.get("enc_secret");
                let plain = self.cipher.open(&sealed)?;
                Ok(Some(decode_secret(&plain, primary_field)))
            }
            None => Ok(None),
        }
    }

    /// Encrypt and upsert a user's credential set for an MCP.
    pub async fn set_credentials(
        &self,
        sub: &str,
        mcp_id: &str,
        values: &CredentialSet,
    ) -> Result<()> {
        let secret = encode_secret(values);
        let sealed = self.cipher.seal(&secret)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_sets_round_trip() {
        let values = CredentialSet::from([
            ("username".to_string(), "me@icloud.com".to_string()),
            ("password".to_string(), "abcd-efgh".to_string()),
        ]);
        assert_eq!(decode_secret(&encode_secret(&values), "username"), values);
    }

    #[test]
    fn legacy_bare_secrets_decode_onto_the_primary_field() {
        let decoded = decode_secret("fmu1-abc123", "token");
        assert_eq!(
            decoded.get("token").map(String::as_str),
            Some("fmu1-abc123")
        );
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn legacy_secrets_that_look_like_json_scalars_still_decode_as_secrets() {
        // Only a JSON *object of strings* is treated as a credential set.
        for raw in ["12345", "[\"a\"]", "\"quoted\"", "{\"a\": 1}"] {
            let decoded = decode_secret(raw, "token");
            assert_eq!(decoded.get("token").map(String::as_str), Some(raw));
        }
    }

    #[test]
    fn encoding_is_stable_for_the_same_values() {
        let a = CredentialSet::from([
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]);
        let b = CredentialSet::from([
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        assert_eq!(encode_secret(&a), encode_secret(&b));
    }

    #[test]
    fn empty_sets_round_trip_to_empty() {
        assert!(decode_secret(&encode_secret(&CredentialSet::new()), "token").is_empty());
    }
}
