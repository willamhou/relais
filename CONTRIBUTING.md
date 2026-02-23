# Contributing to Relais

Thank you for your interest in contributing to Relais! This guide covers how to write and submit adapters, as well as general contribution guidelines.

## Writing an Adapter

### 1. Create the crate

```bash
mkdir -p crates/adapters/mysite
cd crates/adapters/mysite
cargo init --lib --name relais-adapter-mysite
```

Add `relais-core` as a dependency in your new crate's `Cargo.toml`:

```toml
[dependencies]
relais-core = { path = "../../core" }
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json"] }
serde_json = "1"
tracing = "0.1"
```

### 2. Implement the Adapter trait

```rust
use async_trait::async_trait;
use relais_core::{
    Action, Adapter, AdapterError, AuthType, ExecContext,
    Method, Resource, Response, ResponseMeta, SiteManifest,
};
use reqwest::Client;
use serde_json::json;

pub struct MySiteAdapter {
    client: Client,
}

impl MySiteAdapter {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for MySiteAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for MySiteAdapter {
    fn manifest(&self) -> SiteManifest {
        SiteManifest {
            id: "mysite".into(),
            name: "My Site".into(),
            base_url: "https://api.mysite.com".into(),
            auth_type: AuthType::APIKey,
        }
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource {
            id: "items".into(),
            description: "Items on My Site".into(),
            actions: vec![Action {
                id: "list".into(),
                method: Method::Read,
                description: "List all items".into(),
                params: json!({}),
                returns: json!({"type": "array", "items": {"type": "object"}}),
                pagination: None,
            }],
            children: vec![],
        }]
    }

    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        match (ctx.resource.as_str(), ctx.action.as_str()) {
            ("items", "list") => {
                // Fetch from the upstream API and return structured data
                todo!("implement")
            }
            _ => Err(AdapterError::Unsupported(format!(
                "{}.{}",
                ctx.resource, ctx.action
            ))),
        }
    }
}
```

### 3. Test Requirements

- All public methods must have tests
- Resource tree structure tests (manifest, resources, actions)
- `exec()` error handling tests
- Run with: `cargo test -p relais-adapter-mysite`

Example test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_correct_id() {
        let adapter = MySiteAdapter::new();
        assert_eq!(adapter.manifest().id, "mysite");
    }

    #[test]
    fn resources_are_not_empty() {
        let adapter = MySiteAdapter::new();
        assert!(!adapter.resources().is_empty());
    }

    #[tokio::test]
    async fn exec_unsupported_action_returns_error() {
        let adapter = MySiteAdapter::new();
        let ctx = ExecContext {
            site: "mysite".into(),
            resource: "nonexistent".into(),
            action: "nope".into(),
            params: serde_json::json!({}),
            credentials: None,
        };
        let result = adapter.exec(&ctx).await;
        assert!(result.is_err());
    }
}
```

### 4. Review Checklist

Before submitting a PR:

- [ ] Adapter implements all required trait methods
- [ ] Resource tree accurately represents the site's capabilities
- [ ] Error handling maps to appropriate `AdapterError` variants
- [ ] No hardcoded credentials
- [ ] User-Agent header set appropriately
- [ ] Rate limiting respected
- [ ] Tests pass with `cargo test`
- [ ] No clippy warnings with `cargo clippy`

### 5. Submit

1. Fork the repo
2. Create a branch: `feat/adapter-mysite`
3. Add your crate to workspace members in root `Cargo.toml`
4. Submit a PR with a description of what the adapter covers

## Code Style

- `rustfmt` and `clippy` are mandatory
- Follow existing adapter patterns (see the `github` and `hackernews` adapters in `crates/adapters/`)
- Accept interfaces, return structs
- Wrap errors with context

## License

By contributing, you agree that your contributions will be dual-licensed under MIT and Apache 2.0.
