use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, ExecContext, Method, PaginationStyle, Resource,
    Response, ResponseMeta, SiteManifest,
};
use reqwest::Client;
use serde_json::{json, Value};

const BASE_URL: &str = "https://api.github.com";
const USER_AGENT: &str = "relais/0.1.0";

pub struct GitHubAdapter {
    client: Client,
}

impl GitHubAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for GitHubAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for GitHubAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "github".into(),
            name: "GitHub".into(),
            base_url: BASE_URL.into(),
            auth_type: AuthType::APIKey,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![repos_resource()]
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let (method, url) = resolve_endpoint(ctx)?;
        let body = build_request_body(ctx);

        let mut req = match method {
            HttpMethod::Get => self.client.get(&url),
            HttpMethod::Post => self.client.post(&url),
            HttpMethod::Delete => self.client.delete(&url),
        };

        req = req.header("User-Agent", USER_AGENT);
        req = req.header("Accept", "application/vnd.github+json");

        if let Some(creds) = &ctx.credentials {
            req = req.header("Authorization", format!("Bearer {}", creds.token));
        }

        if let Some(body) = body {
            req = req.json(&body);
        }

        let resp = req.send().await?;
        let status = resp.status();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AdapterError::Auth(format!(
                "GitHub API returned {}",
                status
            )));
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AdapterError::NotFound(format!(
                "{}.{} not found",
                ctx.resource, ctx.action
            )));
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
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
                "GitHub API error {}: {}",
                status,
                text
            )));
        }

        let data: Value = resp.json().await?;

        Ok(Response {
            data,
            meta: ResponseMeta {
                pagination: None,
                rate_limit: None,
                cached: false,
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
    Delete,
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

fn resolve_endpoint(ctx: &ExecContext) -> Result<(HttpMethod, String), AdapterError> {
    let p = &ctx.params;

    match (ctx.resource.as_str(), ctx.action.as_str()) {
        // repos
        ("repos", "list") => {
            let owner = param_str(p, "owner");
            let url = match owner {
                Some(o) => format!("{BASE_URL}/users/{o}/repos"),
                None => format!("{BASE_URL}/user/repos"),
            };
            Ok((HttpMethod::Get, url))
        }
        ("repos", "get") => {
            let owner = require_str(p, "owner", "repos.get")?;
            let repo = require_str(p, "repo", "repos.get")?;
            Ok((HttpMethod::Get, format!("{BASE_URL}/repos/{owner}/{repo}")))
        }

        // issues
        ("issues", "list") => {
            let owner = require_str(p, "owner", "issues.list")?;
            let repo = require_str(p, "repo", "issues.list")?;
            Ok((
                HttpMethod::Get,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues"),
            ))
        }
        ("issues", "create") => {
            let owner = require_str(p, "owner", "issues.create")?;
            let repo = require_str(p, "repo", "issues.create")?;
            Ok((
                HttpMethod::Post,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues"),
            ))
        }
        ("issues", "get") => {
            let owner = require_str(p, "owner", "issues.get")?;
            let repo = require_str(p, "repo", "issues.get")?;
            let number = require_u64(p, "issue_number", "issues.get")?;
            Ok((
                HttpMethod::Get,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues/{number}"),
            ))
        }

        // comments
        ("comments", "list") => {
            let owner = require_str(p, "owner", "comments.list")?;
            let repo = require_str(p, "repo", "comments.list")?;
            let number = require_u64(p, "issue_number", "comments.list")?;
            Ok((
                HttpMethod::Get,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues/{number}/comments"),
            ))
        }
        ("comments", "create") => {
            let owner = require_str(p, "owner", "comments.create")?;
            let repo = require_str(p, "repo", "comments.create")?;
            let number = require_u64(p, "issue_number", "comments.create")?;
            Ok((
                HttpMethod::Post,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues/{number}/comments"),
            ))
        }
        ("comments", "delete") => {
            let owner = require_str(p, "owner", "comments.delete")?;
            let repo = require_str(p, "repo", "comments.delete")?;
            let comment_id = require_u64(p, "comment_id", "comments.delete")?;
            Ok((
                HttpMethod::Delete,
                format!("{BASE_URL}/repos/{owner}/{repo}/issues/comments/{comment_id}"),
            ))
        }

        _ => Err(AdapterError::Unsupported(format!(
            "{}.{}",
            ctx.resource, ctx.action
        ))),
    }
}

fn build_request_body(ctx: &ExecContext) -> Option<Value> {
    match (ctx.resource.as_str(), ctx.action.as_str()) {
        ("issues", "create") => {
            let title = ctx.params.get("title").cloned().unwrap_or(Value::Null);
            let body = ctx.params.get("body").cloned().unwrap_or(Value::Null);
            Some(json!({ "title": title, "body": body }))
        }
        ("comments", "create") => {
            let body = ctx.params.get("body").cloned().unwrap_or(Value::Null);
            Some(json!({ "body": body }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parameter helpers
// ---------------------------------------------------------------------------

fn param_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn require_str<'a>(params: &'a Value, key: &str, context: &str) -> Result<&'a str, AdapterError> {
    param_str(params, key).ok_or_else(|| {
        AdapterError::Other(anyhow::anyhow!("missing required param '{key}' for {context}"))
    })
}

fn require_u64(params: &Value, key: &str, context: &str) -> Result<u64, AdapterError> {
    params
        .get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            AdapterError::Other(anyhow::anyhow!(
                "missing required integer param '{key}' for {context}"
            ))
        })
}

// ---------------------------------------------------------------------------
// Resource tree
// ---------------------------------------------------------------------------

fn repos_resource() -> Resource {
    Resource {
        id: "repos".into(),
        description: "GitHub repositories".into(),
        actions: vec![
            Action {
                id: "list".into(),
                method: Method::Read,
                description: "List repositories for a user or the authenticated user".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string", "description": "Username (optional, defaults to authenticated user)" }
                    }
                }),
                returns: json!({
                    "type": "array",
                    "items": { "type": "object" }
                }),
                pagination: Some(PaginationStyle::Cursor),
            },
            Action {
                id: "get".into(),
                method: Method::Read,
                description: "Get a single repository".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" }
                    },
                    "required": ["owner", "repo"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
        ],
        children: vec![issues_resource()],
    }
}

fn issues_resource() -> Resource {
    Resource {
        id: "issues".into(),
        description: "Repository issues".into(),
        actions: vec![
            Action {
                id: "list".into(),
                method: Method::Read,
                description: "List issues for a repository".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" }
                    },
                    "required": ["owner", "repo"]
                }),
                returns: json!({
                    "type": "array",
                    "items": { "type": "object" }
                }),
                pagination: Some(PaginationStyle::Cursor),
            },
            Action {
                id: "create".into(),
                method: Method::Write,
                description: "Create an issue".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "title": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["owner", "repo", "title", "body"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
            Action {
                id: "get".into(),
                method: Method::Read,
                description: "Get a single issue".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "issue_number": { "type": "integer" }
                    },
                    "required": ["owner", "repo", "issue_number"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
        ],
        children: vec![comments_resource()],
    }
}

fn comments_resource() -> Resource {
    Resource {
        id: "comments".into(),
        description: "Issue comments".into(),
        actions: vec![
            Action {
                id: "list".into(),
                method: Method::Read,
                description: "List comments on an issue".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "issue_number": { "type": "integer" }
                    },
                    "required": ["owner", "repo", "issue_number"]
                }),
                returns: json!({
                    "type": "array",
                    "items": { "type": "object" }
                }),
                pagination: Some(PaginationStyle::Cursor),
            },
            Action {
                id: "create".into(),
                method: Method::Write,
                description: "Create a comment on an issue".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "issue_number": { "type": "integer" },
                        "body": { "type": "string" }
                    },
                    "required": ["owner", "repo", "issue_number", "body"]
                }),
                returns: json!({ "type": "object" }),
                pagination: None,
            },
            Action {
                id: "delete".into(),
                method: Method::Delete,
                description: "Delete a comment".into(),
                params: json!({
                    "type": "object",
                    "properties": {
                        "owner": { "type": "string" },
                        "repo": { "type": "string" },
                        "comment_id": { "type": "integer" }
                    },
                    "required": ["owner", "repo", "comment_id"]
                }),
                returns: json!({ "type": "null" }),
                pagination: None,
            },
        ],
        children: vec![],
    }
}
