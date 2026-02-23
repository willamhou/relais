pub mod auth;
pub mod handlers;
pub mod state;

use axum::{middleware, routing, Router};
use state::AppState;

/// Build the axum application with all routes and middleware.
///
/// - `GET /health` is public (no auth).
/// - All `/v1/*` routes require a valid JWT in the Authorization header.
pub fn app(state: AppState) -> Router {
    let authenticated = Router::new()
        .route("/sites", routing::get(handlers::list_sites))
        .route("/apis/{site}", routing::get(handlers::list_apis))
        .route("/spec/{spec_path}", routing::get(handlers::get_spec))
        .route("/exec", routing::post(handlers::exec_action))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_jwt,
        ));

    Router::new()
        .route("/health", routing::get(handlers::health))
        .nest("/v1", authenticated)
        .with_state(state)
}
