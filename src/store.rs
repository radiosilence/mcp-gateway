//! Persistence: the `(sub, mcp) → encrypted credential set` mapping.
//!
//! Credentials are encrypted (see [`crate::crypto`]) before they touch a row.
//! Losing the database costs users re-entering their credentials, nothing
//! more.

use std::collections::HashMap;

use anyhow::Result;
use base64::Engine;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

use crate::config::Mcp;
use crate::crypto::{Cipher, row_binding};

/// A resolved dashboard session.
pub struct Session {
    pub sub: String,
    pub login: String,
}

/// Login state held while the browser is at the issuer.
pub struct OAuthFlow {
    pub csrf: String,
    pub verifier: String,
}

fn session_key(id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(id.as_bytes()))
}

fn new_id() -> String {
    let mut b = [0u8; 32];
    rand::fill(&mut b);
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
    /// For `<time datetime>`.
    pub fn updated_at_rfc3339(&self) -> String {
        self.updated_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| self.updated_at.to_string())
    }

    /// How long ago, in words: the server does not know the viewer's timezone,
    /// so a relative time is the one it can render correctly.
    pub fn updated_ago(&self) -> String {
        let seconds = (OffsetDateTime::now_utc() - self.updated_at).whole_seconds();
        let seconds = seconds.max(0);
        let (n, unit) = match seconds {
            0..60 => return "just now".into(),
            60..3_600 => (seconds / 60, "minute"),
            3_600..86_400 => (seconds / 3_600, "hour"),
            86_400..2_592_000 => (seconds / 86_400, "day"),
            _ => (seconds / 2_592_000, "month"),
        };
        match n {
            1 => format!("1 {unit} ago"),
            n => format!("{n} {unit}s ago"),
        }
    }
}

/// A user's values for one MCP, keyed by [`crate::config::CredentialField::id`].
/// Ordered so the encrypted blob is stable across saves of the same values.
pub type CredentialSet = std::collections::BTreeMap<String, String>;

