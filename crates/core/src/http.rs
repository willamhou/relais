//! Shared outbound HTTP client builder with sane timeouts (always compiled).
//!
//! Every adapter/provider/token-refresh path should build its `reqwest::Client`
//! here so a stalled upstream fails by timeout instead of pinning a task forever.

use std::time::Duration;

/// Timeout profile. Most upstreams are quick API calls; LLM-provider completions are
/// legitimately slow, so they get a longer ceiling.
#[derive(Debug, Clone, Copy)]
pub enum Profile {
    /// Adapters, OAuth token exchange, token refresh.
    Default,
    /// LLM provider completions (slow, but still bounded).
    Llm,
}

/// Build a `reqwest::Client` with connect + total timeouts for the given profile.
pub fn client(profile: Profile) -> reqwest::Client {
    let (connect_secs, total_secs) = match profile {
        Profile::Default => (5, 30),
        Profile::Llm => (10, 180),
    };
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(total_secs))
        .user_agent("relais/0.1")
        .build()
        // Building only fails on TLS-backend/resolver init, which is fatal config —
        // fail closed at startup rather than silently returning a no-timeout client.
        .expect("failed to build the HTTP client (TLS backend init?)")
}
