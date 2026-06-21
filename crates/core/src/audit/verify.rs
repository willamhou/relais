//! Audit verification (C6): chain integrity + per-record signature against a
//! trusted, time-windowed key + sidecar hash recompute.
//!
//! signet's `verify_signatures_with_options` selects records by `AuditFilter` (which
//! has no receipt-id/end-time), so it can't enforce per-receipt rotation windows.
//! Instead this verifies each receipt directly with `verify_compound`, choosing the
//! trusted key valid at that receipt's `ts_request` (design §4.11). Verification is
//! **fail-closed**: an absent or empty trust anchor is an error, never self-trust.

use std::path::Path;

use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use signet_core::CompoundReceipt;

use super::{sidecar, AuditError};

/// Operational status of a trusted key. Window membership (`not_before`/`not_after`)
/// governs verification; status is informational metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    Retired,
}

/// One accepted gateway public key and the window it was valid for.
#[derive(Debug, Clone, Deserialize)]
pub struct TrustedKey {
    /// `ed25519:<base64>` (matches `signer.pubkey` in receipts).
    pub pubkey: String,
    pub status: KeyStatus,
    pub not_before: DateTime<Utc>,
    #[serde(default)]
    pub not_after: Option<DateTime<Utc>>,
}

/// The out-of-band trust anchor (`dir/trusted_keys.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct TrustAnchor {
    pub keys: Vec<TrustedKey>,
}

impl TrustAnchor {
    /// Construct directly (used by callers that hold keys in memory / tests).
    pub fn new(keys: Vec<TrustedKey>) -> Self {
        Self { keys }
    }

    /// Load `dir/trusted_keys.json`. **Fail-closed:** a missing or empty anchor is an
    /// error — relais never verifies against self-reported receipt keys (NF-4/NF-5).
    pub fn load(dir: &Path) -> Result<Self, AuditError> {
        let path = dir.join("trusted_keys.json");
        if !path.exists() {
            return Err(AuditError::Config(
                "no trust anchor (trusted_keys.json); refusing to verify (fail-closed)".into(),
            ));
        }
        let json = std::fs::read_to_string(&path).map_err(|e| AuditError::Io(e.to_string()))?;
        let anchor: TrustAnchor = serde_json::from_str(&json)
            .map_err(|e| AuditError::Config(format!("trusted_keys.json: {e}")))?;
        if anchor.keys.is_empty() {
            return Err(AuditError::Config(
                "trust anchor has no keys; refusing to verify (fail-closed)".into(),
            ));
        }
        // Validate every pubkey up front, so a malformed anchor key is a hard error
        // rather than a silent "no trusted key" at verify time (C6 review Q5).
        for k in &anchor.keys {
            parse_pubkey(&k.pubkey)?;
        }
        Ok(anchor)
    }

    /// The trusted verifying key matching the receipt's declared `signer_pubkey` whose
    /// validity window contains `ts`. Selecting by signer (not first-window-match)
    /// avoids falsely rejecting a record signed by another key in an overlapping
    /// window, and fails closed if the signer is not in the anchor (C6 review Q3).
    fn key_for_signer(&self, signer_pubkey: &str, ts: DateTime<Utc>) -> Option<VerifyingKey> {
        self.keys
            .iter()
            .find(|k| {
                k.pubkey == signer_pubkey
                    && ts >= k.not_before
                    && k.not_after.is_none_or(|na| ts <= na)
            })
            .and_then(|k| parse_pubkey(&k.pubkey).ok())
    }
}

/// Outcome of verifying an audit directory.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub records: usize,
    pub chain_ok: bool,
    /// The current chain head (`record_hash` of the latest record), if any. Retain it
    /// out-of-band and pass it back as `expected_head` next time to detect tail
    /// truncation (deleting the newest records leaves a shorter, still-internally-valid
    /// chain — final review HIGH).
    pub head: Option<String>,
    /// Human-readable per-record failures (empty == all good).
    pub failures: Vec<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.chain_ok && self.failures.is_empty()
    }
}

