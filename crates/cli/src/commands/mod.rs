pub mod apis;
pub mod audit;
pub mod auth;
pub mod exec;
pub mod serve;
pub mod sites;
pub mod spec;
pub mod vault;

use relais_core::router::Router;

/// The signet/audit directory: `RELAIS_SIGNET_DIR` or `~/.relais/signet`.
#[cfg(feature = "audit")]
pub fn audit_dir() -> anyhow::Result<std::path::PathBuf> {
    let dir = if let Ok(d) = std::env::var("RELAIS_SIGNET_DIR") {
        std::path::PathBuf::from(d)
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
            .join(".relais")
            .join("signet")
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Attach an audit sink to a router from the environment, if auditing is enabled.
///
/// Off unless `RELAIS_AUDIT_MODE` is `open` or `closed`. In **closed** mode a
/// sink-init failure is **fatal** (we must not run unaudited when the operator asked
/// for "no result without a receipt"); in **open** mode it logs and runs unaudited.
/// Must be called within a tokio runtime (the writer task needs one).
#[cfg(feature = "audit")]
fn attach_audit(router: Router) -> anyhow::Result<Router> {
    use relais_core::audit::{AuditConfig, AuditMode, AuditSink};

    let mode = match std::env::var("RELAIS_AUDIT_MODE") {
        Err(_) => return Ok(router), // auditing disabled
        Ok(m) => match m.as_str() {
            "open" => AuditMode::Open,
            "closed" => AuditMode::Closed,
            other => {
                tracing::warn!("ignoring RELAIS_AUDIT_MODE={other} (expected 'open' or 'closed')");
                return Ok(router);
            }
        },
    };
    let dir = audit_dir()?;
    let owner = std::env::var("RELAIS_AUDIT_OWNER").unwrap_or_else(|_| "relais".into());
    match AuditSink::new(AuditConfig {
        dir,
        owner,
        mode,
        capacity: 1024,
        ack_timeout: std::time::Duration::from_secs(5),
    }) {
        Ok(sink) => Ok(router.with_audit(sink)),
        Err(e) => match mode {
            AuditMode::Closed => Err(anyhow::anyhow!(
                "audit sink init failed in closed mode (refusing to run unaudited): {e}"
            )),
            AuditMode::Open => {
                tracing::error!(error = %e, "audit sink init failed; running unaudited (open mode)");
                Ok(router)
            }
        },
    }
}

/// Build the router used for **action execution** (`exec`/`serve`): all adapters plus
/// the audit sink when enabled. Introspection commands use [`build_router`] instead
/// (they never execute actions, so auditing does not apply).
pub fn build_exec_router() -> anyhow::Result<Router> {
    let router = build_router();
    #[cfg(feature = "audit")]
    let router = attach_audit(router)?;
    Ok(router)
}

/// Open the vault from the default location (~/.relais/vault/).
///
/// Reads the master password from `RELAIS_VAULT_PASSWORD` or falls back to a
/// hard-coded development default.
pub fn open_vault() -> anyhow::Result<relais_core::vault::Vault> {
    let vault_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not find home directory"))?
        .join(".relais")
        .join("vault");
    std::fs::create_dir_all(&vault_dir)?;
    let password =
        std::env::var("RELAIS_VAULT_PASSWORD").unwrap_or_else(|_| "relais-dev-password".into());
    Ok(relais_core::vault::Vault::open(&vault_dir, &password)?)
}

/// Build a Router with all built-in adapters registered.
pub fn build_router() -> Router {
    let mut router = Router::new();
    router.register(Box::new(relais_adapter_github::GitHubAdapter::new()));
    router.register(Box::new(relais_adapter_hackernews::HackerNewsAdapter::new()));
    router.register(Box::new(relais_adapter_scs::ScsAdapter::new()));
    router.register(Box::new(relais_adapter_scs_legacy::ScsLegacyAdapter::new()));
    // LLM fallback adapter requires a provider configuration.
    // Skip registration here; users can configure it via environment variables in the future.
    router
}
