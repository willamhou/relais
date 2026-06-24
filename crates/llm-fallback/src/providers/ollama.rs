use async_trait::async_trait;
use serde_json::json;

use crate::{LlmClient, LlmError};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

pub struct OllamaClient {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaClient {
    pub fn new(base_url: String, model: String) -> Self {
        let base_url = if base_url.is_empty() {
            DEFAULT_BASE_URL.to_string()
        } else {
            base_url
        };

        Self {
            base_url,
            model,
            client: relais_core::http::client(relais_core::http::Profile::Llm),
        }
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let url = format!("{}/api/generate", self.base_url);
        let body = json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "format": "json",
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp
                .text()
                .await
                .unwrap_or_else(|_| "unable to read body".into());
            return Err(LlmError::Provider(format!(
                "Ollama API returned {status}: {text}"
            )));
        }

        let data: serde_json::Value = resp.json().await?;

        let content = data["response"]
            .as_str()
            .ok_or_else(|| {
                LlmError::ParseError("missing 'response' field in Ollama response".into())
            })?;

        Ok(content.to_string())
    }
}
