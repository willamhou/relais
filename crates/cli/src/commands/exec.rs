use anyhow::{bail, Result};
use relais_core::types::ExecContext;
use serde_json::Value;

use super::build_router;

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

    let router = build_router();
    let adapter = router
        .get(site_id)
        .ok_or_else(|| anyhow::anyhow!("site '{site_id}' not found"))?;

    let ctx = ExecContext {
        site: site_id.to_string(),
        resource: resource_id.to_string(),
        action: action_id.to_string(),
        params,
        credentials: None,
    };

    let response = adapter.exec(&ctx).await?;
    let json = serde_json::to_string_pretty(&response)?;
    println!("{json}");
    Ok(())
}
