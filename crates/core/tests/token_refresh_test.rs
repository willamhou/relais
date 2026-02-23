use chrono::{Duration, Utc};
use relais_core::token_refresh::maybe_refresh;
use relais_core::Credentials;

#[tokio::test]
async fn non_expired_token_returns_unchanged() {
    let cred = Credentials::oauth(
        "valid_token",
        Some("refresh".into()),
        Some(Utc::now() + Duration::hours(1)),
    );
    let result = maybe_refresh(&cred, "github", None).await.unwrap();
    assert_eq!(result.bearer_token(), Some("valid_token"));
}

#[tokio::test]
async fn non_oauth_returns_unchanged() {
    let cred = Credentials::api_key("my_key");
    let result = maybe_refresh(&cred, "github", None).await.unwrap();
    assert_eq!(result.bearer_token(), Some("my_key"));
}

#[tokio::test]
async fn expired_without_refresh_token_errors() {
    let cred = Credentials::oauth("expired", None, Some(Utc::now() - Duration::hours(1)));
    let result = maybe_refresh(&cred, "github", None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("no refresh token"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn oauth_without_expiry_is_not_expired() {
    // Credentials with no expires_at are never considered expired.
    let cred = Credentials::oauth("token_no_expiry", Some("rt".into()), None);
    let result = maybe_refresh(&cred, "github", None).await.unwrap();
    assert_eq!(result.bearer_token(), Some("token_no_expiry"));
}
