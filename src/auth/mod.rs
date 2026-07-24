//! Authentication: upstream identity (GitHub), the Hydra login/consent
//! handshake, opaque-token introspection, and server-side sessions via opaque
//! cookies. No JWTs reach any client.

pub mod cookie;
pub mod github;
pub mod hydra;
pub mod routes;
