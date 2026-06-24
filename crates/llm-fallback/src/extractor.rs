use crate::{LlmClient, LlmError};
use serde_json::Value;

const MAX_HTML_LEN: usize = 50_000;

/// Largest byte index `<= max` that is a UTF-8 char boundary of `s` (so slicing
/// `&s[..n]` never panics by splitting a multibyte char).
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub struct Extractor {
    provider: Box<dyn LlmClient>,
}

impl Extractor {
    pub fn new(provider: Box<dyn LlmClient>) -> Self {
        Self { provider }
    }

    pub async fn extract(&self, html: &str, action: &str) -> Result<Value, LlmError> {
        // Truncate on a UTF-8 char boundary: byte-indexing a `str` panics if it
        // splits a multibyte char.
        let truncated = &html[..floor_char_boundary(html, MAX_HTML_LEN)];
        let prompt = format!(
            "You are a web data extraction agent. Given the following HTML content, \
             extract the requested information and return it as JSON.\n\n\
             Action: {action}\n\n\
             HTML:\n{truncated}\n\n\
             Return only valid JSON, no explanation."
        );

        let response = self.provider.complete(&prompt).await?;

        let data: Value =
            serde_json::from_str(&response).map_err(|e| LlmError::ParseError(e.to_string()))?;

        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::floor_char_boundary;

    #[test]
    fn floor_backs_off_inside_multibyte_char() {
        // "a€b" = [0x61, 0xE2,0x82,0xAC, 0x62] — '€' spans bytes 1..4.
        let s = "a€b";
        assert_eq!(floor_char_boundary(s, 2), 1); // inside € → back to 1
        assert_eq!(floor_char_boundary(s, 3), 1);
        assert_eq!(floor_char_boundary(s, 4), 4); // start of 'b'
        assert_eq!(floor_char_boundary(s, 100), s.len());
        // the crux: slicing at the returned index never panics
        let _ = &s[..floor_char_boundary(s, 2)];
    }
}
