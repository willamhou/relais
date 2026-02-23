use relais_adapter_github::GitHubAdapter;
use relais_core::Adapter;

#[test]
fn github_manifest_is_correct() {
    let adapter = GitHubAdapter::new();
    let manifest = adapter.manifest();
    assert_eq!(manifest.id, "github");
    assert_eq!(manifest.base_url, "https://api.github.com");
}

#[test]
fn github_manifest_uses_api_key_auth() {
    let adapter = GitHubAdapter::new();
    let manifest = adapter.manifest();
    assert_eq!(manifest.name, "GitHub");
    assert!(matches!(manifest.auth_type, relais_core::AuthType::APIKey));
}

#[test]
fn github_exposes_repos_resource() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let ids: Vec<&str> = resources.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"repos"));
}

#[test]
fn github_repos_has_list_action() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let actions: Vec<&str> = repos.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(actions.contains(&"list"));
}

#[test]
fn github_repos_has_get_action() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let actions: Vec<&str> = repos.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(actions.contains(&"get"));
}

#[test]
fn github_repos_has_issues_child() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let children: Vec<&str> = repos.children.iter().map(|r| r.id.as_str()).collect();
    assert!(children.contains(&"issues"));
}

#[test]
fn github_issues_has_list_create_get_actions() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let issues = repos.children.iter().find(|r| r.id == "issues").unwrap();
    let actions: Vec<&str> = issues.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(actions.contains(&"list"), "issues should have list action");
    assert!(actions.contains(&"create"), "issues should have create action");
    assert!(actions.contains(&"get"), "issues should have get action");
}

#[test]
fn github_issues_has_comments_child() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let issues = repos.children.iter().find(|r| r.id == "issues").unwrap();
    let children: Vec<&str> = issues.children.iter().map(|r| r.id.as_str()).collect();
    assert!(children.contains(&"comments"));
}

#[test]
fn github_comments_has_list_create_delete_actions() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();
    let issues = repos.children.iter().find(|r| r.id == "issues").unwrap();
    let comments = issues.children.iter().find(|r| r.id == "comments").unwrap();
    let actions: Vec<&str> = comments.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(
        actions.contains(&"list"),
        "comments should have list action"
    );
    assert!(
        actions.contains(&"create"),
        "comments should have create action"
    );
    assert!(
        actions.contains(&"delete"),
        "comments should have delete action"
    );
}

#[test]
fn github_list_actions_use_cursor_pagination() {
    let adapter = GitHubAdapter::new();
    let resources = adapter.resources();
    let repos = resources.iter().find(|r| r.id == "repos").unwrap();

    let list = repos.actions.iter().find(|a| a.id == "list").unwrap();
    assert!(
        list.pagination.is_some(),
        "repos.list should have pagination"
    );
    assert!(matches!(
        list.pagination,
        Some(relais_core::PaginationStyle::Cursor)
    ));
}

#[tokio::test]
async fn github_exec_returns_unsupported_for_unknown_resource() {
    let adapter = GitHubAdapter::new();
    let ctx = relais_core::ExecContext {
        site: "github".into(),
        resource: "nonexistent".into(),
        action: "list".into(),
        params: serde_json::json!({}),
        credentials: None,
    };
    let result = adapter.exec(&ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn github_exec_returns_unsupported_for_unknown_action() {
    let adapter = GitHubAdapter::new();
    let ctx = relais_core::ExecContext {
        site: "github".into(),
        resource: "repos".into(),
        action: "nonexistent".into(),
        params: serde_json::json!({}),
        credentials: None,
    };
    let result = adapter.exec(&ctx).await;
    assert!(result.is_err());
}

#[test]
fn github_default_creates_adapter() {
    let adapter = GitHubAdapter::default();
    let manifest = adapter.manifest();
    assert_eq!(manifest.id, "github");
}
