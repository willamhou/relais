use std::collections::HashMap;

use crate::adapter::Adapter;
use crate::error::AdapterError;
use crate::types::{ExecContext, Response, SiteManifest};

pub struct Router {
    adapters: HashMap<String, Box<dyn Adapter>>,
    #[cfg(feature = "audit")]
    audit: Option<crate::audit::AuditSink>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
            #[cfg(feature = "audit")]
            audit: None,
        }
    }

    /// Attach an audit sink so every [`Router::exec`] emits a signed receipt.
    #[cfg(feature = "audit")]
    pub fn with_audit(mut self, sink: crate::audit::AuditSink) -> Self {
        self.audit = Some(sink);
        self
    }

    /// Execute an action through the gateway choke point. This is the single path
    /// every caller (HTTP server and CLI) goes through, so auditing (when enabled)
    /// covers them all. Adapters are never called directly in production.
    pub async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let adapter = self
            .get(&ctx.site)
            .ok_or_else(|| AdapterError::SiteNotFound(ctx.site.clone()))?;

        #[cfg(feature = "audit")]
        if let Some(sink) = &self.audit {
            let base_url = adapter.manifest().base_url;
            let t0 = chrono::Utc::now();
            let mut result = adapter.exec(ctx).await;
            let t1 = chrono::Utc::now();
            match sink.record(ctx, &base_url, &result, t0, t1).await {
                Ok(handle) => {
                    if let Ok(resp) = &mut result {
                        resp.meta.receipt = handle;
                    }
                }
                Err(e) => {
                    if sink.closed() {
                        return Err(AdapterError::AuditUnavailable(e.to_string()));
                    }
                    tracing::error!(error = %e, "audit sink failed (response-open)");
                }
            }
            return result;
        }

        adapter.exec(ctx).await
    }

    pub fn register(&mut self, adapter: Box<dyn Adapter>) {
        let id = adapter.manifest().id.clone();
        self.adapters.insert(id, adapter);
    }

    pub fn get(&self, site_id: &str) -> Option<&dyn Adapter> {
        self.adapters.get(site_id).map(|a| a.as_ref())
    }

    pub fn sites(&self) -> Vec<SiteManifest> {
        self.adapters.values().map(|a| a.manifest()).collect()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "audit"))]
mod audit_tests {
    use super::*;
    use crate::adapter::Adapter;
    use crate::audit::{AuditConfig, AuditMode, AuditSink};
    use crate::types::{AuthType, ExecContext, Resource, Response, ResponseMeta};
    use async_trait::async_trait;
    use std::time::Duration;

    struct Stub {
        fail: bool,
    }

    #[async_trait]
    impl Adapter for Stub {
        fn manifest(&self) -> SiteManifest {
            SiteManifest {
                id: "stub".into(),
                name: "Stub".into(),
                base_url: "https://api.example".into(),
                auth_type: AuthType::None,
            }
        }
        fn resources(&self) -> Vec<Resource> {
            vec![]
        }
        async fn exec(&self, _ctx: &ExecContext) -> Result<Response, AdapterError> {
            if self.fail {
                return Err(AdapterError::NotFound("nope".into()));
            }
            Ok(Response {
                data: serde_json::json!({ "ok": true }),
                meta: ResponseMeta::default(),
            })
        }
    }

    fn sink(dir: &std::path::Path, mode: AuditMode) -> AuditSink {
        AuditSink::new(AuditConfig {
            dir: dir.to_path_buf(),
            owner: "acme".into(),
            mode,
            capacity: 16,
            ack_timeout: Duration::from_secs(5),
            passphrase: None,
        })
        .unwrap()
    }

    fn ctx(site: &str) -> ExecContext {
        ExecContext {
            site: site.into(),
            resource: "r".into(),
            action: "a".into(),
            params: serde_json::json!({ "x": 1 }),
            credentials: None,
        }
    }

    #[tokio::test]
    async fn exec_with_closed_sink_records_and_sets_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new();
        router.register(Box::new(Stub { fail: false }));
        let router = router.with_audit(sink(dir.path(), AuditMode::Closed));

        let resp = router.exec(&ctx("stub")).await.unwrap();
        let handle = resp.meta.receipt.expect("receipt handle in closed mode");
        assert!(handle.id.starts_with("rec_"));
        assert!(handle.record_hash.is_some());

        let status = signet_core::audit::verify_chain(dir.path()).unwrap();
        assert!(status.valid, "chain should be intact");
    }

    #[tokio::test]
    async fn exec_unknown_site_is_site_not_found() {
        let router = Router::new();
        let err = router.exec(&ctx("missing")).await.unwrap_err();
        assert!(matches!(err, AdapterError::SiteNotFound(_)));
    }

    #[tokio::test]
    async fn exec_records_failure_outcome_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new();
        router.register(Box::new(Stub { fail: true }));
        let router = router.with_audit(sink(dir.path(), AuditMode::Closed));

        // The adapter error still propagates to the caller...
        let err = router.exec(&ctx("stub")).await.unwrap_err();
        assert!(matches!(err, AdapterError::NotFound(_)));
        // ...and the failure is still recorded on the chain.
        let status = signet_core::audit::verify_chain(dir.path()).unwrap();
        assert!(status.valid);
        let records =
            signet_core::audit::query(dir.path(), &signet_core::audit::AuditFilter::default())
                .unwrap();
        assert_eq!(records.len(), 1, "a receipt is written even for a failed exec");
    }
}
