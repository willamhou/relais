use crate::LlmError;
use tracing::debug;

/// Fetch the raw HTML of a page via a simple HTTP GET.
///
/// This is the MVP approach that avoids requiring Chrome/Chromium to be
/// installed. For pages that rely heavily on client-side JavaScript rendering,
/// a headless browser (e.g., chromiumoxide) can be used as an optional
/// enhancement.
pub async fn fetch_html(url: &str) -> Result<String, LlmError> {
    debug!(url, "fetching HTML via reqwest");
    let client = reqwest::Client::builder()
        .user_agent("relais/0.1")
        .build()
        .map_err(|e| LlmError::Browser(format!("failed to build HTTP client: {e}")))?;

    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        return Err(LlmError::Browser(format!(
            "HTTP {} when fetching {}",
            resp.status(),
            url
        )));
    }

    let body = resp.text().await?;
    Ok(body)
}
