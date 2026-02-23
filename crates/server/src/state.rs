use std::sync::Arc;

use relais_core::router::Router;
use relais_core::vault::Vault;

pub type AppState = Arc<SharedState>;

pub struct SharedState {
    pub router: Router,
    pub jwt_secret: String,
    pub vault: Option<Vault>,
}
