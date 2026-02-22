use relais_core::{
    Action, AuthType, Method, PaginationInfo, PaginationStyle,
    RateLimit, Resource, Response, ResponseMeta, SiteManifest,
};
use serde_json::json;

#[test]
fn site_manifest_serializes() {
    let manifest = SiteManifest {
        id: "github".into(),
        name: "GitHub".into(),
        base_url: "https://github.com".into(),
        auth_type: AuthType::APIKey,
    };
    let json = serde_json::to_value(&manifest).unwrap();
    assert_eq!(json["id"], "github");
    assert_eq!(json["auth_type"], "APIKey");
}

#[test]
fn resource_tree_navigates() {
    let resource = Resource {
        id: "repos".into(),
        description: "GitHub repositories".into(),
        actions: vec![Action {
            id: "list".into(),
            method: Method::Read,
            description: "List repositories".into(),
            params: json!({"type": "object"}),
            returns: json!({"type": "array"}),
            pagination: Some(PaginationStyle::Cursor),
        }],
        children: vec![Resource {
            id: "issues".into(),
            description: "Repository issues".into(),
            actions: vec![],
            children: vec![],
        }],
    };
    assert_eq!(resource.children[0].id, "issues");
    assert!(resource.actions[0].pagination.is_some());
}

#[test]
fn response_meta_includes_pagination() {
    let response = Response {
        data: json!({"items": []}),
        meta: ResponseMeta {
            pagination: Some(PaginationInfo {
                has_next: true,
                cursor: Some("abc123".into()),
                total: Some(42),
            }),
            rate_limit: Some(RateLimit {
                remaining: 58,
                reset_at: chrono::Utc::now(),
            }),
            cached: false,
        },
    };
    assert!(response.meta.pagination.as_ref().unwrap().has_next);
    assert_eq!(response.meta.rate_limit.as_ref().unwrap().remaining, 58);
}
