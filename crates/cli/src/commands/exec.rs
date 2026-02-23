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
    let vault = open_vault().ok();
    let credentials = vault.as_ref().and_then(|v| {
        v.retrieve(site_id)
            .ok()
            .flatten()
            .and_then(|json_str| {
                serde_json::from_str::<Credentials>(&json_str)
                    .ok()
                    .or_else(|| Some(Credentials::api_key(&json_str)))
            })
    });

    // If the token is expired, try to refresh it automatically.
    let credentials = if let Some(cred) = credentials {
        if cred.is_expired() {
            match relais_core::token_refresh::maybe_refresh(&cred, site_id, vault.as_ref()).await {
                Ok(refreshed) => Some(refreshed),
                Err(e) => {
                    tracing::warn!("token refresh failed for {}: {}", site_id, e);
                    Some(cred)
                }
            }
        } else {
            Some(cred)
        }
    } else {
        None
    };

    // Warn if cookie credentials are stale.
    if let Some(ref cred) = credentials {
        if cred.is_cookie_stale(24) {
            tracing::warn!(
                "cookies for {} are older than 24 hours, consider re-authenticating \
                 with 'relais auth import-cookies'",
                site_id
            );
        }
    }

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
