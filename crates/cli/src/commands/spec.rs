use anyhow::{bail, Result};
use relais_core::types::{Action, Resource};

use super::build_router;

pub fn run(path: &str) -> Result<()> {
    let parts: Vec<&str> = path.splitn(3, '.').collect();
    if parts.len() != 3 {
        bail!("spec path must be 'site.resource.action' (e.g., github.repos.list)");
    }

    let (site_id, resource_id, action_id) = (parts[0], parts[1], parts[2]);

    let router = build_router();
    let adapter = router
        .get(site_id)
        .ok_or_else(|| anyhow::anyhow!("site '{site_id}' not found"))?;

    let resources = adapter.resources();
    let action = find_action(&resources, resource_id, action_id)
        .ok_or_else(|| anyhow::anyhow!("action '{resource_id}.{action_id}' not found in site '{site_id}'"))?;

    let json = serde_json::to_string_pretty(&action)?;
    println!("{json}");
    Ok(())
}

/// Recursively search resources for a matching action.
fn find_action(resources: &[Resource], resource_id: &str, action_id: &str) -> Option<Action> {
    for resource in resources {
        if resource.id == resource_id {
            for action in &resource.actions {
                if action.id == action_id {
                    return Some(action.clone());
                }
            }
        }
        if let Some(action) = find_action(&resource.children, resource_id, action_id) {
            return Some(action);
        }
    }
    None
}
