//! Sidecar preimage store (C4/C6).
//!
//! The hash chain stores only the response *commitment* (`content_hash`), so to keep
//! receipts legible and re-verifiable relais persists the exact redacted
//! `{ request, response }` envelope it hashed, keyed by receipt id, at
//! `dir/sidecars/<id>.json`. `relais audit verify` (C6) recomputes the hash from this
//! file. Receipt ids are `rec_<hex>` (signet `derive_id`), so they are filename-safe.

use std::path::Path;

use serde_json::Value;

use super::AuditError;

/// Atomically write the sidecar for `id` (tmp + rename).
pub fn write(dir: &Path, id: &str, value: &Value) -> Result<(), AuditError> {
    let scdir = dir.join("sidecars");
    std::fs::create_dir_all(&scdir).map_err(|e| AuditError::Io(e.to_string()))?;
    let path = scdir.join(format!("{id}.json"));
    let tmp = scdir.join(format!("{id}.json.tmp"));
    let json = serde_json::to_vec(value).map_err(|e| AuditError::Io(e.to_string()))?;
    std::fs::write(&tmp, json).map_err(|e| AuditError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| AuditError::Io(e.to_string()))?;
    Ok(())
}

/// Read the sidecar preimage for `id`.
pub fn read(dir: &Path, id: &str) -> Result<Value, AuditError> {
    let path = dir.join("sidecars").join(format!("{id}.json"));
    let json = std::fs::read_to_string(&path).map_err(|e| AuditError::Io(e.to_string()))?;
    serde_json::from_str(&json).map_err(|e| AuditError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let v = json!({ "request": { "a": 1 }, "response": { "transport_ok": true } });
        write(dir.path(), "rec_abc123", &v).unwrap();
        assert!(dir.path().join("sidecars").join("rec_abc123.json").exists());
        assert_eq!(read(dir.path(), "rec_abc123").unwrap(), v);
    }
}
