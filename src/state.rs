//! Shared application state, cloned into every handler (all fields are cheap
//! to clone — `Arc`, pools, `reqwest::Client`).

use std::sync::Arc;

use crate::auth::hydra::HydraAdmin;
use crate::config::Config;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    /// For calls that answer in one piece: Hydra, the token endpoint, and the
    /// dashboard's questions to backends. Bounded at 15s end to end.
    pub http: reqwest::Client,
    /// For relaying MCP traffic. Its responses can be SSE streams that stay
    /// open for as long as the client listens, which a total timeout would cut.
    pub proxy_http: reqwest::Client,
    pub hydra: HydraAdmin,
}
