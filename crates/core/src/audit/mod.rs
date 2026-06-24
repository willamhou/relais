//! Cryptographic call auditing via [signet](https://github.com/Prismer-AI/signet).
//!
//! Behind the `audit` feature. Every relais `exec` flows through `Router::exec`
//! (added in C5), which (when a sink is configured) emits one signed, hash-chained
//! signet receipt per gateway action. See
//! `docs/design/signet-audit-integration.md` (design) and
//! `docs/design/signet-audit-impl.md` (implementation plan).
//!
//! This module is a skeleton landed in C1; the redaction/envelope/key/writer/verify
//! pieces arrive in C2–C6.

pub mod envelope;
pub mod key;
pub mod redact;
pub mod sidecar;
pub mod verify;
pub mod writer;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::error::AdapterError;
use crate::types::{AuthType, Credentials, ExecContext, ReceiptHandle, Response};
use envelope::{build_action, build_request, build_response_envelope};
use key::{AuditKey, CredBinding, CredRefStore};
use redact::{secret_values_of, AuditMeta, Redactor};
use writer::{spawn_writer, AuditJob, WriterHandle};

/// Errors raised by the audit sink. Convert into
/// [`crate::error::AdapterError::AuditUnavailable`] when surfaced to a caller.
#[derive(Debug, Error)]
pub enum AuditError {
    #[error("audit io: {0}")]
    Io(String),
    #[error("signet: {0}")]
    Signet(String),
    #[error("audit config: {0}")]
    Config(String),
    #[error("audit unavailable: {0}")]
    Unavailable(String),
}

impl From<AuditError> for crate::error::AdapterError {
    fn from(e: AuditError) -> Self {
        crate::error::AdapterError::AuditUnavailable(e.to_string())
    }
}

/// Behaviour when a receipt cannot be committed.
///
/// * `Open` — deliver the result anyway; log + metric on sink failure.
/// * `Closed` — withhold the response (return an error) if the receipt is not
///   committed. ("The caller gets no result without a receipt.")
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditMode {
    Open,
    Closed,
}

/// Sink configuration. `dir` defaults to `RELAIS_SIGNET_DIR` or `~/.relais/signet`.
#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub dir: PathBuf,
    pub owner: String,
    pub mode: AuditMode,
    pub capacity: usize,
    pub ack_timeout: Duration,
    /// Passphrase to encrypt the signing key at rest (Argon2id + XChaCha20-Poly1305).
    /// `None` stores it unencrypted (dev only) — see M6 / the CLI gate.
    pub passphrase: Option<String>,
}

/// The audit sink: owns the single writer task, the opaque credential-ref store and
/// the redactor. Constructed once and held by the [`crate::router::Router`].
pub struct AuditSink {
    writer: WriterHandle,
    credrefs: Mutex<CredRefStore>,
    redactor: Redactor,
    mode: AuditMode,
    ack_timeout: Duration,
}

impl AuditSink {
    /// Build a sink: load-or-init the gateway key, load the credential-ref store and
    /// spawn the single writer task. Must be called within a tokio runtime.
    pub fn new(cfg: AuditConfig) -> Result<Self, AuditError> {
        let key = AuditKey::load_or_init(&cfg.dir, &cfg.owner, cfg.passphrase.as_deref())?;
        let credrefs = CredRefStore::load(&cfg.dir)?;
        let writer = spawn_writer(cfg.dir.clone(), key, cfg.capacity);
        Ok(Self {
            writer,
            credrefs: Mutex::new(credrefs),
            redactor: Redactor::new(),
            mode: cfg.mode,
            ack_timeout: cfg.ack_timeout,
        })
    }

    /// Whether the sink withholds responses on audit failure (response-closed).
    pub fn closed(&self) -> bool {
        self.mode == AuditMode::Closed
    }

    /// Record one gateway action. Returns the receipt handle in closed mode (after
    /// the chain append is acknowledged), or `None` in open mode (fire-and-forget).
    ///
    /// `base_url` is passed in because `ExecContext` does not carry it (it lives on
    /// the adapter's `SiteManifest`).
    pub async fn record(
        &self,
        ctx: &ExecContext,
        base_url: &str,
        result: &Result<Response, AdapterError>,
        t0: DateTime<Utc>,
        t1: DateTime<Utc>,
    ) -> Result<Option<ReceiptHandle>, AuditError> {
        let secrets = secret_values_of(&ctx.credentials);

        // Mint the opaque credential ref in a tight sync scope; never hold the guard
        // across an await. Bind it to a salted fingerprint of the credential so a
        // rotation produces a new ref (L2).
        let credential_ref = {
            let mut store = self
                .credrefs
                .lock()
                .map_err(|_| AuditError::Io("credential-ref store lock poisoned".into()))?;
            let cred_fp = store.fingerprint(&secrets);
            store.mint(CredBinding {
                site: ctx.site.clone(),
                cred_fp,
            })?
        };
        let meta = AuditMeta {
            auth_injection: describe_injection(&ctx.credentials),
            credential_ref,
            t0: t0.to_rfc3339(),
            t1: t1.to_rfc3339(),
        };
        let request = build_request(ctx, &meta, &self.redactor, &secrets);
        let response_env = build_response_envelope(result, &self.redactor, &secrets);
        let exec_id = new_exec_id();
        let action = build_action(
            ctx,
            request.clone(),
            base_url,
            None,
            exec_id.clone(),
            exec_id,
        );

        match self.mode {
            AuditMode::Open => {
                let (ack, _rx) = oneshot::channel();
                let job = AuditJob {
                    action,
                    response_env,
                    request,
                    t0,
                    t1,
                    ack,
                };
                self.writer.try_enqueue(job)?;
                Ok(None)
            }
            AuditMode::Closed => {
                let (ack, rx) = oneshot::channel();
                let job = AuditJob {
                    action,
                    response_env,
                    request,
                    t0,
                    t1,
                    ack,
                };
                self.writer.enqueue_timeout(job, self.ack_timeout).await?;
                let acked = timeout(self.ack_timeout, rx)
                    .await
                    .map_err(|_| AuditError::Unavailable("audit ack timed out".into()))?;
                let acked =
                    acked.map_err(|_| AuditError::Unavailable("audit writer dropped".into()))?;
                let out = acked?;
                Ok(Some(ReceiptHandle {
                    id: out.id,
                    record_hash: Some(out.record_hash),
                }))
            }
        }
    }
}

/// Best-effort, non-secret descriptor of which credential kind was in play. The
/// adapter-specific wire injection (e.g. `acs_token->query`) is not known at the
/// router boundary (NF-8), so this is generic.
fn describe_injection(creds: &Option<Credentials>) -> String {
    match creds.as_ref().map(|c| &c.credential_type) {
        Some(AuthType::APIKey) => "apikey",
        Some(AuthType::OAuth) => "oauth",
        Some(AuthType::Cookie) => "cookie",
        Some(AuthType::None) | None => "none",
    }
    .to_string()
}

/// A random per-exec correlation id (`exec_<hex>`), 128 bits of randomness.
fn new_exec_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("exec_{}", hex::encode(bytes))
}
