use crate::oauth;
use crate::types::{CredentialData, Credentials};
use crate::vault::Vault;
use chrono::Utc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RefreshError {
    #[error("no refresh token available")]
    NoRefreshToken,
    #[error("no OAuth config for provider '{0}'")]
    NoProviderConfig(String),
    #[error("refresh request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("vault error: {0}")]
    Vault(#[from] crate::vault::VaultError),
}

/// Check if credentials need refresh, and if so, exchange the refresh token
/// for a new access token.
///
/// Returns the updated credentials if a refresh was performed, or the original
/// credentials when no refresh is necessary (non-expired or non-OAuth).
///
/// When a `vault` is provided the refreshed credentials are persisted so that
/// subsequent calls can pick them up without hitting the token endpoint again.
pub async fn maybe_refresh(
    credentials: &Credentials,
    site_id: &str,
    vault: Option<&Vault>,
) -> Result<Credentials, RefreshError> {
    // Non-expired credentials are returned as-is.
    if !credentials.is_expired() {
        return Ok(credentials.clone());
    }

    // Only OAuth credentials can be refreshed; other types pass through.
    let refresh_token = match &credentials.data {
        CredentialData::OAuth {
            refresh_token: Some(rt),
            ..
        } => rt.clone(),
        CredentialData::OAuth {
            refresh_token: None,
            ..
        } => return Err(RefreshError::NoRefreshToken),
        _ => return Ok(credentials.clone()),
    };

    let config = oauth::provider_config(site_id)
        .ok_or_else(|| RefreshError::NoProviderConfig(site_id.to_string()))?;

    // Exchange the refresh token for a new access token.
    let client = crate::http::client(crate::http::Profile::Default);
    let response = client
        .post(&config.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let access_token = response["access_token"]
        .as_str()
        .ok_or_else(|| {
            RefreshError::InvalidResponse(format!("no access_token in response: {response}"))
        })?;

    // Prefer the new refresh token from the response; fall back to the
    // existing one so we can keep refreshing in future cycles.
    let new_refresh = response["refresh_token"]
        .as_str()
        .map(String::from)
        .or(Some(refresh_token));

    let expires_at = response["expires_in"]
        .as_u64()
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs as i64));

    let new_cred = Credentials::oauth(access_token, new_refresh, expires_at);

    // Persist the refreshed credentials in the vault so subsequent lookups
    // get the fresh token without an extra round-trip.
    if let Some(vault) = vault {
        let json = serde_json::to_string(&new_cred)
            .map_err(|e| RefreshError::InvalidResponse(e.to_string()))?;
        vault.store(site_id, &json)?;
    }

    Ok(new_cred)
}
