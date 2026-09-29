//! Personal access tokens: long-lived bearers for callers that cannot run an
//! OAuth flow, such as a scheduled job or a voice agent that sends a fixed
//! header on every request.
//!
//! A token authenticates as the user who created it, so the proxy looks up the
//! same credentials it would for that user's OAuth token. The prefix is what
//! tells the two apart: a token carrying it is resolved against our own table
//! and never sent to Hydra, and one without it never touches the table.
//!
//! Tokens are 256 bits of randomness, so an unsalted SHA-256 is enough to make
//! a leaked table useless; a slow password hash would buy nothing and cost a
//! hash per proxied request.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub const PREFIX: &str = "mgw_";

/// A fresh token and the hash to store for it.
pub fn generate() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = hash(&token);
    (token, hash)
}

pub fn hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub fn is_access_token(bearer: &str) -> bool {
    bearer.starts_with(PREFIX)
}

/// Whether a token scoped to `scope` may reach `mcp_id`. No scope means every
/// MCP, including ones registered after the token was made.
pub fn permits(scope: Option<&[String]>, mcp_id: &str) -> bool {
    scope.is_none_or(|ids| ids.iter().any(|id| id == mcp_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_carry_the_prefix_and_match_their_hash() {
        let (token, stored) = generate();
        assert!(is_access_token(&token));
        assert_eq!(hash(&token), stored);
        assert_ne!(generate().0, token);
    }

    #[test]
    fn hydra_tokens_are_not_mistaken_for_access_tokens() {
        assert!(!is_access_token("ory_at_abc.def"));
    }

    #[test]
    fn an_unscoped_token_reaches_every_mcp() {
        assert!(permits(None, "fastmail"));
    }

    #[test]
    fn a_scoped_token_reaches_only_its_mcps() {
        let scope = vec!["caldav".to_string(), "tfl".to_string()];
        assert!(permits(Some(&scope), "tfl"));
        assert!(!permits(Some(&scope), "fastmail"));
        assert!(!permits(Some(&[]), "tfl"));
    }
}
