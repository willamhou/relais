use async_trait::async_trait;
use serde_json::json;

use crate::{LlmClient, LlmError};

pub struct OpenAiClient {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: relais_core::http::client(relais_core::http::Profile::Llm),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "response_format": {"type": "json_object"},
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
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
                "OpenAI API returned {status}: {text}"
            )));
        }

        let data: serde_json::Value = resp.json().await?;

        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                LlmError::ParseError("missing choices[0].message.content in OpenAI response".into())
            })?;

        Ok(content.to_string())
    }
}
