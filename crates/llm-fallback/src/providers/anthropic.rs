use async_trait::async_trait;
use serde_json::json;

use crate::{LlmClient, LlmError};

pub struct AnthropicClient {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": [{"role": "user", "content": prompt}],
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
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
                "Anthropic API returned {status}: {text}"
            )));
        }

        let data: serde_json::Value = resp.json().await?;

        let content = data["content"][0]["text"]
            .as_str()
            .ok_or_else(|| {
                LlmError::ParseError(
                    "missing content[0].text in Anthropic response".into(),
                )
            })?;

        Ok(content.to_string())
    }
}