/// Serialise a credential set for encryption.
fn encode_secret(values: &CredentialSet) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a decrypted blob back into a credential set. Rows written before
/// multi-field support hold a bare secret, which decodes onto `primary_field`.
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
            // Fail fast in an outage rather than holding every request.
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(database_url)
            .await?;
        Ok(Self { pool, cipher })
    }

    /// Run embedded migrations (idempotent).
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Fetch and decrypt a user's credential set for one MCP.
    pub async fn get_credentials(
        &self,
        sub: &str,
        mcp_id: &str,
        primary_field: &str,
    ) -> Result<Option<CredentialSet>> {
        Ok(self
            .get_credentials_with_meta(sub, mcp_id, primary_field)
            .await?
            .map(|(values, _)| values))
    }

    /// [`Self::get_credentials`] along with when they were last saved.
    pub async fn get_credentials_with_meta(
        &self,
        sub: &str,
        mcp_id: &str,
        primary_field: &str,
    ) -> Result<Option<(CredentialSet, TokenMeta)>> {
        let row = sqlx::query(
            "SELECT enc_secret, updated_at FROM credentials WHERE sub = $1 AND mcp_id = $2",
        )
        .bind(sub)
        .bind(mcp_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let sealed: String = row.get("enc_secret");
                let plain = self.unseal(sub, mcp_id, &sealed).await?;
                let meta = TokenMeta {
                    updated_at: row.get("updated_at"),
                };
                Ok(Some((decode_secret(&plain, primary_field), meta)))
            }
            None => Ok(None),
        }
    }

    /// Every credential set a user has stored, keyed by MCP id, for the MCPs
    /// still in the registry.
    pub async fn credentials_for(
        &self,
        sub: &str,
        mcps: &[Mcp],
    ) -> Result<HashMap<String, (CredentialSet, TokenMeta)>> {
        let rows =
            sqlx::query("SELECT mcp_id, enc_secret, updated_at FROM credentials WHERE sub = $1")
                .bind(sub)
                .fetch_all(&self.pool)
                .await?;
        let mut stored = HashMap::with_capacity(rows.len());
        for row in rows {
            let mcp_id: String = row.get("mcp_id");
            let Some(mcp) = mcps.iter().find(|m| m.id == mcp_id) else {
                continue;
            };
            let plain = self
                .unseal(sub, &mcp_id, &row.get::<String, _>("enc_secret"))
                .await?;
            let meta = TokenMeta {
                updated_at: row.get("updated_at"),
            };
            stored.insert(mcp_id, (decode_secret(&plain, mcp.primary_field()), meta));
        }
        Ok(stored)
    }

    /// Replace a user's credential set for an MCP with `merge(stored)`, and
    /// return what was stored before alongside what is stored now.
    ///
    /// The row is locked from read to write, since every change rewrites the
    /// whole encrypted set and two concurrent saves would otherwise lose one
    /// side's fields. `merge` refusing leaves the row untouched and hands its
    /// error back.
    pub async fn update_credentials<E>(
        &self,
        sub: &str,
        mcp_id: &str,
        primary_field: &str,
        merge: impl FnOnce(Option<&CredentialSet>) -> std::result::Result<CredentialSet, E>,
    ) -> Result<std::result::Result<(Option<CredentialSet>, CredentialSet), E>> {
        let mut tx = self.pool.begin().await?;
        let stored = sqlx::query(
            "SELECT enc_secret FROM credentials WHERE sub = $1 AND mcp_id = $2 FOR UPDATE",
        )
        .bind(sub)
        .bind(mcp_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| {
            let sealed: String = row.get("enc_secret");
            self.cipher
                .open(&sealed, &row_binding(sub, mcp_id))
                .map(|opened| decode_secret(&opened.plain, primary_field))
        })
        .transpose()?;

        let values = match merge(stored.as_ref()) {
            Ok(values) => values,
            Err(e) => {
                tx.rollback().await?;
                return Ok(Err(e));
            }
        };
        let sealed = self
            .cipher
            .seal(&encode_secret(&values), &row_binding(sub, mcp_id))?;
        sqlx::query(
            "INSERT INTO credentials (sub, mcp_id, enc_secret, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (sub, mcp_id) DO UPDATE SET enc_secret = EXCLUDED.enc_secret, updated_at = now()",
        )
        .bind(sub)
        .bind(mcp_id)
        .bind(&sealed)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok((stored, values)))
    }

    pub async fn delete_credential(&self, sub: &str, mcp_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM credentials WHERE sub = $1 AND mcp_id = $2")
            .bind(sub)
            .bind(mcp_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Open a stored credential, resealing it bound to its row if it predates
    /// binding. Older replicas keep writing unbound blobs during a rollout, so
    /// this is not left to startup alone.
    async fn unseal(&self, sub: &str, mcp_id: &str, sealed: &str) -> Result<String> {
        let binding = row_binding(sub, mcp_id);
        let opened = self.cipher.open(sealed, &binding)?;
        if opened.legacy
            && let Err(e) = self.reseal(sub, mcp_id, sealed, &opened.plain).await
        {
            tracing::warn!(error = format!("{e:#}"), mcp = %mcp_id, "resealing a credential failed");
        }
        Ok(opened.plain)
    }

    /// Replace `old` with `plain` sealed to its row, only if the row still
    /// holds `old`: a save that landed in between wins.
    async fn reseal(&self, sub: &str, mcp_id: &str, old: &str, plain: &str) -> Result<bool> {
        let sealed = self.cipher.seal(plain, &row_binding(sub, mcp_id))?;
        let done = sqlx::query(
            "UPDATE credentials SET enc_secret = $4 WHERE sub = $1 AND mcp_id = $2 AND enc_secret = $3",
        )
        .bind(sub)
        .bind(mcp_id)
        .bind(old)
        .bind(sealed)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Reseal every credential stored before row binding, at startup and
    /// hourly. Neither a row that will not open nor a failed update stops the
    /// gateway.
    pub async fn reseal_legacy(&self) -> Result<()> {
        let rows = sqlx::query(
            "SELECT sub, mcp_id, enc_secret FROM credentials WHERE enc_secret NOT LIKE 'v2.%'",
        )
        .fetch_all(&self.pool)
        .await?;
        let (mut resealed, mut unreadable) = (0, 0);
        for row in rows {
            let (sub, mcp_id, sealed): (String, String, String) =
                (row.get("sub"), row.get("mcp_id"), row.get("enc_secret"));
            let Ok(opened) = self.cipher.open(&sealed, &row_binding(&sub, &mcp_id)) else {
                unreadable += 1;
                continue;
            };
            match self.reseal(&sub, &mcp_id, &sealed, &opened.plain).await {
                Ok(true) => resealed += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    error = format!("{e:#}"),
                    mcp = %mcp_id,
                    "resealing a credential failed"
                ),
            }
        }
        if resealed > 0 {
            tracing::info!(resealed, "bound legacy credentials to their rows");
        }
        if unreadable > 0 {
            tracing::warn!(
                unreadable,
                "stored credentials that will not open with this key"
            );
        }
        Ok(())
    }

    // ---- Dashboard sessions. The cookie carries the id; the table holds only
    // its hash, so reading the table does not yield usable sessions. ----

    pub async fn create_session(&self, sub: &str, login: &str, ttl_secs: i64) -> Result<String> {
        let id = new_id();
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs);
        sqlx::query("INSERT INTO sessions (id, sub, login, expires_at) VALUES ($1, $2, $3, $4)")
            .bind(session_key(&id))
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
                .bind(session_key(id))
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| Session {
            sub: row.get("sub"),
            login: row.get("login"),
        }))
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_key(id))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove sessions and login flows past their expiry; lookups already
    /// ignore them, so this only keeps the tables from growing.
    pub async fn delete_expired(&self) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM oauth_flows WHERE expires_at <= now()")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- Transient OAuth flow state (one-shot) ----

    pub async fn create_oauth_flow(
        &self,
        csrf: &str,
        verifier: &str,
        ttl_secs: i64,
    ) -> Result<String> {
        let id = new_id();
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs);
        sqlx::query(
            "INSERT INTO oauth_flows (id, csrf, verifier, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(csrf)
        .bind(verifier)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Consume an OAuth flow: return it and delete it in one shot (only if unexpired).
    pub async fn take_oauth_flow(&self, id: &str) -> Result<Option<OAuthFlow>> {
        let row = sqlx::query(
            "DELETE FROM oauth_flows WHERE id = $1 AND expires_at > now()
             RETURNING csrf, verifier",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| OAuthFlow {
            csrf: row.get("csrf"),
            verifier: row.get("verifier"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_reads_as_words() {
        let ago = |secs: i64| {
            TokenMeta {
                updated_at: OffsetDateTime::now_utc() - time::Duration::seconds(secs),
            }
            .updated_ago()
        };

        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(3 * 3_600), "3 hours ago");
        assert_eq!(ago(2 * 86_400), "2 days ago");
        assert_eq!(ago(-200), "just now");
    }

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
