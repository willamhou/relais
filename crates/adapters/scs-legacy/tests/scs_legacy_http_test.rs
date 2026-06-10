//! HTTP-path integration tests for `ScsLegacyAdapter::exec`, driven by a wiremock
//! mock server (no real legacy SCS instance required).
use relais_adapter_scs_legacy::ScsLegacyAdapter;
use relais_core::{Adapter, AdapterError, Credentials, ExecContext};
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx(resource: &str, action: &str, params: serde_json::Value) -> ExecContext {
    ExecContext {
        site: "scs".into(),
        resource: resource.into(),
        action: action.into(),
        params,
        credentials: Some(Credentials::api_key("tok-123")),
    }
}

#[tokio::test]
async fn post_action_injects_acs_token_into_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/accounts/create"))
        // params + the injected credential go into the JSON body
        .and(body_json(json!({"name": "Acme", "acs_token": "tok-123"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"id": 1}})))
        .mount(&server)
        .await;

    let adapter = ScsLegacyAdapter::with_base_url(server.uri());
    let resp = adapter
        .exec(&ctx("accounts", "create", json!({"name": "Acme"})))
        .await
        .expect("create should succeed");
    assert_eq!(resp.data["data"]["id"], 1);
}

#[tokio::test]
async fn get_action_injects_acs_token_into_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/advice/"))
        .and(query_param("acs_token", "tok-123"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;

    let adapter = ScsLegacyAdapter::with_base_url(server.uri());
    let resp = adapter
        .exec(&ctx("advice", "index", json!({"page": 1})))
        .await
        .expect("get action should succeed");
    assert!(resp.data["data"].is_array());
}

#[tokio::test]
async fn unknown_action_is_unsupported() {
    let adapter = ScsLegacyAdapter::with_base_url("http://unused.test");
    let err = adapter
        .exec(&ctx("accounts", "no_such_action", json!({})))
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::Unsupported(_)), "got {err:?}");
}

#[tokio::test]
async fn not_found_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/accounts/create"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let adapter = ScsLegacyAdapter::with_base_url(server.uri());
    let err = adapter
        .exec(&ctx("accounts", "create", json!({"name": "x"})))
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn empty_body_does_not_panic() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/accounts/create"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let adapter = ScsLegacyAdapter::with_base_url(server.uri());
    let resp = adapter
        .exec(&ctx("accounts", "create", json!({"name": "x"})))
        .await
        .expect("empty 200 should be tolerated");
    assert_eq!(resp.data, json!({}));
}

#[tokio::test]
async fn server_error_preserves_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/accounts/create"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let adapter = ScsLegacyAdapter::with_base_url(server.uri());
    let err = adapter
        .exec(&ctx("accounts", "create", json!({"name": "x"})))
        .await
        .unwrap_err();
    match err {
        AdapterError::Other(e) => assert!(e.to_string().contains("boom"), "got {e}"),
        other => panic!("expected Other, got {other:?}"),
    }
}
