use std::sync::Arc;

use relais_core::router::Router;

pub type AppState = Arc<SharedState>;

pub struct SharedState {
    pub router: Router,
    pub jwt_secret: String,
}
