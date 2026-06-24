//! Adapter for the SCS service (Go/kratos microservice, `account.v1` REST API).
//!
//! Unlike the GitHub/HackerNews adapters which hardcode their `BASE_URL`, SCS is a
//! self-hosted service whose endpoint varies per environment, so the base URL is
//! read from the `SCS_BASE_URL` env var (default `http://127.0.0.1:8000`) at
//! construction time. Use [`ScsAdapter::with_base_url`] in tests to avoid the env var.
use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, ExecContext, Method, PaginationInfo, PaginationStyle,
    Resource, Response, ResponseMeta, SiteManifest,
};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000";

/// Max page size advertised for `accounts.list`.
const MAX_PAGE_SIZE: u32 = 100;

pub struct ScsAdapter {
    client: Client,
    base_url: String,
}

impl ScsAdapter {
    /// Construct an adapter, reading the base URL from `SCS_BASE_URL`
    /// (falling back to [`DEFAULT_BASE_URL`]).
    pub fn new() -> Self {
        Self::with_base_url(
            std::env::var("SCS_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
        )
    }

    /// Construct an adapter with an explicit base URL (no env lookup).
    ///
    /// Trailing slashes are stripped so endpoint joins never produce `//v1/...`.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            client: relais_core::http::client(relais_core::http::Profile::Default),
            base_url,
        }
    }
}

