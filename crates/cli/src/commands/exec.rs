use anyhow::{bail, Result};
use relais_core::types::{Credentials, ExecContext};
use serde_json::Value;

use super::{build_router, open_vault};

pub async fn run(path: &str, data: Option<&str>) -> Result<()> {
    let parts: Vec<&str> = path.splitn(3, '.').collect();
    if parts.len() != 3 {
        bail!("exec path must be 'site.resource.action' (e.g., github.repos.list)");
    }

    let (site_id, resource_id, action_id) = (parts[0], parts[1], parts[2]);

    let params: Value = match data {
        Some(json_str) => serde_json::from_str(json_str)
            .map_err(|e| anyhow::anyhow!("invalid JSON in --data: {e}"))?,
        None => Value::Object(serde_json::Map::new()),
    };

    // Try to retrieve stored credentials from the vault.
    // If the vault is unavailable or no credential is found, proceed without credentials.
    let credentials = match open_vault() {
        Ok(vault) => match vault.retrieve(site_id) {
            Ok(Some(json_str)) => match serde_json::from_str::<Credentials>(&json_str) {
                Ok(creds) => Some(creds),
                Err(_) => {
                    // Legacy plain token format — wrap as ApiKey for backward compat
                    Some(Credentials::api_key(&json_str))
                }
            },
            _ => None,
        },
        Err(_) => None,
    };

    let router = build_router();
    let adapter = router
        .get(site_id)
        .ok_or_else(|| anyhow::anyhow!("site '{site_id}' not found"))?;

    let ctx = ExecContext {
        site: site_id.to_string(),
        resource: resource_id.to_string(),
        action: action_id.to_string(),
        params,
        credentials,
    };

    let response = adapter.exec(&ctx).await?;
    let json = serde_json::to_string_pretty(&response)?;
    println!("{json}");
    Ok(())
}
