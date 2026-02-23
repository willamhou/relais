pub mod apis;
pub mod exec;
pub mod serve;
pub mod sites;
pub mod spec;
pub mod vault;

use relais_core::router::Router;

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
    let password = std::env::var("RELAIS_VAULT_PASSWORD")
        .unwrap_or_else(|_| "relais-dev-password".into());
    Ok(relais_core::vault::Vault::open(&vault_dir, &password)?)
}

/// Build a Router with all built-in adapters registered.
pub fn build_router() -> Router {
    let mut router = Router::new();
    router.register(Box::new(
        relais_adapter_github::GitHubAdapter::new(),
    ));
    router.register(Box::new(
        relais_adapter_hackernews::HackerNewsAdapter::new(),
    ));
    // LLM fallback adapter requires a provider configuration.
    // Skip registration here; users can configure it via environment variables in the future.
    router
}
