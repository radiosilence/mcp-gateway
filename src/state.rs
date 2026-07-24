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
    pub http: reqwest::Client,
    pub hydra: HydraAdmin,
}
