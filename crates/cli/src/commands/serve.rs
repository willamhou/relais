use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use relais_server::state::SharedState;
use tokio::net::TcpListener;

use super::{build_exec_router, open_vault};

pub async fn run(host: String, port: u16, jwt_secret: String) -> Result<()> {
    // Canonical opt-in only (not any non-empty value).
    let allow_weak = matches!(
        std::env::var("RELAIS_ALLOW_WEAK_JWT_SECRET").as_deref(),
        Ok("1") | Ok("true")
    );
    validate_jwt_secret(&jwt_secret, allow_weak)?;

    let router = build_exec_router()?;

    // Open vault if available; don't fail if vault is inaccessible.
    let vault = open_vault().ok();

    let state = Arc::new(SharedState {
        router,
        jwt_secret,
        vault,
    });

    let app = relais_server::app(state);

    // Build the bind address; warn on any unspecified (all-interfaces) address and
    // bracket IPv6 literals so `::` formats as `[::]:port` not `:::port`.
    let addr = match host.parse::<IpAddr>() {
        Ok(ip) => {
            if ip.is_unspecified() {
                tracing::warn!("binding all interfaces ({ip}) — relais is exposed; ensure a strong JWT secret and network controls");
            }
            if ip.is_ipv6() {
                format!("[{ip}]:{port}")
            } else {
                format!("{ip}:{port}")
            }
        }
        Err(_) => format!("{host}:{port}"), // hostname; bound as-is
    };
    tracing::info!("listening on {addr}");
    println!("Relais server listening on http://{addr}");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Reject well-known / weak JWT secrets so a default launch can't be token-forged.
/// `allow_weak` (from `RELAIS_ALLOW_WEAK_JWT_SECRET`) is a per-area dev escape hatch.
fn validate_jwt_secret(secret: &str, allow_weak: bool) -> Result<()> {
    if allow_weak {
        tracing::warn!(
            "RELAIS_ALLOW_WEAK_JWT_SECRET is set — a weak JWT secret is permitted (DEV ONLY)"
        );
        return Ok(());
    }
    if secret == "dev-secret" {
        bail!(
            "refusing to start with the well-known 'dev-secret' JWT secret. \
             Set a strong secret via RELAIS_JWT_SECRET or --jwt-secret (≥32 chars). \
             For local dev only: RELAIS_ALLOW_WEAK_JWT_SECRET=1"
        );
    }
    if secret.len() < 32 {
        bail!(
            "JWT secret is too short ({} chars); use at least 32 chars \
             (RELAIS_JWT_SECRET or --jwt-secret). For local dev only: \
             RELAIS_ALLOW_WEAK_JWT_SECRET=1",
            secret.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_jwt_secret;

    #[test]
    fn rejects_known_dev_secret() {
        assert!(validate_jwt_secret("dev-secret", false).is_err());
    }

    #[test]
    fn rejects_short_secret() {
        assert!(validate_jwt_secret("tooshort", false).is_err());
    }

    #[test]
    fn accepts_strong_secret() {
        assert!(validate_jwt_secret(&"x".repeat(32), false).is_ok());
    }

    #[test]
    fn allow_weak_overrides() {
        assert!(validate_jwt_secret("dev-secret", true).is_ok());
    }
}
