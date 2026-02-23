use std::sync::Arc;

use anyhow::Result;
use relais_server::state::SharedState;
use tokio::net::TcpListener;

use super::build_router;

pub async fn run(port: u16, jwt_secret: String) -> Result<()> {
    let router = build_router();

    let state = Arc::new(SharedState {
        router,
        jwt_secret,
    });

    let app = relais_server::app(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("listening on {addr}");
    println!("Relais server listening on http://{addr}");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
