use serde::{Deserialize, Serialize};

/// Configuration for an OAuth 2.0 authorization code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// Built-in OAuth configuration for GitHub.
///
/// Reads `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` from environment
/// variables. Returns `None` if either is missing.
pub fn github_oauth_config() -> Option<OAuthConfig> {
    let client_id = std::env::var("GITHUB_CLIENT_ID").ok()?;
    let client_secret = std::env::var("GITHUB_CLIENT_SECRET").ok()?;
    Some(OAuthConfig {
        client_id,
        client_secret,
        auth_url: "https://github.com/login/oauth/authorize".into(),
        token_url: "https://github.com/login/oauth/access_token".into(),
        redirect_uri: "http://127.0.0.1:9876/callback".into(),
        scopes: vec!["repo".into(), "read:user".into()],
    })
}

/// Get OAuth config for a known provider by name.
///
/// Currently supported providers: `"github"`.
pub fn provider_config(provider: &str) -> Option<OAuthConfig> {
    match provider {
        "github" => github_oauth_config(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn github_config_returns_none_without_env_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("GITHUB_CLIENT_ID");
        std::env::remove_var("GITHUB_CLIENT_SECRET");

        assert!(github_oauth_config().is_none());
    }

    #[test]
    fn github_config_returns_none_with_partial_env_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GITHUB_CLIENT_ID", "test-id");
        std::env::remove_var("GITHUB_CLIENT_SECRET");

        let result = github_oauth_config();

        // Clean up.
        std::env::remove_var("GITHUB_CLIENT_ID");

        assert!(result.is_none());
    }

    #[test]
    fn github_config_returns_some_with_both_env_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GITHUB_CLIENT_ID", "test-id");
        std::env::set_var("GITHUB_CLIENT_SECRET", "test-secret");

        let config = github_oauth_config().expect("should return config");

        // Clean up before assertions so env is restored even on panic.
        std::env::remove_var("GITHUB_CLIENT_ID");
        std::env::remove_var("GITHUB_CLIENT_SECRET");

        assert_eq!(config.client_id, "test-id");
        assert_eq!(config.client_secret, "test-secret");
        assert_eq!(config.auth_url, "https://github.com/login/oauth/authorize");
        assert_eq!(
            config.token_url,
            "https://github.com/login/oauth/access_token"
        );
        assert_eq!(config.redirect_uri, "http://127.0.0.1:9876/callback");
        assert_eq!(config.scopes, vec!["repo", "read:user"]);
    }

    #[test]
    fn provider_config_returns_none_for_unknown_provider() {
        assert!(provider_config("unknown").is_none());
    }

    #[test]
    fn oauth_config_round_trips_through_serde() {
        let config = OAuthConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            redirect_uri: "http://localhost:9876/callback".into(),
            scopes: vec!["read".into(), "write".into()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OAuthConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.client_id, "id");
        assert_eq!(deserialized.scopes.len(), 2);
    }
}
