use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteManifest {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth_type: AuthType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    OAuth,
    APIKey,
    Cookie,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    pub description: String,
    pub actions: Vec<Action>,
    pub children: Vec<Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub method: Method,
    pub description: String,
    pub params: Value,
    pub returns: Value,
    pub pagination: Option<PaginationStyle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Method {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaginationStyle {
    Cursor,
    Offset { max_limit: u32 },
    PageToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub data: Value,
    pub meta: ResponseMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMeta {
    pub pagination: Option<PaginationInfo>,
    pub rate_limit: Option<RateLimit>,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationInfo {
    pub has_next: bool,
    pub cursor: Option<String>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub remaining: u32,
    pub reset_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExecContext {
    pub site: String,
    pub resource: String,
    pub action: String,
    pub params: Value,
    pub credentials: Option<Credentials>,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub token: String,
    pub token_type: AuthType,
}
