use anyhow::Result;

use super::open_vault;
use crate::VaultAction;

pub fn run(action: &VaultAction) -> Result<()> {
    let vault = open_vault()?;

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
