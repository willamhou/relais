use thiserror::Error;

#[derive(Error, Debug)]
pub enum AdapterError {
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("action not supported: {0}")]
    Unsupported(String),
    #[error("site not found: {0}")]
    SiteNotFound(String),
    #[error("audit unavailable: {0}")]
    AuditUnavailable(String),
    #[error("upstream error: {0}")]
    Upstream(#[from] reqwest::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
