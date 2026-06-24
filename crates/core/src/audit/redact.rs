//! Audit-specific redaction metadata.
//!
//! The generic value/key/secret redaction now lives in the always-compiled
//! [`crate::redact`] module (so non-audit code can use it too); this module
//! re-exports it and adds the audit-only `AuditMeta`.

use serde_json::Value;

pub use crate::redact::{secret_values_of, Redactor, REDACTED};

/// Non-secret descriptor + opaque credential reference attached to the signed
/// request envelope under `_relais_audit`. `t0`/`t1` are the **true** request
/// start/end (RFC 3339) — they live here, inside the signed `params`, so the
/// receipt's top-level `ts_request` can be a monotonic audit-order time without
/// losing the real request window (see C4).
#[derive(Debug, Clone)]
pub struct AuditMeta {
    pub auth_injection: String,
    pub credential_ref: String,
    pub t0: String,
    pub t1: String,
}

impl AuditMeta {
    /// Render as the `_relais_audit` JSON object embedded in the request envelope.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "auth_injection": self.auth_injection,
            "credential_ref": self.credential_ref,
            "t0": self.t0,
            "t1": self.t1,
        })
    }
}
