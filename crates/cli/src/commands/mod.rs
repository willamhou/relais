pub mod apis;
pub mod exec;
pub mod serve;
pub mod sites;
pub mod spec;
pub mod vault;

use relais_core::router::Router;

/// Build a Router with all built-in adapters registered.
pub fn build_router() -> Router {
    let mut router = Router::new();
    router.register(Box::new(
        relais_adapter_github::GitHubAdapter::new(),
    ));
    router.register(Box::new(
        relais_adapter_hackernews::HackerNewsAdapter::new(),
    ));
    // LLM fallback adapter requires a provider configuration.
    // Skip registration here; users can configure it via environment variables in the future.
    router
}
