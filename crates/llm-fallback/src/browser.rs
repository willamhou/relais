use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use relais_core::net_guard;
use tracing::debug;

use crate::LlmError;

/// Hard cap on a fetched body, to bound memory against a hostile/huge response.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Cookies plus the domain they were captured for, so they are only ever sent to a
/// matching host.
pub struct CookieScope {
    pub domain: String,
    pub cookies: HashMap<String, String>,
}

/// Fetch the raw HTML of a page via a guarded HTTP GET.
///
/// SSRF defenses (H3): the URL host is resolved and validated against
/// [`net_guard`] (private/loopback/metadata IPs refused); the validated IPs are
/// **pinned** on the client to defeat DNS rebinding; redirects are disabled; and the
/// host must be on the `RELAIS_FALLBACK_ALLOW` allowlist (fail-closed). Imported
/// cookies are attached only when the request host matches their stored domain.
pub async fn fetch_html(url: &str, cookie_scope: Option<&CookieScope>) -> Result<String, LlmError> {
    debug!(url, "fetching HTML via reqwest (guarded)");

    // 1. Egress guard: validate scheme/host and resolve to IPs with no non-public one.
    let target = net_guard::guard_and_resolve(url)
        .map_err(|e| LlmError::Browser(format!("egress blocked: {e}")))?;

    // 2. Host allowlist (fail-closed): the LLM fallback can reach arbitrary URLs, so
    //    it only fetches hosts the operator explicitly allows.
    if !host_allowed(&target.host) {
        return Err(LlmError::Browser(format!(
            "host '{}' is not in RELAIS_FALLBACK_ALLOW; refusing to fetch. Set \
             RELAIS_FALLBACK_ALLOW=host1,host2 to enable the LLM fallback for specific hosts",
            target.host
        )));
    }

    // 3. Pin the validated IPs (anti-rebinding), disable redirects, set a timeout.
    let mut builder = reqwest::Client::builder()
        .user_agent("relais/0.1")
        .redirect(reqwest::redirect::Policy::none())
        // Disable system proxies: a proxy would re-resolve the host itself, bypassing
        // our IP validation + pinning (SSRF). Connect directly to the validated IPs.
        .no_proxy()
        .timeout(Duration::from_secs(30));
    for addr in &target.addrs {
        builder = builder.resolve(&target.host, *addr);
    }
    let client = builder
        .build()
        .map_err(|e| LlmError::Browser(format!("failed to build HTTP client: {e}")))?;

    let mut request = client.get(url);

    // 4. Cookies only when the host matches the cookie's stored domain.
    if let Some(scope) = cookie_scope {
        if net_guard::host_matches_cookie_domain(&target.host, &scope.domain) {
            let cookie_str: String = scope
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            debug!(cookie_count = scope.cookies.len(), "injecting cookies");
            request = request.header("Cookie", cookie_str);
        } else {
            debug!(host = %target.host, domain = %scope.domain, "withholding cookies: host does not match cookie domain");
        }
    }

    let resp = request.send().await?;
    if !resp.status().is_success() {
        // Don't echo the URL (it may carry sensitive query params).
        return Err(LlmError::Browser(format!("HTTP {}", resp.status())));
    }

    // Reject an oversized declared body up front.
    if let Some(len) = resp.content_length() {
        if len > MAX_BODY_BYTES as u64 {
            return Err(LlmError::Browser(format!(
                "response too large: {len} bytes (cap {MAX_BODY_BYTES})"
            )));
        }
    }

    // Stream with a running byte cap so a chunked/undeclared body can't exhaust memory.
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = MAX_BODY_BYTES.saturating_sub(buf.len());
        if chunk.len() >= remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            break; // hit the cap; truncate
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Whether `host` is on `RELAIS_FALLBACK_ALLOW` (comma-separated). Unset/empty ⇒
/// fail-closed (no host allowed).
fn host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    match std::env::var("RELAIS_FALLBACK_ALLOW") {
        Ok(list) => list
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .any(|h| h == host),
        Err(_) => false,
    }
}
