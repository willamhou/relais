use anyhow::Result;

use super::open_vault;
use crate::VaultAction;

pub fn run(action: &VaultAction) -> Result<()> {
    let vault = open_vault()?;

    match action {
        VaultAction::Store {
            site,
            token,
            token_file,
        } => {
            let token = super::read_secret(token.clone(), token_file.as_deref(), "vault token")?;
            vault.store(site, &token)?;
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
        VaultAction::Migrate => {
            let n = vault.migrate()?;
            println!("Re-encrypted {n} credential(s) into the current vault format.");
        }
    }

    Ok(())
}
