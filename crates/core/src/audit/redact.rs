//! Redaction of request/response payloads before they enter an audit receipt.
//!
//! Two complementary defences (C2):
//! 1. **Key-name redaction** — values under sensitive keys (`token`, `password`,
//!    `*_token`, …) are masked regardless of content.
//! 2. **Secret-value redaction** — the actual credential strings (pulled from
//!    [`crate::types::Credentials`]) are masked wherever they appear, under *any*
//!    key or as a substring of a larger string. This is what makes the leak guard
//!    hold even when an upstream echoes a token back in its response.

use serde_json::{Map, Value};

use crate::types::{CredentialData, Credentials};

/// The placeholder written in place of redacted content.
pub const REDACTED: &str = "[REDACTED]";

/// Keys whose values are always masked (compared case-insensitively).
const DEFAULT_DENY_EXACT: &[&str] = &[
    "token",
    "password",
    "secret",
    "authorization",
    "api_key",
    "acs_token",
    "cookie",
    "cookies",
];

/// Key suffixes whose values are always masked (e.g. `access_token`, `refresh_token`).
const DEFAULT_DENY_SUFFIX: &[&str] = &["_token"];

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

/// Redacts JSON values by key name and by secret value.
#[derive(Debug, Clone)]
pub struct Redactor {
    deny_exact: Vec<String>,
    deny_suffix: Vec<String>,
}

