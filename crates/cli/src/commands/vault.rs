use std::path::PathBuf;

use anyhow::Result;
use relais_core::vault::Vault;

use crate::VaultAction;

/// Return the vault directory path (~/.relais/vault/).
fn vault_path() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.join(".relais").join("vault"))
}

/// Read the vault master password from environment or use a default for dev.
fn master_password() -> String {
    std::env::var("RELAIS_VAULT_PASSWORD").unwrap_or_else(|_| "relais-dev-password".to_string())
}

pub fn run(action: &VaultAction) -> Result<()> {
    let path = vault_path()?;
    std::fs::create_dir_all(&path)?;

    let password = master_password();
    let vault = Vault::open(&path, &password)?;

    match action {
        VaultAction::Store { site, token } => {
            vault.store(site, token)?;
            println!("Stored credential for '{site}'");
        }
        VaultAction::List => {
            let sites = vault.list()?;
            if sites.is_empty() {
                println!("No credentials stored.");
            } else {
                let json = serde_json::to_string_pretty(&sites)?;
                println!("{json}");
            }
        }
        VaultAction::Delete { site } => {
            vault.delete(site)?;
            println!("Deleted credential for '{site}'");
        }
    }

    Ok(())
}
