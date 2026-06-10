//! HTTP-path integration tests for `ScsAdapter::exec`, driven by a wiremock
//! mock server (no real SCS instance required).
use relais_adapter_scs::ScsAdapter;
use relais_core::{Adapter, AdapterError, Credentials, ExecContext};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx(resource: &str, action: &str, params: serde_json::Value) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: None,
    }
}

async fn adapter_for(server: &MockServer) -> ScsAdapter {
    ScsAdapter::with_base_url(server.uri())
}

#[tokio::test]
async fn get_returns_account_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "5", "name": "Acme", "phone": "13800000000"
        })))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let resp = adapter
        .exec(&ctx("accounts", "get", json!({"id": 5})))
        .await
        .unwrap();
    assert_eq!(resp.data["name"], "Acme");
    assert_eq!(resp.data["id"], "5");
}

#[tokio::test]
async fn get_missing_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/99"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "not found"})))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let err = adapter
        .exec(&ctx("accounts", "get", json!({"id": 99})))
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn create_posts_only_present_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/accounts"))
        .and(body_json(json!({"name": "Acme", "phone": "13800000000"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1", "name": "Acme"})))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let resp = adapter
        .exec(&ctx(
            "accounts",
            "create",
            json!({"name": "Acme", "phone": "13800000000"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.data["id"], "1");
}

#[tokio::test]
async fn list_sends_query_and_fills_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts"))
        .and(query_param("page", "1"))
        .and(query_param("page_size", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accounts": [{"id": "1"}],
            "total": "100"
        })))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let resp = adapter
        .exec(&ctx(
            "accounts",
            "list",
            json!({"page": 1, "page_size": 20}),
        ))
        .await
        .unwrap();
    let pg = resp.meta.pagination.expect("list should fill pagination");
    assert_eq!(pg.total, Some(100));
    assert!(pg.has_next, "page 1 of 100 with size 20 has next");
}

#[tokio::test]
async fn unauthorized_maps_to_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/1"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let err = adapter
        .exec(&ctx("accounts", "get", json!({"id": 1})))
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::Auth(_)), "got {err:?}");
}

#[tokio::test]
async fn too_many_requests_maps_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/1"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let err = adapter
        .exec(&ctx("accounts", "get", json!({"id": 1})))
        .await
        .unwrap_err();
    match err {
        AdapterError::RateLimited { retry_after_secs } => assert_eq!(retry_after_secs, 30),
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn too_many_requests_without_retry_after_defaults_to_60() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/1"))
        .respond_with(ResponseTemplate::new(429)) // no retry-after header
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let err = adapter
        .exec(&ctx("accounts", "get", json!({"id": 1})))
        .await
        .unwrap_err();
    match err {
        AdapterError::RateLimited { retry_after_secs } => assert_eq!(retry_after_secs, 60),
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_request_preserves_body_in_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/accounts"))
        .respond_with(ResponseTemplate::new(400).set_body_string("name is required"))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let err = adapter
        .exec(&ctx("accounts", "create", json!({"phone": "x"})))
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(e) => assert!(
            e.to_string().contains("name is required"),
            "error should preserve kratos body, got: {e}"
        ),
        other => panic!("expected Other, got {other:?}"),
    }
}

#[tokio::test]
async fn delete_with_empty_body_does_not_panic() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v1/accounts/5"))
        .respond_with(ResponseTemplate::new(200)) // empty body
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let resp = adapter
        .exec(&ctx("accounts", "delete", json!({"id": 5})))
        .await
        .unwrap();
    assert_eq!(resp.data, json!({}));
}

#[tokio::test]
async fn credentials_inject_bearer_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/accounts/1"))
        .and(header("authorization", "Bearer acs-token-xyz"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1"})))
        .mount(&server)
        .await;

    let adapter = adapter_for(&server).await;
    let mut c = ctx("accounts", "get", json!({"id": 1}));
    c.credentials = Some(Credentials::api_key("acs-token-xyz"));
    // If the header matcher fails, wiremock returns no match -> exec errors.
    let resp = adapter.exec(&c).await.unwrap();
    assert_eq!(resp.data["id"], "1");
}
