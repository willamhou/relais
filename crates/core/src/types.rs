use std::collections::HashMap;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub credential_type: AuthType,
    pub data: CredentialData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CredentialData {
    ApiKey {
        token: String,
    },
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        token_type: String,
    },
    Cookie {
        cookies: HashMap<String, String>,
        domain: String,
        captured_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    },
}

impl Credentials {
    /// Create an API key credential.
    pub fn api_key(token: impl Into<String>) -> Self {
        Self {
            credential_type: AuthType::APIKey,
            data: CredentialData::ApiKey {
                token: token.into(),
            },
        }
    }

    /// Create an OAuth credential.
    pub fn oauth(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            credential_type: AuthType::OAuth,
            data: CredentialData::OAuth {
                access_token: access_token.into(),
                refresh_token,
                expires_at,
                token_type: "Bearer".into(),
            },
        }
    }

    /// Get the bearer token string (works for both ApiKey and OAuth).
    pub fn bearer_token(&self) -> Option<&str> {
        match &self.data {
            CredentialData::ApiKey { token } => Some(token),
            CredentialData::OAuth { access_token, .. } => Some(access_token),
            CredentialData::Cookie { .. } => None,
        }
    }

    /// Check if credentials are expired.
    pub fn is_expired(&self) -> bool {
        match &self.data {
            CredentialData::OAuth {
                expires_at: Some(exp),
                ..
            } => *exp < Utc::now(),
            _ => false,
        }
    }

    /// Check if cookie credentials are stale (older than `max_age_hours`).
    ///
    /// Returns `true` when:
    /// - The explicit `expires_at` timestamp is in the past, **or**
    /// - The cookie was captured more than `max_age_hours` ago.
    ///
    /// For non-cookie credentials this always returns `false`.
    pub fn is_cookie_stale(&self, max_age_hours: i64) -> bool {
        match &self.data {
            CredentialData::Cookie {
                captured_at,
                expires_at,
                ..
            } => {
                // Check explicit expiry first.
                if let Some(exp) = expires_at {
                    if *exp < Utc::now() {
                        return true;
                    }
                }
                // Check staleness by capture time.
                let age = Utc::now() - *captured_at;
                age.num_hours() >= max_age_hours
            }
            _ => false,
        }
    }
}
