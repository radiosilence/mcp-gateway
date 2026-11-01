//! Encryption for stored credentials at rest: XChaCha20-Poly1305 with a random
//! 24-byte nonce, under the 32-byte `TOKEN_ENC_KEY`. A database dump alone
//! reveals nothing.
//!
//! Each blob is bound to the row it belongs to through the associated data, so
//! one user's sealed credentials copied into another's row fail to open. The
//! stored form is `v2.` followed by base64(`nonce || ciphertext`). A blob without
//! the prefix predates the binding; it still opens, reported as legacy so the
//! store can reseal it. Until that path is removed, a legacy blob planted in a
//! row opens there too, so the binding protects fully only once it is gone.
//!
//! Rotating the key would mean re-encrypting every stored credential and is not
//! supported.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Nonce, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;
const V2: &str = "v2.";
const NONCE_LEN: usize = 24;

#[derive(Clone)]
pub struct Cipher {
    inner: XChaCha20Poly1305,
}

/// What a blob is bound to: the credential row's key. The NUL cannot occur in
/// either part, since Postgres `TEXT` refuses it.
pub fn row_binding(sub: &str, mcp_id: &str) -> Vec<u8> {
    [sub.as_bytes(), b"\0", mcp_id.as_bytes()].concat()
}

pub struct Opened {
    pub plain: String,
    /// Sealed before row binding, and due to be resealed.
    pub legacy: bool,
}

impl Cipher {
    pub fn new(key: &[u8]) -> Result<Self> {
        let key = chacha20poly1305::Key::try_from(key)
            .map_err(|_| anyhow!("key must be exactly 32 bytes"))?;
        Ok(Self {
            inner: XChaCha20Poly1305::new(&key),
        })
    }

    pub fn seal(&self, plaintext: &str, binding: &[u8]) -> Result<String> {
        let nonce = Nonce::<XChaCha20Poly1305>::try_generate().context("no system randomness")?;
        let ciphertext = self
            .inner
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: binding,
                },
            )
            .map_err(|e| anyhow!("encrypt failed: {e}"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(format!("{V2}{}", B64.encode(blob)))
    }

    pub fn open(&self, sealed: &str, binding: &[u8]) -> Result<Opened> {
        match sealed.strip_prefix(V2) {
            Some(sealed) => Ok(Opened {
                plain: self.decrypt(sealed, binding)?,
                legacy: false,
            }),
            None => Ok(Opened {
                plain: self.decrypt(sealed, b"")?,
                legacy: true,
            }),
        }
    }

    fn decrypt(&self, sealed: &str, aad: &[u8]) -> Result<String> {
        let blob = B64
            .decode(sealed)
            .context("stored credential not valid base64")?;
        anyhow::ensure!(blob.len() > NONCE_LEN, "stored credential too short");
        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
        let nonce = XNonce::try_from(nonce_bytes).context("stored credential nonce malformed")?;
        let plaintext = self
            .inner
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|e| anyhow!("decrypt failed (wrong key, wrong row or tampered): {e}"))?;
        // Not `.context()`: the error would carry the decrypted bytes with it.
        String::from_utf8(plaintext).map_err(|_| anyhow!("decrypted credential not valid UTF-8"))
    }

    /// A blob sealed without a binding, as rows written before it were.
    #[cfg(test)]
    pub fn seal_legacy(&self, plaintext: &str) -> String {
        let nonce = Nonce::<XChaCha20Poly1305>::try_generate().unwrap();
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&self.inner.encrypt(&nonce, plaintext.as_bytes()).unwrap());
        B64.encode(blob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> Cipher {
        Cipher::new(&[7u8; 32]).unwrap()
    }

    #[test]
    fn roundtrip() {
        let c = cipher();
        let row = row_binding("github:1", "fastmail");
        let sealed = c.seal("fmu1-secret-token", &row).unwrap();
        assert!(!sealed.contains("fmu1"));
        assert!(sealed.starts_with(V2));
        let opened = c.open(&sealed, &row).unwrap();
        assert_eq!(opened.plain, "fmu1-secret-token");
        assert!(!opened.legacy);
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let c = cipher();
        assert_ne!(c.seal("x", b"r").unwrap(), c.seal("x", b"r").unwrap());
    }

    #[test]
    fn wrong_key_fails() {
        let a = Cipher::new(&[1u8; 32]).unwrap();
        let b = Cipher::new(&[2u8; 32]).unwrap();
        let sealed = a.seal("secret", b"r").unwrap();
        assert!(b.open(&sealed, b"r").is_err());
    }

    #[test]
    fn tamper_fails() {
        let c = cipher();
        let mut sealed = c.seal("secret", b"r").unwrap();
        sealed.push('A');
        assert!(c.open(&sealed, b"r").is_err());
    }

    #[test]
    fn a_blob_copied_to_another_row_does_not_open() {
        let c = cipher();
        let sealed = c
            .seal("secret", &row_binding("victim", "fastmail"))
            .unwrap();
        assert!(
            c.open(&sealed, &row_binding("attacker", "fastmail"))
                .is_err()
        );
        assert!(c.open(&sealed, &row_binding("victim", "caldav")).is_err());
    }

    #[test]
    fn the_binding_separates_its_parts() {
        assert_ne!(row_binding("ab", "c"), row_binding("a", "bc"));
        let c = cipher();
        let sealed = c.seal("secret", &row_binding("ab", "c")).unwrap();
        assert!(c.open(&sealed, &row_binding("a", "bc")).is_err());
    }

    #[test]
    fn legacy_blobs_still_open() {
        let c = cipher();
        let sealed = c.seal_legacy("old");
        assert!(!sealed.starts_with(V2));
        let opened = c.open(&sealed, b"anything").unwrap();
        assert_eq!(opened.plain, "old");
        assert!(opened.legacy);
    }
}
