use crate::{LlmClient, LlmError};
use serde_json::Value;

const MAX_HTML_LEN: usize = 50_000;

pub struct Extractor {
    provider: Box<dyn LlmClient>,
}

impl Extractor {
    pub fn new(provider: Box<dyn LlmClient>) -> Self {
        Self { provider }
    }

    pub async fn extract(&self, html: &str, action: &str) -> Result<Value, LlmError> {
        let truncated = &html[..html.len().min(MAX_HTML_LEN)];
        let prompt = format!(
            "You are a web data extraction agent. Given the following HTML content, \
             extract the requested information and return it as JSON.\n\n\
             Action: {action}\n\n\
             HTML:\n{truncated}\n\n\
             Return only valid JSON, no explanation."
        );

        let response = self.provider.complete(&prompt).await?;

        let data: Value = serde_json::from_str(&response)
            .map_err(|e| LlmError::ParseError(e.to_string()))?;

        Ok(data)
    }
}
