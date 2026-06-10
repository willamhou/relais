use relais_adapter_scs::ScsAdapter;
use relais_core::{Adapter, AuthType, Method, PaginationStyle};

fn adapter() -> ScsAdapter {
    // Use an explicit base URL so tests never depend on the SCS_BASE_URL env var.
    ScsAdapter::with_base_url("http://example.test:8000")
}

#[test]
fn scs_manifest_id_and_name() {
    let m = adapter().manifest();
    assert_eq!(m.id, "scs");
    assert_eq!(m.name, "SCS");
}

#[test]
fn scs_manifest_uses_api_key_auth() {
    let m = adapter().manifest();
    assert!(matches!(m.auth_type, AuthType::APIKey));
}

#[test]
fn scs_manifest_base_url_is_configurable() {
    let m = adapter().manifest();
    assert_eq!(m.base_url, "http://example.test:8000");
}

#[test]
fn scs_default_base_url() {
    let m = ScsAdapter::with_base_url(relais_adapter_scs::DEFAULT_BASE_URL).manifest();
    assert_eq!(m.base_url, "http://127.0.0.1:8000");
}

#[test]
fn scs_exposes_accounts_resource() {
    let resources = adapter().resources();
    let ids: Vec<&str> = resources.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"accounts"));
}

#[test]
fn scs_accounts_has_all_five_actions() {
    let resources = adapter().resources();
    let accounts = resources.iter().find(|r| r.id == "accounts").unwrap();
    let actions: Vec<&str> = accounts.actions.iter().map(|a| a.id.as_str()).collect();
    for expected in ["list", "get", "create", "update", "delete"] {
        assert!(actions.contains(&expected), "missing action {expected}");
    }
}

#[test]
fn scs_accounts_has_no_children() {
    let resources = adapter().resources();
    let accounts = resources.iter().find(|r| r.id == "accounts").unwrap();
    assert!(accounts.children.is_empty());
}

#[test]
fn scs_action_methods_are_correct() {
    let resources = adapter().resources();
    let accounts = resources.iter().find(|r| r.id == "accounts").unwrap();
    let method = |id: &str| {
        accounts
            .actions
            .iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("no action {id}"))
            .method
            .clone()
    };
    assert!(matches!(method("list"), Method::Read));
    assert!(matches!(method("get"), Method::Read));
    assert!(matches!(method("create"), Method::Write));
    assert!(matches!(method("update"), Method::Write));
    assert!(matches!(method("delete"), Method::Delete));
}

#[test]
fn scs_list_uses_offset_pagination() {
    let resources = adapter().resources();
    let accounts = resources.iter().find(|r| r.id == "accounts").unwrap();
    let list = accounts.actions.iter().find(|a| a.id == "list").unwrap();
    assert!(matches!(
        list.pagination,
        Some(PaginationStyle::Offset { .. })
    ));
}

#[test]
fn scs_list_params_declare_pagination_fields() {
    let resources = adapter().resources();
    let accounts = resources.iter().find(|r| r.id == "accounts").unwrap();
    let list = accounts.actions.iter().find(|a| a.id == "list").unwrap();
    let props = &list.params["properties"];
    assert!(props.get("page").is_some(), "list params must declare page");
    assert!(
        props.get("page_size").is_some(),
        "list params must declare page_size"
    );
}

#[tokio::test]
async fn scs_exec_unsupported_resource() {
    let ctx = relais_core::ExecContext {
        site: "scs".into(),
        resource: "nonexistent".into(),
        action: "list".into(),
        params: serde_json::json!({}),
        credentials: None,
    };
    assert!(adapter().exec(&ctx).await.is_err());
}

#[tokio::test]
async fn scs_exec_unsupported_action() {
    let ctx = relais_core::ExecContext {
        site: "scs".into(),
        resource: "accounts".into(),
        action: "nonexistent".into(),
        params: serde_json::json!({}),
        credentials: None,
    };
    assert!(adapter().exec(&ctx).await.is_err());
}
