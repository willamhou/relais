use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderValue;
use axum_test::TestServer;
use jsonwebtoken::{encode, EncodingKey, Header};
use relais_core::adapter::Adapter;
use relais_core::error::AdapterError;
use relais_core::router::Router;
use relais_core::types::*;
use relais_server::state::{AppState, SharedState};
use serde::{Deserialize, Serialize};
use serde_json::json;

const TEST_JWT_SECRET: &str = "test-secret-for-jwt-signing";

// --- Mock Adapter ---

struct MockAdapter;

#[async_trait]
impl Adapter for MockAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "mock".to_string(),
            name: "Mock Site".to_string(),
            base_url: "https://mock.example.com".to_string(),
            auth_type: AuthType::None,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource {
            id: "widgets".to_string(),
            description: "Mock widgets".to_string(),
            actions: vec![Action {
                id: "list".to_string(),
                method: Method::Read,
                description: "List all widgets".to_string(),
                params: json!({}),
                returns: json!({"type": "array"}),
                pagination: None,
            }],
            children: vec![],
        }]
    }

    async fn exec(&self, _ctx: &ExecContext) -> Result<Response, AdapterError> {
        Ok(Response {
            data: json!({"items": [{"id": 1, "name": "widget-1"}]}),
            meta: ResponseMeta {
                pagination: None,
                rate_limit: None,
                cached: false,
                receipt: None,
            },
        })
    }
}

// --- Test Helpers ---

fn test_state() -> AppState {
    let mut router = Router::new();
    router.register(Box::new(MockAdapter));
    Arc::new(SharedState {
        router,
        jwt_secret: TEST_JWT_SECRET.to_string(),
        vault: None,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

fn test_jwt() -> String {
    let claims = Claims {
        sub: "test-user".to_string(),
        exp: 9_999_999_999, // far future
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
    )
    .expect("failed to encode test JWT")
}

// --- Tests ---

#[tokio::test]
async fn health_check() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let response = server.get("/health").await;
    response.assert_status_ok();
    response.assert_text("ok");
}

#[tokio::test]
async fn list_sites_requires_auth() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let response = server.get("/v1/sites").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn list_sites_with_valid_jwt() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let token = test_jwt();
    let response = server
        .get("/v1/sites")
        .add_header(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        )
        .await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let sites = body.as_array().expect("response should be an array");
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0]["id"], "mock");
    assert_eq!(sites[0]["name"], "Mock Site");
}

#[tokio::test]
async fn list_apis_for_site() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let token = test_jwt();
    let response = server
        .get("/v1/apis/mock")
        .add_header(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        )
        .await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let resources = body.as_array().expect("response should be an array");
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["id"], "widgets");
}

#[tokio::test]
async fn list_apis_unknown_site_returns_404() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let token = test_jwt();
    let response = server
        .get("/v1/apis/unknown")
        .add_header(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        )
        .await;

    response.assert_status_not_found();
}

#[tokio::test]
async fn get_spec_for_action() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let token = test_jwt();
    let response = server
        .get("/v1/spec/mock.widgets.list")
        .add_header(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        )
        .await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], "list");
    assert_eq!(body["method"], "Read");
}

#[tokio::test]
async fn exec_action_returns_response() {
    let state = test_state();
    let app = relais_server::app(state);
    let server = TestServer::new(app).expect("failed to create test server");

    let token = test_jwt();
    let response = server
        .post("/v1/exec")
        .add_header(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        )
        .json(&json!({
            "site": "mock",
            "resource": "widgets",
            "action": "list",
            "params": {}
        }))
        .await;

    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["data"]["items"].is_array());
}