impl Default for Redactor {
    fn default() -> Self {
        Self {
            deny_exact: DEFAULT_DENY_EXACT.iter().map(|s| s.to_string()).collect(),
            deny_suffix: DEFAULT_DENY_SUFFIX.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    fn key_is_sensitive(&self, key: &str) -> bool {
        let k = key.to_ascii_lowercase();
        self.deny_exact.iter().any(|d| d == &k) || self.deny_suffix.iter().any(|s| k.ends_with(s))
    }

    /// Returns `v` with sensitive keys masked and any occurrence of a `secret`
    /// (non-empty) masked, recursively — in object **keys** and values, in string
    /// leaves (substring), and in numeric/boolean leaves whose textual form is
    /// exactly a secret. The result is what gets hashed and stored, so the proof is
    /// honest about what was recorded.
    pub fn redact_value(&self, v: &Value, secrets: &[String]) -> Value {
        self.redact_inner(v, &sorted_secrets(secrets))
    }

    fn redact_inner(&self, v: &Value, secrets: &[&str]) -> Value {
        match v {
            Value::Object(map) => {
                let mut out = Map::with_capacity(map.len());
                for (k, val) in map {
                    // mask a secret used AS a key name too (keys were skipped before).
                    // Disambiguate if masking collapses two keys to the same string, so
                    // a field is never silently dropped (data-integrity, RQ1).
                    let key = unique_key(&out, mask_secrets(k, secrets));
                    if self.key_is_sensitive(k) {
                        out.insert(key, Value::String(REDACTED.to_string()));
                    } else {
                        out.insert(key, self.redact_inner(val, secrets));
                    }
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|i| self.redact_inner(i, secrets))
                    .collect(),
            ),
            Value::String(s) => Value::String(mask_secrets(s, secrets)),
            Value::Number(_) | Value::Bool(_) => {
                // a secret like "123" or "true" present as a JSON scalar (HIGH)
                let text = v.to_string();
                if secrets.iter().any(|sec| text == *sec) {
                    Value::String(REDACTED.to_string())
                } else {
                    v.clone()
                }
            }
            Value::Null => Value::Null,
        }
    }

    /// Mask secrets in a plain string (e.g. an error message). Used by the response
    /// envelope so error text never leaks a credential.
    ///
    /// **Residual risk (accepted for v1, RQ3):** this is exact-substring masking. A
    /// secret that a nested error (`reqwest`/`anyhow`) emits in a *transformed* form
    /// — URL-encoded, base64, truncated — can survive, because v1 redaction is
    /// denylist + raw-value matching, not format-aware. Tracked in the design's
    /// best-effort redaction stance; format-aware redaction is out of scope for v1.
    pub fn redact_str(&self, s: &str, secrets: &[String]) -> String {
        mask_secrets(s, &sorted_secrets(secrets))
    }
}

/// Return `key`, or a `key#N` variant that does not yet exist in `out`, so masking
/// two distinct keys to the same string never silently drops a field (RQ1).
fn unique_key(out: &Map<String, Value>, key: String) -> String {
    if !out.contains_key(&key) {
        return key;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{key}#{n}");
        if !out.contains_key(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Non-empty secrets, longest first, so an overlapping shorter secret can't leave
/// suffix material from a longer one (MEDIUM).
fn sorted_secrets(secrets: &[String]) -> Vec<&str> {
    let mut v: Vec<&str> = secrets
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    v.sort_by_key(|b| std::cmp::Reverse(b.len()));
    v
}

/// Replace every occurrence of each secret substring with [`REDACTED`].
fn mask_secrets(s: &str, secrets: &[&str]) -> String {
    let mut out = s.to_string();
    for secret in secrets {
        if out.contains(secret) {
            out = out.replace(secret, REDACTED);
        }
    }
    out
}

/// The actual secret strings carried by `credentials` — matched directly against
/// **all** `CredentialData` variants (not via `bearer_token()`, which omits the
/// refresh token).
pub fn secret_values_of(creds: &Option<Credentials>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(c) = creds {
        match &c.data {
            CredentialData::ApiKey { token } => out.push(token.clone()),
            CredentialData::OAuth {
                access_token,
                refresh_token,
                ..
            } => {
                out.push(access_token.clone());
                if let Some(rt) = refresh_token {
                    out.push(rt.clone());
                }
            }
            CredentialData::Cookie { cookies, .. } => {
                out.extend(cookies.values().cloned());
            }
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_sensitive_keys_case_insensitive_and_suffix() {
        let r = Redactor::new();
        let v = json!({
            "Authorization": "Bearer abc",
            "API_KEY": "k",
            "access_token": "t",
            "customer_id": "42",
            "nested": { "password": "p", "ok": "keep" }
        });
        let out = r.redact_value(&v, &[]);
        assert_eq!(out["Authorization"], json!(REDACTED));
        assert_eq!(out["API_KEY"], json!(REDACTED));
        assert_eq!(out["access_token"], json!(REDACTED));
        assert_eq!(out["customer_id"], json!("42"));
        assert_eq!(out["nested"]["password"], json!(REDACTED));
        assert_eq!(out["nested"]["ok"], json!("keep"));
    }

    #[test]
    fn masks_secret_values_under_any_key_and_as_substring() {
        let r = Redactor::new();
        let secrets = vec!["SUPERSECRET".to_string()];
        let v = json!({
            "echoed": "SUPERSECRET",
            "in_text": "prefix SUPERSECRET suffix",
            "arr": ["x", "SUPERSECRET"]
        });
        let out = r.redact_value(&v, &secrets);
        assert_eq!(out["echoed"], json!(REDACTED));
        assert_eq!(out["in_text"], json!(format!("prefix {REDACTED} suffix")));
        assert_eq!(out["arr"][1], json!(REDACTED));
    }

    #[test]
    fn empty_secret_does_not_mask_everything() {
        let r = Redactor::new();
        let out = r.redact_value(&json!({"a": "b"}), &["".to_string()]);
        assert_eq!(out["a"], json!("b"));
    }

    #[test]
    fn masks_secret_used_as_object_key() {
        let r = Redactor::new();
        let secrets = vec!["SECRETKEY".to_string()];
        let out = r.redact_value(&json!({ "SECRETKEY": "v", "ok": 1 }), &secrets);
        let s = serde_json::to_string(&out).unwrap();
        assert!(!s.contains("SECRETKEY"), "secret survived as a key: {s}");
        assert_eq!(out["ok"], json!(1));
    }

    #[test]
    fn masks_numeric_scalar_equal_to_secret() {
        let r = Redactor::new();
        let secrets = vec!["123456".to_string()];
        let out = r.redact_value(&json!({ "pin": 123456, "qty": 2 }), &secrets);
        assert_eq!(out["pin"], json!(REDACTED));
        assert_eq!(out["qty"], json!(2));
    }

    #[test]
    fn longest_secret_masked_first() {
        let r = Redactor::new();
        // "abc" is a prefix of "abcdef"; masking the longer first avoids leaving "def"
        let secrets = vec!["abc".to_string(), "abcdef".to_string()];
        let out = r.redact_value(&json!({ "v": "abcdef" }), &secrets);
        assert_eq!(out["v"], json!(REDACTED));
    }

    #[test]
    fn masked_key_collision_is_disambiguated_not_dropped() {
        let r = Redactor::new();
        // both keys contain the secret → both mask to the same base; keep both.
        let secrets = vec!["S".to_string()];
        let out = r.redact_value(&json!({ "Sa": 1, "Sb": 2 }), &secrets);
        let obj = out.as_object().unwrap();
        assert_eq!(obj.len(), 2, "a field was silently dropped: {out}");
    }

    #[test]
    fn redact_str_masks_error_text() {
        let r = Redactor::new();
        let masked = r.redact_str("auth failed for TOKENXYZ", &["TOKENXYZ".to_string()]);
        assert!(!masked.contains("TOKENXYZ"));
    }

    #[test]
    fn secret_values_cover_all_variants() {
        use crate::types::AuthType;
        use std::collections::HashMap;

        let api = Credentials::api_key("apitok");
        assert_eq!(secret_values_of(&Some(api)), vec!["apitok".to_string()]);

        let oauth = Credentials::oauth("acc", Some("ref".to_string()), None);
        let got = secret_values_of(&Some(oauth));
        assert!(got.contains(&"acc".to_string()) && got.contains(&"ref".to_string()));

        let mut cookies = HashMap::new();
        cookies.insert("session".to_string(), "cookieval".to_string());
        let cookie = Credentials {
            credential_type: AuthType::Cookie,
            data: CredentialData::Cookie {
                cookies,
                domain: "x".into(),
                captured_at: chrono::Utc::now(),
                expires_at: None,
            },
        };
        assert_eq!(
            secret_values_of(&Some(cookie)),
            vec!["cookieval".to_string()]
        );
    }
}