impl Default for ScsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for ScsAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "scs-v2".into(),
            name: "SCS (kratos v2)".into(),
            base_url: self.base_url.clone(),
            auth_type: AuthType::APIKey,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![accounts_resource()]
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let (method, url) =
            resolve_endpoint(&self.base_url, &ctx.resource, &ctx.action, &ctx.params)?;

        let mut req = match method {
            HttpMethod::Get => self.client.get(&url),
            HttpMethod::Post => self.client.post(&url),
            HttpMethod::Put => self.client.put(&url),
            HttpMethod::Delete => self.client.delete(&url),
        };

        // List supports offset pagination + an optional type filter via query string.
        if ctx.resource == "accounts" && ctx.action == "list" {
            req = req.query(&build_query(&ctx.params));
        }

        if let Some(body) = build_request_body(&ctx.resource, &ctx.action, &ctx.params) {
            req = req.json(&body);
        }

        // SCS account service has no auth today; we still inject the acs_token as a
        // Bearer credential when present so the adapter is ready once auth lands.
        // NOTE: confirm the header name SCS expects when it enables auth.
        if let Some(creds) = &ctx.credentials {
            if let Some(token) = creds.bearer_token() {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
        }

        let resp = req.send().await?;
        let status = resp.status();

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(AdapterError::Auth(format!("SCS returned {status}")));
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
            // 400/409/422 and other errors: preserve the kratos error body so callers
            // can debug validation/conflict failures.
            let text = resp.text().await.unwrap_or_default();
            return Err(AdapterError::Other(anyhow::anyhow!(
                "SCS error {status}: {text}"
            )));
        }

        let text = resp.text().await.unwrap_or_default();
        let data = parse_body(&text)?;

        let pagination = if ctx.resource == "accounts" && ctx.action == "list" {
            Some(list_pagination(&ctx.params, &data))
        } else {
            None
        };

        Ok(Response {
            data,
            meta: ResponseMeta {
                pagination,
                rate_limit: None,
                cached: false,
                receipt: None,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Internal HTTP method enum (distinct from relais_core::Method)
// ---------------------------------------------------------------------------

enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

fn resolve_endpoint(
    base_url: &str,
    resource: &str,
    action: &str,
    params: &Value,
) -> Result<(HttpMethod, String), AdapterError> {
    match (resource, action) {
        ("accounts", "list") => Ok((HttpMethod::Get, format!("{base_url}/v1/accounts"))),
        ("accounts", "create") => Ok((HttpMethod::Post, format!("{base_url}/v1/accounts"))),
        ("accounts", "get") => {
            let id = require_id(params, "accounts.get")?;
            Ok((HttpMethod::Get, format!("{base_url}/v1/accounts/{id}")))
        }
        ("accounts", "update") => {
            let id = require_id(params, "accounts.update")?;
            Ok((HttpMethod::Put, format!("{base_url}/v1/accounts/{id}")))
        }
        ("accounts", "delete") => {
            let id = require_id(params, "accounts.delete")?;
            Ok((HttpMethod::Delete, format!("{base_url}/v1/accounts/{id}")))
        }
        _ => Err(AdapterError::Unsupported(format!("{resource}.{action}"))),
    }
}

fn build_request_body(resource: &str, action: &str, params: &Value) -> Option<Value> {
    match (resource, action) {
        ("accounts", "create") | ("accounts", "update") => {
            let mut body = serde_json::Map::new();
            for key in ["name", "phone", "type"] {
                if let Some(v) = params.get(key) {
                    if !v.is_null() {
                        body.insert(key.to_string(), v.clone());
                    }
                }
            }
            Some(Value::Object(body))
        }
        _ => None,
    }
}

/// Collect the `page` / `page_size` / `type` query params for `accounts.list`.
fn build_query(params: &Value) -> Vec<(String, String)> {
    let mut q = Vec::new();
    for key in ["page", "page_size", "type"] {
        if let Some(v) = params.get(key) {
            if let Some(s) = scalar_to_string(v) {
                q.push((key.to_string(), s));
            }
        }
    }
    q
}

fn list_pagination(params: &Value, data: &Value) -> PaginationInfo {
    let page = params.get("page").and_then(parse_loose_i64).unwrap_or(1);
    let page_size = params
        .get("page_size")
        .and_then(parse_loose_i64)
        .unwrap_or(0);
    // protobuf JSON serializes int64 (`total`) as a string. If it is missing or
    // unparseable, report `None` rather than masquerading a real count of 0.
    let total = data.get("total").and_then(parse_loose_i64);
    PaginationInfo {
        has_next: total.is_some_and(|t| compute_has_next(page, page_size, t)),
        cursor: None,
        total: total.map(|t| t.max(0) as u64),
    }
}

fn compute_has_next(page: i64, page_size: i64, total: i64) -> bool {
    if page_size <= 0 || page <= 0 {
        return false;
    }
    page * page_size < total
}

/// Parse an i64 from a JSON number or string (protobuf JSON emits int64 as string).
fn parse_loose_i64(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    v.as_str().and_then(|s| s.parse::<i64>().ok())
}

/// Render a JSON scalar to a query-string value.
fn scalar_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Parse a response body, tolerating empty bodies (e.g. a 200/204 with no content).
fn parse_body(text: &str) -> Result<Value, AdapterError> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text)
        .map_err(|e| AdapterError::Other(anyhow::anyhow!("invalid SCS response body: {e}")))
}

fn require_id(params: &Value, context: &str) -> Result<i64, AdapterError> {
    match params.get("id") {
        None | Some(Value::Null) => Err(AdapterError::Other(anyhow::anyhow!(
            "missing required param 'id' for {context}"
        ))),
        Some(v) => match parse_loose_i64(v) {
            // SCS rejects id <= 0 with InvalidArgument; reject locally for a clearer
            // error and to avoid building a nonsensical `/v1/accounts/-1` URL.
            Some(n) if n > 0 => Ok(n),
            _ => Err(AdapterError::Other(anyhow::anyhow!(
                "param 'id' must be a positive integer for {context}"
            ))),
        },
    }
}

// ---------------------------------------------------------------------------
// Resource tree
// ---------------------------------------------------------------------------

fn accounts_resource() -> Resource {
    let account_type = json!({
        "type": "integer",
        "description": "AccountType: 0=unspecified, 1=center, 2=supplier, 3=distributor"
    });

    Resource {
        id: "accounts".into(),
        description: "SCS accounts".into(),
        actions: vec![
            Action {
                id: "list".into(),
                method: Method::Read,
                description: "List accounts (paginated)".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "page": { "type": "integer", "description": "1-based page number" },
                        "page_size": { "type": "integer", "description": "items per page" },
                        "type": account_type,
                    }
                }),
                returns: json!({
                    "type": "object",
                    "properties": {
                        "accounts": { "type": "array", "items": { "type": "object" } },
                        "total": { "type": "string", "description": "total count (int64 as string)" }
                    }
                }),
                pagination: Some(PaginationStyle::Offset {
                    max_limit: MAX_PAGE_SIZE,
                }),
            },
            Action {
                id: "get".into(),
                method: Method::Read,
                description: "Get a single account by id".into(),
                params: json!({
                    "type": "object",
                    "properties": { "id": { "type": "integer" } },
                    "required": ["id"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
            Action {
                id: "create".into(),
                method: Method::Write,
                description: "Create an account".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "phone": { "type": "string" },
                        "type": account_type,
                    },
                    "required": ["name"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
            Action {
                id: "update".into(),
                method: Method::Write,
                description: "Update an account (full replace: SCS overwrites name/phone/type \
                    with the request values, so omitted fields are reset to their zero value)"
                    .into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "phone": { "type": "string" },
                        "type": account_type,
                    },
                    "required": ["id"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
            Action {
                id: "delete".into(),
                method: Method::Delete,
                description: "Delete an account by id".into(),
                params: json!({
                    "type": "object",
                    "properties": { "id": { "type": "integer" } },
                    "required": ["id"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
        ],
        children: vec![],
    }
}

#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BASE: &str = "http://example.test:8000";

    fn url(method_url: (HttpMethod, String)) -> String {
        method_url.1
    }

    #[test]
    fn resolve_list_is_get_accounts() {
        let (m, u) = resolve_endpoint(BASE, "accounts", "list", &json!({})).unwrap();
        assert!(matches!(m, HttpMethod::Get));
        assert_eq!(u, "http://example.test:8000/v1/accounts");
    }

    #[test]
    fn resolve_get_uses_id_in_path() {
        let r = resolve_endpoint(BASE, "accounts", "get", &json!({"id": 5})).unwrap();
        assert!(matches!(r.0, HttpMethod::Get));
        assert_eq!(url(r), "http://example.test:8000/v1/accounts/5");
    }

    #[test]
    fn resolve_get_accepts_string_id() {
        // protobuf JSON serializes int64 as a string.
        let r = resolve_endpoint(BASE, "accounts", "get", &json!({"id": "42"})).unwrap();
        assert_eq!(url(r), "http://example.test:8000/v1/accounts/42");
    }

    #[test]
    fn resolve_get_missing_id_errors() {
        assert!(resolve_endpoint(BASE, "accounts", "get", &json!({})).is_err());
    }

    #[test]
    fn resolve_create_is_post() {
        let r = resolve_endpoint(BASE, "accounts", "create", &json!({})).unwrap();
        assert!(matches!(r.0, HttpMethod::Post));
        assert_eq!(url(r), "http://example.test:8000/v1/accounts");
    }

    #[test]
    fn resolve_update_is_put_with_id() {
        let r = resolve_endpoint(BASE, "accounts", "update", &json!({"id": 7})).unwrap();
        assert!(matches!(r.0, HttpMethod::Put));
        assert_eq!(url(r), "http://example.test:8000/v1/accounts/7");
    }

    #[test]
    fn resolve_delete_is_delete_with_id() {
        let r = resolve_endpoint(BASE, "accounts", "delete", &json!({"id": 7})).unwrap();
        assert!(matches!(r.0, HttpMethod::Delete));
        assert_eq!(url(r), "http://example.test:8000/v1/accounts/7");
    }

    #[test]
    fn resolve_unknown_action_errors() {
        assert!(resolve_endpoint(BASE, "accounts", "bogus", &json!({})).is_err());
        assert!(resolve_endpoint(BASE, "bogus", "list", &json!({})).is_err());
    }

    #[test]
    fn build_query_collects_pagination_and_type() {
        let q = build_query(&json!({"page": 2, "page_size": 20, "type": 1}));
        assert!(q.contains(&("page".to_string(), "2".to_string())));
        assert!(q.contains(&("page_size".to_string(), "20".to_string())));
        assert!(q.contains(&("type".to_string(), "1".to_string())));
    }

    #[test]
    fn build_query_empty_when_no_params() {
        assert!(build_query(&json!({})).is_empty());
    }

    #[test]
    fn has_next_true_when_more_pages() {
        assert!(compute_has_next(1, 20, 100));
    }

    #[test]
    fn has_next_false_on_last_page() {
        assert!(!compute_has_next(5, 20, 100));
        assert!(!compute_has_next(1, 20, 0));
    }

    #[test]
    fn parse_loose_i64_handles_number_and_string() {
        assert_eq!(parse_loose_i64(&json!(42)), Some(42));
        assert_eq!(parse_loose_i64(&json!("42")), Some(42));
        assert_eq!(parse_loose_i64(&json!("abc")), None);
        assert_eq!(parse_loose_i64(&json!(null)), None);
    }

    #[test]
    fn parse_body_empty_returns_empty_object() {
        assert_eq!(parse_body("").unwrap(), json!({}));
        assert_eq!(parse_body("   ").unwrap(), json!({}));
    }

    #[test]
    fn parse_body_parses_json() {
        assert_eq!(parse_body(r#"{"a":1}"#).unwrap(), json!({"a": 1}));
    }

    #[test]
    fn build_request_body_only_includes_present_fields() {
        let body = build_request_body("accounts", "create", &json!({"name": "Acme"})).unwrap();
        assert_eq!(body, json!({"name": "Acme"}));
    }

    #[test]
    fn list_pagination_parses_string_total() {
        let p = list_pagination(
            &json!({"page": 1, "page_size": 20}),
            &json!({"total": "100"}),
        );
        assert_eq!(p.total, Some(100));
        assert!(p.has_next);
    }

    #[test]
    fn list_pagination_unparseable_total_is_none() {
        let p = list_pagination(
            &json!({"page": 1, "page_size": 20}),
            &json!({"total": "bad"}),
        );
        assert_eq!(p.total, None);
        assert!(!p.has_next);
    }

    #[test]
    fn list_pagination_missing_total_is_none() {
        let p = list_pagination(&json!({"page": 1, "page_size": 20}), &json!({}));
        assert_eq!(p.total, None);
    }

    #[test]
    fn resolve_get_rejects_non_positive_id() {
        assert!(resolve_endpoint(BASE, "accounts", "get", &json!({"id": 0})).is_err());
        assert!(resolve_endpoint(BASE, "accounts", "get", &json!({"id": -1})).is_err());
    }

    #[test]
    fn with_base_url_strips_trailing_slash() {
        let a = ScsAdapter::with_base_url("http://x:8000/");
        assert_eq!(a.base_url, "http://x:8000");
    }

    #[test]
    fn new_uses_default_when_env_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCS_BASE_URL");
        let a = ScsAdapter::new();
        assert_eq!(a.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn new_reads_env_when_present() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("SCS_BASE_URL", "http://from-env:9000");
        let a = ScsAdapter::new();
        assert_eq!(a.base_url, "http://from-env:9000");
        std::env::remove_var("SCS_BASE_URL");
    }
}
