use std::collections::HashMap;

use anyhow::{bail, Result};
use axum::extract::Query;
use serde::Deserialize;

use relais_core::oauth::OAuthConfig;

use crate::AuthAction;

/// Query parameters received on the OAuth callback endpoint.
#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

pub async fn run(action: AuthAction) -> Result<()> {
    match action {
        AuthAction::Login { provider } => oauth_login(&provider).await,
        AuthAction::Custom {
            auth_url,
            token_url,
            client_id,
            client_secret,
            client_secret_file,
            site,
            scopes,
        } => {
            let client_secret = super::read_secret(
                client_secret,
                client_secret_file.as_deref(),
                "OAuth client secret",
            )?;
            let scope_list: Vec<String> = scopes
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let config = OAuthConfig {
                client_id,
                client_secret,
                auth_url,
                token_url,
                redirect_uri: "http://127.0.0.1:9876/callback".into(),
                scopes: scope_list,
            };
            run_oauth_flow(&config, &site).await
        }
        AuthAction::ImportCookies {
            site,
            domain,
            cookies,
            cookies_file,
        } => {
            let cookies = super::read_secret(cookies, cookies_file.as_deref(), "cookies")?;
            import_cookies(&site, &domain, &cookies)
        }
    }
}

/// Run the OAuth browser flow for a known provider.
async fn oauth_login(provider: &str) -> Result<()> {
    let config = relais_core::oauth::provider_config(provider).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider '{}'. Set {}_CLIENT_ID and {}_CLIENT_SECRET env vars, or use 'auth custom'.",
            provider,
            provider.to_uppercase(),
            provider.to_uppercase(),
        )
    })?;

    run_oauth_flow(&config, provider).await
}

/// Execute the full OAuth 2.0 authorization code flow:
///
/// 1. Generate a random `state` parameter for CSRF protection.
/// 2. Build the authorization URL.
/// 3. Start a local callback server on 127.0.0.1:9876.
/// 4. Open the authorization URL in the user's default browser.
/// 5. Wait for the callback (with a 120-second timeout).
/// 6. Exchange the authorization code for an access token.
/// 7. Store the resulting credential in the vault.
async fn run_oauth_flow(config: &OAuthConfig, site_id: &str) -> Result<()> {
    // 1. Generate random state parameter (32-char hex string).
    let state: String = {
        let bytes: [u8; 16] = rand::random();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    };

    // 2. Build authorization URL.
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&scope={}&state={}",
        config.auth_url,
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&config.redirect_uri),
        urlencoding::encode(&config.scopes.join(" ")),
        &state,
    );

    // 3. Start local callback server.
    println!("Opening browser for authentication...");
    println!("If the browser doesn't open, visit:\n  {auth_url}");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let expected_state = state.clone();

    // Wrap the sender in an Arc<Mutex> so the closure is Clone (required by axum).
    let tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let callback_app = axum::Router::new().route(
        "/callback",
        axum::routing::get(move |query: Query<CallbackParams>| {
            let tx = tx.clone();
            async move {
                if query.state != expected_state {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "Error: state mismatch. This may be a CSRF attack.".to_string(),
                    );
                }
                if let Some(sender) = tx.lock().await.take() {
                    let _ = sender.send(query.code.clone());
                }
                (
                    axum::http::StatusCode::OK,
                    "Authentication successful! You can close this tab.".to_string(),
                )
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:9876").await?;

    // 4. Open browser.
    if let Err(e) = open::that(&auth_url) {
        eprintln!("Warning: could not open browser automatically: {e}");
        println!("Please open the URL above manually.");
    }

    // 5. Serve until callback received (with timeout).
    let server = axum::serve(listener, callback_app);

    let code = tokio::select! {
        result = rx => result?,
        _ = tokio::time::sleep(std::time::Duration::from_secs(120)) => {
            bail!("Authentication timed out after 120 seconds");
        }
        result = server => {
            result?;
            bail!("Server stopped unexpectedly");
        }
    };

    // 6. Exchange code for tokens.
    let client = relais_core::http::client(relais_core::http::Profile::Default);
    let token_response = client
        .post(&config.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
        ])
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let access_token = token_response["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in response: {token_response}"))?;

    let refresh_token = token_response["refresh_token"].as_str().map(String::from);
    let expires_in = token_response["expires_in"].as_u64();
    let expires_at =
        expires_in.map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs as i64));

    // 7. Store in vault.
    let cred = relais_core::Credentials::oauth(access_token, refresh_token, expires_at);
    let vault = super::open_vault()?;
    vault.store(site_id, &serde_json::to_string(&cred)?)?;

    println!("Successfully authenticated with {site_id}!");
    println!("Credential stored in vault.");
    Ok(())
}

/// Parse a cookie string and store it in the vault as a Cookie credential.
fn import_cookies(site: &str, domain: &str, cookies_str: &str) -> Result<()> {
    let mut cookies = HashMap::new();
    for pair in cookies_str.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        if let Some((name, value)) = pair.split_once('=') {
            cookies.insert(name.trim().to_string(), value.trim().to_string());
        } else {
            bail!("Invalid cookie format: '{pair}'. Expected name=value.");
        }
    }

    if cookies.is_empty() {
        bail!("No cookies parsed from input.");
    }

    let cred = relais_core::Credentials {
        credential_type: relais_core::AuthType::Cookie,
        data: relais_core::CredentialData::Cookie {
            cookies,
            domain: domain.to_string(),
            captured_at: chrono::Utc::now(),
            expires_at: None,
        },
    };

    let vault = super::open_vault()?;
    vault.store(site, &serde_json::to_string(&cred)?)?;

    println!("Imported cookies for '{site}' (domain: {domain}).");
    println!("Credential stored in vault.");
    Ok(())
}
