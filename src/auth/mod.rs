//! Authentication: an authorization-code flow against the issuer, opaque-token
//! introspection for the MCP proxy, personal access tokens for callers that
//! cannot run that flow, and server-side sessions via opaque cookies. No JWTs
//! reach any client.
//!
//! Who may sign in, and which upstream vouches for them, is the login
//! provider's business and appears nowhere here.

pub mod access_token;
pub mod cookie;
pub mod extract;
pub mod hydra;
pub mod oidc;
pub mod routes;
