//! Data-driven adapter for the **legacy SCS** service (scs_old, Beego) — the
//! full `/1/*` action-based API (~1324 endpoints).
//!
//! Unlike the hand-written adapters, this one is generated: an offline script
//! (`generate/gen_spec.py`) distills the legacy Swagger into `scs_legacy_spec.json`,
//! embedded here via `include_str!`. The engine builds the resource tree and routes
//! `exec` purely from that spec, so its code size is constant regardless of how many
//! endpoints the legacy service has.
//!
//! Auth: legacy SCS authenticates via an `acs_token` carried in the request (body
//! field for POST, query param for GET). Store it as an `APIKey` credential for the
//! `scs` site; the adapter injects it automatically.
use std::collections::BTreeMap;
use std::sync::LazyLock;

use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, ExecContext, Method, Resource, Response, ResponseMeta,
    SiteManifest,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8501";

const SPEC_JSON: &str = include_str!("../scs_legacy_spec.json");

#[derive(Debug, Deserialize)]
struct Spec {
    base_path: String,
    modules: BTreeMap<String, ModuleDef>,
}

#[derive(Debug, Deserialize)]
struct ModuleDef {
    #[serde(default)]
    description: String,
    actions: BTreeMap<String, ActionDef>,
}

#[derive(Debug, Deserialize)]
struct ActionDef {
    method: String,
    path: String,
    #[serde(default)]
    description: String,
    params: Value,
}

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    serde_json::from_str(SPEC_JSON).expect("embedded scs_legacy_spec.json is valid")
});

pub struct ScsLegacyAdapter {
    client: Client,
    base_url: String,
}

impl ScsLegacyAdapter {
    /// Construct an adapter, reading the base URL from `SCS_LEGACY_BASE_URL`
    /// (falling back to [`DEFAULT_BASE_URL`]).
    pub fn new() -> Self {
        Self::with_base_url(
            std::env::var("SCS_LEGACY_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
        )
    }

    /// Construct an adapter with an explicit base URL (no env lookup).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if check_transport(&base_url, false).is_err() {
            tracing::warn!(
                "SCS legacy base_url '{base_url}' is plaintext http to a non-loopback host; \
                 acs_token requests will be refused unless RELAIS_SCS_ALLOW_INSECURE=1 — prefer https"
            );
        }
        Self {
            client: Client::new(),
            base_url,
        }
    }
}

