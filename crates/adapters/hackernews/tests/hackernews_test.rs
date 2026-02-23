use relais_adapter_hackernews::HackerNewsAdapter;
use relais_core::Adapter;

#[test]
fn hackernews_manifest_is_correct() {
    let adapter = HackerNewsAdapter::new();
    let manifest = adapter.manifest();
    assert_eq!(manifest.id, "hackernews");
    assert_eq!(manifest.name, "Hacker News");
    assert_eq!(manifest.base_url, "https://hacker-news.firebaseio.com/v0");
    assert!(matches!(manifest.auth_type, relais_core::AuthType::None));
}

#[test]
fn hackernews_exposes_stories_resource() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let ids: Vec<&str> = resources.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"stories"));
}

#[test]
fn hackernews_stories_has_list_actions() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();
    let actions: Vec<&str> = stories.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(actions.contains(&"list_top"));
    assert!(actions.contains(&"list_new"));
    assert!(actions.contains(&"list_best"));
    assert!(actions.contains(&"get"));
}

#[test]
fn hackernews_stories_has_comments_child() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();
    let children: Vec<&str> = stories.children.iter().map(|r| r.id.as_str()).collect();
    assert!(children.contains(&"comments"));
}

#[test]
fn hackernews_comments_has_list_action() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();
    let comments = stories.children.iter().find(|r| r.id == "comments").unwrap();
    let actions: Vec<&str> = comments.actions.iter().map(|a| a.id.as_str()).collect();
    assert!(actions.contains(&"list"));
}

#[test]
fn hackernews_list_actions_use_offset_pagination() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();

    for action_id in &["list_top", "list_new", "list_best"] {
        let action = stories
            .actions
            .iter()
            .find(|a| a.id == *action_id)
            .unwrap_or_else(|| panic!("action {} not found", action_id));
        match &action.pagination {
            Some(relais_core::PaginationStyle::Offset { max_limit }) => {
                assert_eq!(*max_limit, 500);
            }
            other => panic!(
                "expected Offset pagination for {}, got {:?}",
                action_id, other
            ),
        }
    }
}

#[test]
fn hackernews_get_action_has_no_pagination() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();
    let get_action = stories.actions.iter().find(|a| a.id == "get").unwrap();
    assert!(get_action.pagination.is_none());
}

#[test]
fn hackernews_get_action_uses_read_method() {
    let adapter = HackerNewsAdapter::new();
    let resources = adapter.resources();
    let stories = resources.iter().find(|r| r.id == "stories").unwrap();
    let get_action = stories.actions.iter().find(|a| a.id == "get").unwrap();
    assert!(matches!(get_action.method, relais_core::Method::Read));
}

#[test]
fn hackernews_default_impl() {
    let adapter = HackerNewsAdapter::default();
    let manifest = adapter.manifest();
    assert_eq!(manifest.id, "hackernews");
}
