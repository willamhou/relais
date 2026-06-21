use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, ExecContext, Method, PaginationStyle, Resource,
    Response, ResponseMeta, SiteManifest,
};
use reqwest::Client;
use serde_json::{json, Value};
use tracing::debug;

const BASE_URL: &str = "https://hacker-news.firebaseio.com/v0";
const DEFAULT_LIMIT: usize = 30;

pub struct HackerNewsAdapter {
    client: Client,
}

impl HackerNewsAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    async fn fetch_json(&self, url: &str) -> Result<Value, AdapterError> {
        debug!(url, "fetching from HN API");
        let resp = self.client.get(url).send().await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AdapterError::NotFound(url.to_string()));
        }

        let body: Value = resp.json().await?;
        Ok(body)
    }

    async fn fetch_story_ids(&self, endpoint: &str, limit: usize) -> Result<Response, AdapterError> {
        let url = format!("{}/{}.json", BASE_URL, endpoint);
        let ids = self.fetch_json(&url).await?;

        let ids = match ids.as_array() {
            Some(arr) => arr.iter().take(limit).cloned().collect::<Vec<_>>(),
            None => {
                return Err(AdapterError::Other(anyhow::anyhow!(
                    "expected array of story IDs from HN API"
                )));
            }
        };

        let total = ids.len() as u64;

        Ok(Response {
            data: Value::Array(ids),
            meta: ResponseMeta {
                pagination: Some(relais_core::PaginationInfo {
                    has_next: false,
                    cursor: None,
                    total: Some(total),
                }),
                rate_limit: None,
                cached: false,
                receipt: None,
            },
        })
    }

    async fn fetch_item(&self, id: u64) -> Result<Value, AdapterError> {
        let url = format!("{}/item/{}.json", BASE_URL, id);
        let item = self.fetch_json(&url).await?;
        if item.is_null() {
            return Err(AdapterError::NotFound(format!("item {}", id)));
        }
        Ok(item)
    }
}

impl Default for HackerNewsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for HackerNewsAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "hackernews".into(),
            name: "Hacker News".into(),
            base_url: BASE_URL.into(),
            auth_type: AuthType::None,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let offset_pagination = Some(PaginationStyle::Offset { max_limit: 500 });

        let comments_resource = Resource {
            id: "comments".into(),
            description: "Comments on a story".into(),
            actions: vec![Action {
                id: "list".into(),
                method: Method::Read,
                description: "List comments for a story".into(),
                params: json!({"story_id": "integer"}),
                returns: json!({"type": "array", "items": "comment"}),
                pagination: offset_pagination.clone(),
            }],
            children: vec![],
        };

        let stories_resource = Resource {
            id: "stories".into(),
            description: "Hacker News stories".into(),
            actions: vec![
                Action {
                    id: "list_top".into(),
                    method: Method::Read,
                    description: "List top stories".into(),
                    params: json!({"limit": "integer"}),
                    returns: json!({"type": "array", "items": "story_id"}),
                    pagination: offset_pagination.clone(),
                },
                Action {
                    id: "list_new".into(),
                    method: Method::Read,
                    description: "List newest stories".into(),
                    params: json!({"limit": "integer"}),
                    returns: json!({"type": "array", "items": "story_id"}),
                    pagination: offset_pagination.clone(),
                },
                Action {
                    id: "list_best".into(),
                    method: Method::Read,
                    description: "List best stories".into(),
                    params: json!({"limit": "integer"}),
                    returns: json!({"type": "array", "items": "story_id"}),
                    pagination: offset_pagination,
                },
                Action {
                    id: "get".into(),
                    method: Method::Read,
                    description: "Get a single story by ID".into(),
                    params: json!({"id": "integer"}),
                    returns: json!({"type": "object", "description": "story item"}),
                    pagination: None,
                },
            ],
            children: vec![comments_resource],
        };

        vec![stories_resource]
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        match (ctx.resource.as_str(), ctx.action.as_str()) {
            ("stories", "list_top") => {
                let limit = parse_limit(&ctx.params);
                self.fetch_story_ids("topstories", limit).await
            }
            ("stories", "list_new") => {
                let limit = parse_limit(&ctx.params);
                self.fetch_story_ids("newstories", limit).await
            }
            ("stories", "list_best") => {
                let limit = parse_limit(&ctx.params);
                self.fetch_story_ids("beststories", limit).await
            }
            ("stories", "get") => {
                let id = ctx.params["id"]
                    .as_u64()
                    .ok_or_else(|| AdapterError::NotFound("missing required param: id".into()))?;
                let item = self.fetch_item(id).await?;
                Ok(Response {
                    data: item,
                    meta: ResponseMeta {
                        pagination: None,
                        rate_limit: None,
                        cached: false,
                        receipt: None,
                    },
                })
            }
            ("comments", "list") => {
                let story_id = ctx.params["story_id"].as_u64().ok_or_else(|| {
                    AdapterError::NotFound("missing required param: story_id".into())
                })?;

                let story = self.fetch_item(story_id).await?;

                let kid_ids = story["kids"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();

                let mut comments = Vec::with_capacity(kid_ids.len());
                for kid_value in &kid_ids {
                    if let Some(kid_id) = kid_value.as_u64() {
                        match self.fetch_item(kid_id).await {
                            Ok(comment) => comments.push(comment),
                            Err(e) => {
                                debug!(kid_id, error = %e, "failed to fetch comment, skipping");
                            }
                        }
                    }
                }

                let total = comments.len() as u64;

                Ok(Response {
                    data: Value::Array(comments),
                    meta: ResponseMeta {
                        pagination: Some(relais_core::PaginationInfo {
                            has_next: false,
                            cursor: None,
                            total: Some(total),
                        }),
                        rate_limit: None,
                        cached: false,
                        receipt: None,
                    },
                })
            }
            _ => Err(AdapterError::Unsupported(format!(
                "{}.{}",
                ctx.resource, ctx.action
            ))),
        }
    }
}

fn parse_limit(params: &Value) -> usize {
    params["limit"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_LIMIT)
}
