use std::sync::Arc;

use axum::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use relais_adapter_hackernews::HackerNewsAdapter;
use relais_core::router::Router;
use relais_server::state::{AppState, SharedState};
use serde::{Deserialize, Serialize};
use serde_json::json;

const TEST_JWT_SECRET: &str = "e2e-test-secret-for-jwt-signing";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

/// Build an [`AppState`] with the real Hacker News adapter registered.
fn hn_state() -> AppState {
    let mut router = Router::new();
    router.register(Box::new(HackerNewsAdapter::new()));
    Arc::new(SharedState {
        router,
        jwt_secret: TEST_JWT_SECRET.to_string(),
    })
}

/// Create a [`TestServer`] wired to the real HN adapter.
fn hn_server() -> TestServer {
    let state = hn_state();
    let app = relais_server::app(state);
    TestServer::new(app).expect("failed to create test server")
}

/// Generate a valid JWT with a far-future expiry.
fn valid_jwt() -> String {
    let claims = Claims {
        sub: "e2e-test-user".to_string(),
        exp: 9_999_999_999, // far future
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .expect("failed to encode test JWT")
}

/// Generate an expired JWT (exp in the past).
fn expired_jwt() -> String {
    let claims = Claims {
        sub: "e2e-test-user".to_string(),
        exp: 1_000_000_000, // 2001-09-09, well in the past
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .expect("failed to encode expired JWT")
}

/// Convenience: add a valid Authorization header to a request builder.
fn auth_header(token: &str) -> (axum::http::header::HeaderName, HeaderValue) {
    (
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    )
}

// ===========================================================================
// Test 1: Full API flow with Hacker News adapter
// ===========================================================================

#[tokio::test]
async fn e2e_list_sites_contains_hackernews() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server.get("/v1/sites").add_header(name, value).await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let sites = body.as_array().expect("response should be an array");
    let ids: Vec<&str> = sites
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();

    assert!(
        ids.contains(&"hackernews"),
        "expected 'hackernews' in site list, got: {:?}",
        ids
    );
}

#[tokio::test]
async fn e2e_list_apis_hackernews_has_stories_resource() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server.get("/v1/apis/hackernews").add_header(name, value).await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let resources = body.as_array().expect("response should be an array");
    let resource_ids: Vec<&str> = resources
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();

    assert!(
        resource_ids.contains(&"stories"),
        "expected 'stories' resource for hackernews, got: {:?}",
        resource_ids
    );
}

#[tokio::test]
async fn e2e_get_spec_hackernews_stories_list_top() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server
        .get("/v1/spec/hackernews.stories.list_top")
        .add_header(name, value)
        .await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "list_top", "action id should be 'list_top'");
    assert_eq!(body["method"], "Read", "method should be 'Read'");
    assert!(
        body["description"].as_str().is_some(),
        "action should have a description"
    );
}

// ===========================================================================
// Test 2: Auth rejection flow
// ===========================================================================

#[tokio::test]
async fn e2e_auth_rejected_without_jwt() {
    let server = hn_server();

    let response = server.get("/v1/sites").await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn e2e_auth_rejected_with_invalid_jwt() {
    let server = hn_server();
    let (name, value) = auth_header("this-is-not-a-valid-jwt-token");

    let response = server.get("/v1/sites").add_header(name, value).await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn e2e_auth_rejected_with_expired_jwt() {
    let server = hn_server();
    let token = expired_jwt();
    let (name, value) = auth_header(&token);

    let response = server.get("/v1/sites").add_header(name, value).await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn e2e_auth_rejected_with_wrong_secret() {
    // Sign with a different secret than the server expects.
    let claims = Claims {
        sub: "e2e-test-user".to_string(),
        exp: 9_999_999_999,
    };
    let wrong_secret_token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(b"wrong-secret"),
    )
    .expect("failed to encode JWT with wrong secret");

    let server = hn_server();
    let (name, value) = auth_header(&wrong_secret_token);

    let response = server.get("/v1/sites").add_header(name, value).await;

    response.assert_status_unauthorized();
}

// ===========================================================================
// Test 3: 404 for unknown site
// ===========================================================================

#[tokio::test]
async fn e2e_unknown_site_returns_404() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server
        .get("/v1/apis/unknown_site")
        .add_header(name, value)
        .await;

    response.assert_status_not_found();
}

// ===========================================================================
// Test 4: Exec endpoint structure (hits real HN API)
// ===========================================================================

/// This test hits the real Hacker News API. If the network is unavailable,
/// the test should be run with `--include-ignored` or skipped gracefully.
#[tokio::test]
#[ignore]
async fn e2e_exec_hackernews_list_top_stories() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server
        .post("/v1/exec")
        .add_header(name, value)
        .json(&json!({
            "site": "hackernews",
            "resource": "stories",
            "action": "list_top",
            "params": { "limit": 5 }
        }))
        .await;

    // The request should be accepted (not a 4xx).
    let status = response.status_code();
    assert!(
        status.is_success() || status.is_server_error(),
        "expected 2xx or 5xx (network issue), got {}",
        status
    );

    // If successful, verify the response structure.
    if status.is_success() {
        let body: serde_json::Value = response.json();
        assert!(body["data"].is_array(), "data should be an array of story IDs");
        assert!(
            body["meta"].is_object(),
            "response should include a meta object"
        );
    }
}

/// Verify exec returns 404 for an unknown site.
#[tokio::test]
async fn e2e_exec_unknown_site_returns_404() {
    let server = hn_server();
    let token = valid_jwt();
    let (name, value) = auth_header(&token);

    let response = server
        .post("/v1/exec")
        .add_header(name, value)
        .json(&json!({
            "site": "nonexistent",
            "resource": "things",
            "action": "list",
            "params": {}
        }))
        .await;

    response.assert_status_not_found();
}