impl Default for ScsLegacyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for ScsLegacyAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "scs".into(),
            name: "SCS (legacy)".into(),
            base_url: self.base_url.clone(),
            auth_type: AuthType::APIKey,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        SPEC.modules
            .iter()
            .map(|(mid, m)| Resource {
                id: mid.clone(),
                description: m.description.clone(),
                actions: m
                    .actions
                    .iter()
                    .map(|(aid, a)| Action {
                        id: aid.clone(),
                        method: http_to_method(&a.method),
                        description: a.description.clone(),
                        params: a.params.clone(),
                        returns: json!({ "type": "object" }),
                        pagination: None,
                    })
                    .collect(),
                children: vec![],
            })
            .collect()
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let action = lookup(&ctx.resource, &ctx.action)?;
        // Never send the acs_token over plaintext http to a remote host.
        let allow_insecure = matches!(
            std::env::var("RELAIS_SCS_ALLOW_INSECURE").as_deref(),
            Ok("1") | Ok("true")
        );
        check_transport(&self.base_url, allow_insecure).map_err(AdapterError::Unsupported)?;
        let url = build_url(&self.base_url, action);
        let acs_token = ctx.credentials.as_ref().and_then(|c| c.bearer_token());

        let req = if action.method.eq_ignore_ascii_case("GET") {
            self.client
                .get(&url)
                .query(&prepare_query(&ctx.params, acs_token))
        } else {
            let method = reqwest::Method::from_bytes(action.method.as_bytes())
                .unwrap_or(reqwest::Method::POST);
            self.client
                .request(method, &url)
                .json(&prepare_body(&ctx.params, acs_token))
        };

        let resp = req.send().await?;
        let status = resp.status();

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(AdapterError::Auth(format!("legacy SCS returned {status}")));
        }
        if status == StatusCode::NOT_FOUND {
            return Err(AdapterError::NotFound(format!(
                "{}.{} not found",
                ctx.resource, ctx.action
            )));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(AdapterError::RateLimited {
                retry_after_secs: retry_after,
            });
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            // Defense-in-depth: if the upstream echoes the token in its error body,
            // strip it before it can reach logs/audit/callers.
            let text = match acs_token {
                Some(t) if !t.is_empty() => text.replace(t, "[REDACTED]"),
                _ => text,
            };
            return Err(AdapterError::Other(anyhow::anyhow!(
                "legacy SCS error {status}: {text}"
            )));
        }

        // Legacy returns business JSON ({err_code, err_msg, data, ...}); pass it
        // through verbatim. Business-level error codes are left for the caller.
        let text = resp.text().await.unwrap_or_default();
        let data = parse_body(&text)?;

        Ok(Response {
            data,
            meta: ResponseMeta {
                pagination: None,
                rate_limit: None,
                cached: false,
                receipt: None,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (spec lookup, URL/body/query construction)
// ---------------------------------------------------------------------------

fn lookup(resource: &str, action: &str) -> Result<&'static ActionDef, AdapterError> {
    SPEC.modules
        .get(resource)
        .and_then(|m| m.actions.get(action))
        .ok_or_else(|| AdapterError::Unsupported(format!("{resource}.{action}")))
}

/// `{base_url}{base_path}{action.path}` — e.g. `http://h:8501` + `/1` + `/accounts/create`.
fn build_url(base_url: &str, action: &ActionDef) -> String {
    format!("{base_url}{}{}", SPEC.base_path, action.path)
}

/// Refuse to send the `acs_token` over plaintext http to a non-loopback host — it
/// leaks through access logs, proxies, and caches (H4). `https` and loopback `http`
/// are allowed; `allow_insecure` (RELAIS_SCS_ALLOW_INSECURE=1) is an explicit
/// override for trusted private networks. An unknown scheme is left untouched.
fn check_transport(base_url: &str, allow_insecure: bool) -> Result<(), String> {
    // Normalize the scheme to lowercase so `HTTP://` can't bypass the check.
    let lower = base_url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let rest = match lower.strip_prefix("http://") {
        Some(r) => r,
        None => return Ok(()),
    };
    // Extract the host, handling bracketed IPv6 literals (`[::1]:port`), and strip any
    // `user:pass@` userinfo so it can't smuggle a fake host.
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(after) = hostport.strip_prefix('[') {
        after.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    // Use real IP parsing for loopback (127.0.0.0/8 and ::1) so a name like
    // `127.0.0.1.evil.com` (which does NOT parse as an IP) is correctly NOT loopback.
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if is_loopback || allow_insecure {
        return Ok(());
    }
    Err(format!(
        "refusing to send acs_token over plaintext http to non-loopback host '{host}'; \
         use https or set RELAIS_SCS_ALLOW_INSECURE=1 to override"
    ))
}

fn http_to_method(http: &str) -> Method {
    match http.to_ascii_uppercase().as_str() {
        "GET" => Method::Read,
        "DELETE" => Method::Delete,
        _ => Method::Write,
    }
}

/// Build a JSON body from the caller params plus the injected `acs_token`.
fn prepare_body(params: &Value, acs_token: Option<&str>) -> Value {
    let mut obj: Map<String, Value> = params.as_object().cloned().unwrap_or_default();
    if let Some(t) = acs_token {
        obj.insert("acs_token".into(), json!(t));
    }
    Value::Object(obj)
}

/// Build query pairs from scalar params plus the injected `acs_token`.
fn prepare_query(params: &Value, acs_token: Option<&str>) -> Vec<(String, String)> {
    let mut q = Vec::new();
    if let Some(obj) = params.as_object() {
        for (k, v) in obj {
            if let Some(s) = scalar_to_string(v) {
                q.push((k.clone(), s));
            }
        }
    }
    if let Some(t) = acs_token {
        q.push(("acs_token".into(), t.to_string()));
    }
    q
}

fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_body(text: &str) -> Result<Value, AdapterError> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    // Most legacy endpoints return JSON, but a few (test stubs, gateway callbacks,
    // exports) return plain text/HTML. Wrap non-JSON in `{ "raw": ... }` rather than
    // failing the whole call, so every endpoint round-trips.
    match serde_json::from_str(text) {
        Ok(v) => Ok(v),
        Err(_) => Ok(json!({ "raw": text })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_loads_with_modules_and_actions() {
        assert!(SPEC.modules.len() >= 70, "expected ~79 modules");
        assert_eq!(SPEC.base_path, "/1");
        let total: usize = SPEC.modules.values().map(|m| m.actions.len()).sum();
        assert!(total >= 1300, "expected ~1324 actions, got {total}");
    }

    #[test]
    fn transport_allows_https_and_loopback_http() {
        assert!(check_transport("https://scs.example.com", false).is_ok());
        assert!(check_transport("http://127.0.0.1:8501", false).is_ok());
        assert!(check_transport("http://localhost:8501", false).is_ok());
        assert!(check_transport("http://[::1]:8501", false).is_ok());
    }

    #[test]
    fn transport_blocks_remote_http_unless_override() {
        assert!(check_transport("http://scs.example.com/1", false).is_err());
        assert!(check_transport("http://scs.example.com/1", true).is_ok());
    }

    #[test]
    fn transport_rejects_loopback_lookalike_and_userinfo() {
        // a name that merely starts with `127.` is NOT loopback
        assert!(check_transport("http://127.0.0.1.evil.com/1", false).is_err());
        // userinfo cannot smuggle a loopback host
        assert!(check_transport("http://127.0.0.1@evil.com/1", false).is_err());
        // a real 127.0.0.0/8 address IS loopback
        assert!(check_transport("http://127.5.5.5:8501", false).is_ok());
        // case-variant scheme must not bypass the check
        assert!(check_transport("HTTP://scs.example.com/1", false).is_err());
        assert!(check_transport("HtTpS://scs.example.com", false).is_ok());
    }

    #[test]
    fn accounts_module_has_create() {
        let a = lookup("accounts", "create").unwrap();
        assert_eq!(a.method, "POST");
        assert_eq!(a.path, "/accounts/create");
    }

    #[test]
    fn lookup_unknown_is_unsupported() {
        assert!(lookup("nope", "nope").is_err());
        assert!(lookup("accounts", "nope").is_err());
    }

    #[test]
    fn build_url_joins_base_path_and_path() {
        let a = lookup("accounts", "create").unwrap();
        assert_eq!(
            build_url("http://h:8501", a),
            "http://h:8501/1/accounts/create"
        );
    }

    #[test]
    fn params_never_expose_acs_token() {
        // acs_token is a credential, stripped by the generator from every action.
        for m in SPEC.modules.values() {
            for a in m.actions.values() {
                if let Some(props) = a.params.get("properties").and_then(|p| p.as_object()) {
                    assert!(
                        !props.contains_key("acs_token"),
                        "acs_token leaked into params for {}",
                        a.path
                    );
                }
            }
        }
    }

    #[test]
    fn prepare_body_injects_acs_token() {
        let body = prepare_body(&json!({"name": "x"}), Some("tok"));
        assert_eq!(body["name"], "x");
        assert_eq!(body["acs_token"], "tok");
    }

    #[test]
    fn prepare_body_without_token() {
        let body = prepare_body(&json!({"name": "x"}), None);
        assert_eq!(body, json!({"name": "x"}));
    }

    #[test]
    fn prepare_query_includes_token_and_scalars() {
        let q = prepare_query(&json!({"page": 2, "kw": "a"}), Some("tok"));
        assert!(q.contains(&("page".to_string(), "2".to_string())));
        assert!(q.contains(&("kw".to_string(), "a".to_string())));
        assert!(q.contains(&("acs_token".to_string(), "tok".to_string())));
    }

    #[test]
    fn http_method_mapping() {
        assert!(matches!(http_to_method("GET"), Method::Read));
        assert!(matches!(http_to_method("post"), Method::Write));
        assert!(matches!(http_to_method("DELETE"), Method::Delete));
    }

    #[test]
    fn with_base_url_strips_trailing_slash() {
        let a = ScsLegacyAdapter::with_base_url("http://h:8501/");
        assert_eq!(a.base_url, "http://h:8501");
    }

    #[test]
    fn parse_body_empty_is_empty_object() {
        assert_eq!(parse_body("").unwrap(), json!({}));
        assert_eq!(parse_body("  ").unwrap(), json!({}));
    }

    #[test]
    fn parse_body_json_passes_through() {
        assert_eq!(
            parse_body(r#"{"err_code":"0"}"#).unwrap(),
            json!({"err_code": "0"})
        );
    }

    #[test]
    fn parse_body_non_json_wraps_raw() {
        // legacy test stubs / gateway callbacks may return plain text — don't fail.
        assert_eq!(parse_body("OK").unwrap(), json!({"raw": "OK"}));
        assert_eq!(
            parse_body("<html>x</html>").unwrap(),
            json!({"raw": "<html>x</html>"})
        );
    }
}
