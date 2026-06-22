use relais_core::router::Router;
use relais_core::{
    Adapter, AdapterError, ExecContext, Resource, Response, ResponseMeta, SiteManifest, AuthType,
};
use async_trait::async_trait;
use serde_json::json;

struct MockAdapter {
    site_id: String,
}

#[async_trait]
impl Adapter for MockAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: self.site_id.clone(),
            name: self.site_id.clone(),
            base_url: format!("https://{}.com", self.site_id),
            auth_type: AuthType::None,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![]
    }

    async fn exec(&self, _ctx: &ExecContext) -> Result<Response, AdapterError> {
        Ok(Response {
            data: json!({"from": self.site_id}),
            meta: ResponseMeta {
                pagination: None,
                rate_limit: None,
                cached: false,
                receipt: None,
            },
        })
    }
}

#[test]
fn router_registers_and_finds_adapter() {
    let mut router = Router::new();
    router.register(Box::new(MockAdapter { site_id: "github".into() }));
    router.register(Box::new(MockAdapter { site_id: "hackernews".into() }));

    assert!(router.get("github").is_some());
    assert!(router.get("hackernews").is_some());
    assert!(router.get("unknown").is_none());
}

#[test]
fn router_lists_all_sites() {
    let mut router = Router::new();
    router.register(Box::new(MockAdapter { site_id: "github".into() }));
    router.register(Box::new(MockAdapter { site_id: "hackernews".into() }));

    let sites = router.sites();
    assert_eq!(sites.len(), 2);
    assert!(sites.iter().any(|s| s.id == "github"));
}
