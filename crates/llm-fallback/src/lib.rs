pub mod browser;
pub mod extractor;
pub mod providers;

use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, CredentialData, ExecContext, Method, Resource,
    Response, ResponseMeta, SiteManifest,
};
use serde_json::json;
use tracing::debug;

use crate::browser::fetch_html;
use crate::extractor::Extractor;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("LLM response parse error: {0}")]
    ParseError(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("browser error: {0}")]
    Browser(String),
}

// ---------------------------------------------------------------------------
// LlmClient trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError>;
}

// ---------------------------------------------------------------------------
// Provider configuration enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum LlmProvider {
    OpenAI { api_key: String, model: String },
    Anthropic { api_key: String, model: String },
    Ollama { base_url: String, model: String },
}

// ---------------------------------------------------------------------------
// LlmFallbackAdapter
// ---------------------------------------------------------------------------

pub struct LlmFallbackAdapter {
    extractor: Extractor,
}

impl LlmFallbackAdapter {
    pub fn new(provider: Box<dyn LlmClient>) -> Self {
        Self {
            extractor: Extractor::new(provider),
        }
    }

    /// Build an adapter from a provider configuration enum.
    pub fn from_provider(provider: LlmProvider) -> Self {
        let client: Box<dyn LlmClient> = match provider {
            LlmProvider::OpenAI { api_key, model } => {
                Box::new(providers::OpenAiClient::new(api_key, model))
            }
            LlmProvider::Anthropic { api_key, model } => {
                Box::new(providers::AnthropicClient::new(api_key, model))
            }
            LlmProvider::Ollama { base_url, model } => {
                Box::new(providers::OllamaClient::new(base_url, model))
            }
        };
        Self::new(client)
    }
}

#[async_trait]
impl Adapter for LlmFallbackAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "web".into(),
            name: "Web (LLM Fallback)".into(),
            base_url: "https://any".into(),
            auth_type: AuthType::None,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource {
            id: "pages".into(),
            description: "Arbitrary web pages extracted via LLM".into(),
            actions: vec![Action {
                id: "read".into(),
                method: Method::Read,
                description: "Fetch a web page and extract structured data using an LLM".into(),
                params: json!({
                    "url": "string (required) – the page URL to fetch",
                    "action": "string (required) – what data to extract"
                }),
                returns: json!({"type": "object", "description": "LLM-extracted JSON"}),
                pagination: None,
            }],
            children: vec![],
        }]
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        match (ctx.resource.as_str(), ctx.action.as_str()) {
            ("pages", "read") => {
                let url = ctx.params["url"]
                    .as_str()
                    .ok_or_else(|| AdapterError::NotFound("missing required param: url".into()))?;

                let action = ctx.params["action"]
                    .as_str()
                    .ok_or_else(|| {
                        AdapterError::NotFound("missing required param: action".into())
                    })?;

                debug!(url, action, "LLM fallback: fetching and extracting");

                // Extract cookies from credentials if present.
                let cookies = ctx.credentials.as_ref().and_then(|cred| match &cred.data {
                    CredentialData::Cookie { cookies, .. } => Some(cookies),
                    _ => None,
                });

                let html = fetch_html(url, cookies).await.map_err(|e| {
                    AdapterError::Other(anyhow::anyhow!("failed to fetch HTML: {e}"))
                })?;

                let data = self.extractor.extract(&html, action).await.map_err(|e| {
                    AdapterError::Other(anyhow::anyhow!("LLM extraction failed: {e}"))
                })?;

                Ok(Response {
                    data,
                    meta: ResponseMeta {
                        pagination: None,
                        rate_limit: None,
                        cached: false,
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
