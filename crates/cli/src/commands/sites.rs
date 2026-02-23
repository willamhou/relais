use anyhow::Result;

use super::build_router;

pub fn run() -> Result<()> {
    let router = build_router();
    let sites = router.sites();
    let json = serde_json::to_string_pretty(&sites)?;
    println!("{json}");
    Ok(())
}