/// Verify chain integrity, every record's signature against the windowed trust
/// anchor, and every record's sidecar hash recompute. If `expected_head` is given,
/// also assert the chain head matches it (tail-truncation detection).
pub fn audit_verify(
    dir: &Path,
    anchor: &TrustAnchor,
    expected_head: Option<&str>,
) -> Result<VerifyReport, AuditError> {
    let chain =
        signet_core::audit::verify_chain(dir).map_err(|e| AuditError::Signet(e.to_string()))?;
    let chain_ok = chain.valid;

    let records = signet_core::audit::query(dir, &signet_core::audit::AuditFilter::default())
        .map_err(|e| AuditError::Signet(e.to_string()))?;

    // signet `query` returns records OLDEST-first (it reverses at the end), so the
    // chain head (most recently appended) is the LAST element.
    let head = records.last().map(|r| r.record_hash.clone());

    let mut failures = Vec::new();
    if let Some(expected) = expected_head {
        match &head {
            Some(h) if h == expected => {}
            Some(h) => failures.push(format!(
                "chain head mismatch: expected {expected}, found {h} (possible tail truncation)"
            )),
            None => failures.push(format!(
                "chain head mismatch: expected {expected}, found an empty chain (truncation)"
            )),
        }
    }
    for record in &records {
        // Only signed v2 compound receipts are produced by relais; skip/flag others.
        let receipt: CompoundReceipt = match serde_json::from_value(record.receipt.clone()) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("record is not a v2 compound receipt: {e}"));
                continue;
            }
        };
        let id = receipt.id.clone();

        let ts = match DateTime::parse_from_rfc3339(&receipt.ts_request) {
            Ok(t) => t.with_timezone(&Utc),
            Err(e) => {
                failures.push(format!("{id}: bad ts_request: {e}"));
                continue;
            }
        };

        // Select the trusted key by the receipt's declared signer, then confirm the
        // signature actually verifies under it. verify_compound protects the receipt's
        // own fields (action incl. params_hash, response content_hash); the sidecar
        // recompute below binds the external preimages to those hashes (C6 review Q2).
        let key = match anchor.key_for_signer(&receipt.signer.pubkey, ts) {
            Some(k) => k,
            None => {
                failures.push(format!(
                    "{id}: signer {} not trusted at {ts}",
                    receipt.signer.pubkey
                ));
                continue;
            }
        };

        if let Err(e) = signet_core::verify_compound(&receipt, &key) {
            failures.push(format!("{id}: signature verification failed: {e}"));
            continue;
        }

        // Sidecar recompute (byte-exact, sha256: prefixed) for response AND request.
        match sidecar::read(dir, &id) {
            Ok(sc) => {
                if let Some(resp) = sc.get("response") {
                    match hash_value(resp) {
                        Ok(h) if h == receipt.response.content_hash => {}
                        Ok(h) => failures.push(format!(
                            "{id}: response hash mismatch (sidecar {h} != receipt {})",
                            receipt.response.content_hash
                        )),
                        Err(e) => failures.push(format!("{id}: cannot hash sidecar response: {e}")),
                    }
                } else {
                    failures.push(format!("{id}: sidecar missing 'response'"));
                }
                if let Some(req) = sc.get("request") {
                    match hash_value(req) {
                        Ok(h) if h == receipt.action.params_hash => {}
                        Ok(h) => failures.push(format!(
                            "{id}: request hash mismatch (sidecar {h} != receipt {})",
                            receipt.action.params_hash
                        )),
                        Err(e) => failures.push(format!("{id}: cannot hash sidecar request: {e}")),
                    }
                } else {
                    failures.push(format!("{id}: sidecar missing 'request'"));
                }
            }
            Err(e) => failures.push(format!("{id}: missing/unreadable sidecar: {e}")),
        }
    }

    Ok(VerifyReport {
        records: records.len(),
        chain_ok,
        head,
        failures,
    })
}

/// A compact view of one audit record for `relais audit tail`.
#[derive(Debug, Clone)]
pub struct TailEntry {
    pub id: String,
    pub tool: String,
    pub ts: String,
    pub signer: String,
}

