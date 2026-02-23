use anyhow::Result;

use super::build_router;

pub fn run(site: &str) -> Result<()> {
    let router = build_router();
    let adapter = router
        .get(site)
        .ok_or_else(|| anyhow::anyhow!("site '{site}' not found"))?;

    let resources = adapter.resources();
    let json = serde_json::to_string_pretty(&resources)?;
    println!("{json}");
    Ok(())
}
