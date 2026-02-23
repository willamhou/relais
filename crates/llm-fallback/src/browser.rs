use std::collections::HashMap;

use crate::LlmError;
use tracing::debug;

/// Fetch the raw HTML of a page via a simple HTTP GET, optionally with cookies.
///
/// This is the MVP approach that avoids requiring Chrome/Chromium to be
/// installed. For pages that rely heavily on client-side JavaScript rendering,
/// a headless browser (e.g., chromiumoxide) can be used as an optional
/// enhancement.
///
/// When `cookies` is `Some`, the key-value pairs are serialised into a single
/// `Cookie` header and attached to the request. This allows authenticated
/// fetches using session cookies imported via `relais auth import-cookies`.
pub async fn fetch_html(
    url: &str,
    cookies: Option<&HashMap<String, String>>,
) -> Result<String, LlmError> {
    debug!(url, "fetching HTML via reqwest");
    let client = reqwest::Client::builder()
        .user_agent("relais/0.1")
        .build()
        .map_err(|e| LlmError::Browser(format!("failed to build HTTP client: {e}")))?;

    let mut request = client.get(url);

    if let Some(cookies) = cookies {
        let cookie_str: String = cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        debug!(cookie_count = cookies.len(), "injecting cookies into request");
        request = request.header("Cookie", cookie_str);
    }

    let resp = request.send().await?;

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
