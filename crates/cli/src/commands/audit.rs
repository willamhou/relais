//! `relais audit {init,pubkey,verify,tail}` (C7).
//!
//! Real implementations require the `audit` feature; without it the command returns
//! a clear "rebuild with --features audit" error so the CLI surface is stable.

use crate::AuditAction;
use anyhow::Result;

#[cfg(not(feature = "audit"))]
pub async fn run(_action: AuditAction) -> Result<()> {
    anyhow::bail!(
        "this build of relais was compiled without the `audit` feature; \
         rebuild with `cargo install relais-cli --features audit` (or `--features audit`)"
    )
}

#[cfg(feature = "audit")]
pub async fn run(action: AuditAction) -> Result<()> {
    use relais_core::audit::key::AuditKey;
    use relais_core::audit::verify::{audit_verify, tail, TrustAnchor};

    let dir = super::audit_dir()?;

    match action {
        AuditAction::Init { owner } => {
            let owner = owner
                .or_else(|| std::env::var("RELAIS_AUDIT_OWNER").ok())
                .unwrap_or_else(|| "relais".into());
            let key = AuditKey::load_or_init(&dir, &owner, None)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!("audit key ready under {}", dir.join("keys").display());
            println!("owner:  {owner}");
            println!("pubkey: {}", key.pubkey);
            println!(
                "\nTo verify elsewhere, add this pubkey to {}",
                dir.join("trusted_keys.json").display()
            );
            Ok(())
        }
        AuditAction::Pubkey => {
            let key = AuditKey::load_or_init(&dir, "relais", None)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!("{}", key.pubkey);
            Ok(())
        }
        AuditAction::Verify { head } => {
            let anchor = TrustAnchor::load(&dir).map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let report = audit_verify(&dir, &anchor, head.as_deref())
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            println!("records: {}", report.records);
            println!("chain:   {}", if report.chain_ok { "ok" } else { "BROKEN" });
            if let Some(h) = &report.head {
                println!("head:    {h}");
            }
            if report.ok() {
                println!("result:  OK — audit chain verified");
                Ok(())
            } else {
                if !report.chain_ok {
                    eprintln!("FAIL: hash chain is broken");
                }
                for f in &report.failures {
                    eprintln!("FAIL: {f}");
                }
                let issues = report.failures.len() + usize::from(!report.chain_ok);
                anyhow::bail!("audit verification failed ({issues} issue(s))")
            }
        }
        AuditAction::Tail { site, since, limit } => {
            let entries = tail(&dir, site.as_deref(), since.as_deref(), limit)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            if entries.is_empty() {
                println!("(no audit records)");
            }
            for e in entries {
                println!("{}  {}  {}  signer={}", e.ts, e.id, e.tool, e.signer);
            }
            Ok(())
        }
    }
}
