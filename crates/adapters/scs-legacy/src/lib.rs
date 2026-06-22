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
