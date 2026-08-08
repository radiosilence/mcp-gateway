//! Authentication: an authorization-code flow against the issuer, opaque-token
//! introspection for the MCP proxy, and server-side sessions via opaque
//! cookies. No JWTs reach any client.
//!
//! Who may sign in, and which upstream vouches for them, is the login
//! provider's business and appears nowhere here.

pub mod cookie;
pub mod extract;
pub mod hydra;
pub mod oidc;
pub mod routes;
