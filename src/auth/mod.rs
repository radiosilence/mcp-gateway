//! Authentication: upstream identity (GitHub), the Hydra login/consent
//! handshake, JWT verification for MCP bearer tokens, and signed cookies.

pub mod cookie;
pub mod github;
pub mod hydra;
pub mod jwks;
pub mod jwt;
pub mod routes;
