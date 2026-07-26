//! Envelope encryption for stored credentials at rest.
//!
//! We use XChaCha20-Poly1305 (24-byte random nonce, AEAD). The DB stores only
//! `nonce || ciphertext`, base64-encoded. The 32-byte key lives in a k8s Secret
//! and is injected via `TOKEN_ENC_KEY` — a DB dump alone reveals nothing.
//!
//! Key rotation is out of scope for now: rotating the key means re-encrypting
//! every stored token. Flagged for the security review.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, XChaCha20Poly1305, XNonce};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

#[derive(Clone)]
pub struct Cipher {
    inner: XChaCha20Poly1305,
}

impl Cipher {
    pub fn new(key: &[u8]) -> Result<Self> {
        let key = chacha20poly1305::Key::from_slice(key);
        Ok(Self {
            inner: XChaCha20Poly1305::new(key),
        })
    }

    /// Encrypt plaintext → base64(`nonce || ciphertext`).
    pub fn seal(&self, plaintext: &str) -> Result<String> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .inner
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow!("encrypt failed: {e}"))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        Ok(B64.encode(blob))
    }

    /// Decrypt base64(`nonce || ciphertext`) → plaintext.
    pub fn open(&self, sealed: &str) -> Result<String> {
        let blob = B64
            .decode(sealed)
            .context("stored token not valid base64")?;
        anyhow::ensure!(blob.len() > 24, "stored token too short");
        let (nonce_bytes, ciphertext) = blob.split_at(24);
        let nonce = XNonce::from_slice(nonce_bytes);
        let plaintext = self
            .inner
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow!("decrypt failed (wrong key or tampered data): {e}"))?;
        String::from_utf8(plaintext).context("decrypted token not valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn roundtrip() {
        let c = Cipher::new(&key()).unwrap();
        let sealed = c.seal("fmu1-secret-token").unwrap();
        assert_ne!(sealed, "fmu1-secret-token");
        assert_eq!(c.open(&sealed).unwrap(), "fmu1-secret-token");
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let c = Cipher::new(&key()).unwrap();
        assert_ne!(c.seal("x").unwrap(), c.seal("x").unwrap());
    }

    #[test]
    fn wrong_key_fails() {
        let a = Cipher::new(&[1u8; 32]).unwrap();
        let b = Cipher::new(&[2u8; 32]).unwrap();
        let sealed = a.seal("secret").unwrap();
        assert!(b.open(&sealed).is_err());
    }

    #[test]
    fn tamper_fails() {
        let c = Cipher::new(&key()).unwrap();
        let mut sealed = c.seal("secret").unwrap();
        // flip a character in the ciphertext region
        sealed.push('A');
        assert!(c.open(&sealed).is_err());
    }
}
