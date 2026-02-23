use std::sync::Arc;

use anyhow::Result;
use relais_server::state::SharedState;
use tokio::net::TcpListener;

use super::{build_router, open_vault};

pub async fn run(port: u16, jwt_secret: String) -> Result<()> {
    let router = build_router();

    // Open vault if available; don't fail if vault is inaccessible.
    let vault = open_vault().ok();

    let state = Arc::new(SharedState {
        router,
        jwt_secret,
        vault,
    });

    let app = relais_server::app(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("listening on {addr}");
    println!("Relais server listening on http://{addr}");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