/// List recent audit records, optionally filtered by `since` (RFC 3339), a site
/// prefix (`site.`), and a max `limit`. Keeps signet usage inside core so the CLI
/// needs no direct signet dependency.
pub fn tail(
    dir: &Path,
    site: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<TailEntry>, AuditError> {
    // When filtering by site we must NOT cap the query first, or the newest global N
    // records (possibly for other sites) would hide older matching ones. Query
    // unbounded, prefix-filter, then truncate to `limit`.
    let query_limit = if site.is_some() { None } else { limit };
    let mut filter = signet_core::audit::AuditFilter {
        limit: query_limit,
        ..Default::default()
    };
    if let Some(s) = since {
        let dt = DateTime::parse_from_rfc3339(s)
            .map_err(|e| AuditError::Config(format!("--since must be RFC 3339: {e}")))?
            .with_timezone(&Utc);
        filter.since = Some(dt);
    }
    let records =
        signet_core::audit::query(dir, &filter).map_err(|e| AuditError::Signet(e.to_string()))?;

    let prefix = site.map(|s| format!("{s}."));
    let mut out = Vec::new();
    for r in records {
        let rcpt = &r.receipt;
        let tool = rcpt
            .get("action")
            .and_then(|a| a.get("tool"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(p) = &prefix {
            if !tool.starts_with(p) {
                continue;
            }
        }
        out.push(TailEntry {
            id: rcpt
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            tool,
            ts: rcpt
                .get("ts_request")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            signer: rcpt
                .get("signer")
                .and_then(|s| s.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    // Apply the limit after site filtering (query was unbounded in that case).
    if site.is_some() {
        if let Some(n) = limit {
            out.truncate(n);
        }
    }
    Ok(out)
}

/// `"sha256:" + hex(sha256(JCS(value)))` — byte-identical to how signet computes
/// `content_hash`/`params_hash` (json-canon + sha2 + hex, STANDARD).
fn hash_value(value: &serde_json::Value) -> Result<String, AuditError> {
    let canon = json_canon::to_string(value).map_err(|e| AuditError::Io(e.to_string()))?;
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(canon.as_bytes()))
    ))
}

fn parse_pubkey(s: &str) -> Result<VerifyingKey, AuditError> {
    let b64 = s
        .strip_prefix("ed25519:")
        .ok_or_else(|| AuditError::Config(format!("pubkey must be ed25519:<base64>: {s}")))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| AuditError::Config(format!("pubkey base64: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AuditError::Config("pubkey must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| AuditError::Config(format!("pubkey: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::key::AuditKey;
    use crate::audit::writer::{spawn_writer, AuditJob};
    use serde_json::json;
    use signet_core::Action;
    use tokio::sync::oneshot;

    fn action() -> Action {
        Action {
            tool: "site.res.act".into(),
            params: json!({ "x": 1 }),
            params_hash: String::new(),
            target: "https://api.example".into(),
            transport: "https".into(),
            session: None,
            call_id: Some("c".into()),
            response_hash: None,
            trace_id: Some("t".into()),
            parent_receipt_id: None,
        }
    }

    async fn write_one(dir: &std::path::Path) {
        let key = AuditKey::load_or_init(dir, "acme", None).unwrap();
        let h = spawn_writer(dir.to_path_buf(), key, 8);
        let (ack, rx) = oneshot::channel();
        let a = action();
        let job = AuditJob {
            request: a.params.clone(),
            response_env: json!({ "transport_ok": true, "data": { "ok": true } }),
            action: a,
            t0: Utc::now(),
            t1: Utc::now(),
            ack,
        };
        h.enqueue_timeout(job, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        rx.await.unwrap().unwrap();
    }

    fn anchor_for(dir: &std::path::Path) -> TrustAnchor {
        let key = AuditKey::load_or_init(dir, "acme", None).unwrap();
        TrustAnchor::new(vec![TrustedKey {
            pubkey: key.pubkey,
            status: KeyStatus::Active,
            not_before: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            not_after: None,
        }])
    }

    #[tokio::test]
    async fn verifies_a_clean_chain() {
        let dir = tempfile::tempdir().unwrap();
        write_one(dir.path()).await;
        let report = audit_verify(dir.path(), &anchor_for(dir.path()), None).unwrap();
        assert!(report.chain_ok);
        assert_eq!(report.records, 1);
        assert!(report.ok(), "unexpected failures: {:?}", report.failures);
    }

    #[tokio::test]
    async fn missing_or_empty_anchor_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(TrustAnchor::load(dir.path()).is_err());
        std::fs::write(dir.path().join("trusted_keys.json"), r#"{"keys":[]}"#).unwrap();
        assert!(TrustAnchor::load(dir.path()).is_err());
    }

    #[tokio::test]
    async fn untrusted_key_window_flags_record() {
        let dir = tempfile::tempdir().unwrap();
        write_one(dir.path()).await;
        // Anchor whose window is entirely in the past → no key valid at receipt ts.
        let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        let anchor = TrustAnchor::new(vec![TrustedKey {
            pubkey: key.pubkey,
            status: KeyStatus::Retired,
            not_before: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            not_after: Some(
                DateTime::parse_from_rfc3339("2000-01-02T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        }]);
        let report = audit_verify(dir.path(), &anchor, None).unwrap();
        assert!(!report.ok());
        assert!(report.failures.iter().any(|f| f.contains("not trusted at")));
    }

    #[tokio::test]
    async fn signer_not_in_anchor_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        write_one(dir.path()).await;
        // A valid but unrelated key → the receipt's signer pubkey is not in the anchor.
        let other = tempfile::tempdir().unwrap();
        let other_key = AuditKey::load_or_init(other.path(), "other", None).unwrap();
        let anchor = TrustAnchor::new(vec![TrustedKey {
            pubkey: other_key.pubkey,
            status: KeyStatus::Active,
            not_before: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            not_after: None,
        }]);
        let report = audit_verify(dir.path(), &anchor, None).unwrap();
        assert!(!report.ok());
        assert!(report.failures.iter().any(|f| f.contains("not trusted at")));
    }

    #[tokio::test]
    async fn tail_truncation_detected_with_expected_head() {
        let dir = tempfile::tempdir().unwrap();
        write_one(dir.path()).await;
        write_one(dir.path()).await; // two records
        let anchor = anchor_for(dir.path());

        let before = audit_verify(dir.path(), &anchor, None).unwrap();
        assert_eq!(before.records, 2);
        let head = before.head.clone().unwrap();

        // Delete exactly the head record (by record_hash), regardless of file order.
        // Pick the .jsonl (the dir also holds signet's <date>.jsonl.lock).
        let adir = dir.path().join("audit");
        let file = std::fs::read_dir(&adir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
            .unwrap();
        let content = std::fs::read_to_string(&file).unwrap();
        let remaining: Vec<String> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| {
                let rec: serde_json::Value = serde_json::from_str(l).unwrap();
                rec.get("record_hash").and_then(|v| v.as_str()) != Some(head.as_str())
            })
            .map(|s| s.to_string())
            .collect();
        std::fs::write(&file, format!("{}\n", remaining.join("\n"))).unwrap();

        // The shorter chain is still internally valid, but the head no longer matches.
        let after = audit_verify(dir.path(), &anchor, Some(&head)).unwrap();
        assert!(
            !after.ok(),
            "truncation should fail when expected_head is supplied"
        );
        assert!(after.failures.iter().any(|f| f.contains("head mismatch")));
    }

    #[test]
    fn malformed_anchor_key_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("trusted_keys.json"),
            r#"{"keys":[{"pubkey":"ed25519:not-base64!!","status":"active","not_before":"2000-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        assert!(TrustAnchor::load(dir.path()).is_err());
    }

    #[tokio::test]
    async fn tampered_sidecar_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        write_one(dir.path()).await;
        // Corrupt the single sidecar's response.
        let scdir = dir.path().join("sidecars");
        let entry = std::fs::read_dir(&scdir).unwrap().next().unwrap().unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        v["response"]["data"]["ok"] = json!(false);
        std::fs::write(entry.path(), serde_json::to_string(&v).unwrap()).unwrap();

        let report = audit_verify(dir.path(), &anchor_for(dir.path()), None).unwrap();
        assert!(!report.ok());
        assert!(report.failures.iter().any(|f| f.contains("hash mismatch")));
    }
}
