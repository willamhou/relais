use async_trait::async_trait;
use relais_llm_fallback::extractor::Extractor;
use relais_llm_fallback::{LlmClient, LlmError};

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
        Ok(r#"{"title": "Test Page", "price": 29.99}"#.to_string())
    }
}

#[tokio::test]
async fn extractor_returns_structured_data() {
    let extractor = Extractor::new(Box::new(MockLlmClient));
    let html = "<html><body><h1>Test Page</h1><span>$29.99</span></body></html>";
    let result = extractor
        .extract(html, "Extract the page title and price")
        .await
        .unwrap();
    assert_eq!(result["title"], "Test Page");
    assert_eq!(result["price"], 29.99);
}

#[tokio::test]
async fn extractor_handles_invalid_json_from_llm() {
    struct BadLlmClient;

    #[async_trait]
    impl LlmClient for BadLlmClient {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok("not valid json".to_string())
        }
    }

    let extractor = Extractor::new(Box::new(BadLlmClient));
    let result = extractor.extract("<html></html>", "test").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, LlmError::ParseError(_)),
        "expected ParseError, got: {err:?}"
    );
}

#[tokio::test]
async fn extractor_handles_provider_error() {
    struct ErrorLlmClient;

    #[async_trait]
    impl LlmClient for ErrorLlmClient {
        async fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Err(LlmError::Provider("API rate limited".to_string()))
        }
    }

    let extractor = Extractor::new(Box::new(ErrorLlmClient));
    let result = extractor.extract("<html></html>", "test").await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, LlmError::Provider(_)),
        "expected Provider error, got: {err:?}"
    );
}

#[tokio::test]
async fn extractor_truncates_long_html() {
    struct CapturingLlmClient;

    #[async_trait]
    impl LlmClient for CapturingLlmClient {
        async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
            // The prompt should contain truncated HTML (max 50000 chars)
            // We'll verify that a very long input gets truncated
            assert!(
                prompt.len() <= 60000,
                "prompt should be bounded, got {} chars",
                prompt.len()
            );
            Ok(r#"{"truncated": true}"#.to_string())
        }
    }

    let extractor = Extractor::new(Box::new(CapturingLlmClient));
    let long_html = "x".repeat(100_000);
    let result = extractor
        .extract(&long_html, "extract something")
        .await
        .unwrap();
    assert_eq!(result["truncated"], true);
}

#[tokio::test]
async fn adapter_manifest_returns_web_id() {
    use relais_core::Adapter;
    use relais_llm_fallback::LlmFallbackAdapter;

    let adapter = LlmFallbackAdapter::new(Box::new(MockLlmClient));
    let manifest = adapter.manifest();
    assert_eq!(manifest.id, "web");
    assert!(matches!(manifest.auth_type, relais_core::AuthType::None));
}

#[tokio::test]
async fn adapter_resources_contains_pages() {
    use relais_core::Adapter;
    use relais_llm_fallback::LlmFallbackAdapter;

    let adapter = LlmFallbackAdapter::new(Box::new(MockLlmClient));
    let resources = adapter.resources();
    assert!(!resources.is_empty());
    assert_eq!(resources[0].id, "pages");
    assert!(!resources[0].actions.is_empty());
    assert_eq!(resources[0].actions[0].id, "read");
}

#[tokio::test]
async fn adapter_exec_unsupported_action() {
    use relais_core::{Adapter, ExecContext};
    use relais_llm_fallback::LlmFallbackAdapter;
    use serde_json::json;

    let adapter = LlmFallbackAdapter::new(Box::new(MockLlmClient));
    let ctx = ExecContext {
        site: "web".to_string(),
        resource: "unknown".to_string(),
        action: "delete".to_string(),
        params: json!({}),
        credentials: None,
    };
    let result = adapter.exec(&ctx).await;
    assert!(result.is_err());
}
