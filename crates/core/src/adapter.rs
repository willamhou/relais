use async_trait::async_trait;

use crate::error::AdapterError;
use crate::types::{ExecContext, Resource, Response, SiteManifest};

#[async_trait]
pub trait Adapter: Send + Sync {
    fn manifest(&self) -> SiteManifest;
    fn resources(&self) -> Vec<Resource>;
    async fn init(&self) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError>;
    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}
