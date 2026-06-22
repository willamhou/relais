//! The single sequential audit writer (C4).
//!
//! One task owns the chain. It signs (`sign_compound`), persists the sidecar, then
//! appends to signet's hash chain — **one job at a time, start to finish, never
//! aborted** (RD-3). The sign+sidecar+append sequence runs under a cross-process
//! exclusive lock over the audit dir, and re-clamps the timestamp against the on-disk
//! latest under that lock, so concurrent processes sharing one dir cannot fork the
//! chain (design §4.7).
//!
//! It also stamps a **monotonic non-decreasing `ts_request`** (seeded from the
//! existing chain), so signet's per-date file selection never files an older date
//! after a newer one — keeping the chain linear across midnight/restart (R2-1).
//!
//! Caller-side timeouts ([`WriterHandle::enqueue_timeout`] and the ack await in C5)
//! bound the *request's wait*, never the append.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use signet_core::Action;
use tokio::sync::{mpsc, oneshot};

use super::key::AuditKey;
use super::{sidecar, AuditError};

/// Signer name recorded in every receipt's `Signer`.
const SIGNER_NAME: &str = "relais";

/// A unit of audit work handed to the writer.
pub struct AuditJob {
    pub action: Action,
    /// The response envelope `sign_compound` hashes; also stored as `sidecar.response`.
    pub response_env: Value,
    /// `action.params` re-supplied for the sidecar (`sidecar.request`).
    pub request: Value,
    pub t0: DateTime<Utc>,
    pub t1: DateTime<Utc>,
    pub ack: oneshot::Sender<Result<ReceiptOut, AuditError>>,
}

/// What a successful append yields back to the caller.
#[derive(Debug, Clone)]
pub struct ReceiptOut {
    pub id: String,
    pub record_hash: String,
}

/// Handle to enqueue work onto the single writer task.
#[derive(Clone)]
pub struct WriterHandle {
    tx: mpsc::Sender<AuditJob>,
}

impl WriterHandle {
    /// Non-blocking enqueue for response-open mode: never waits; a full or closed
    /// channel is an error the caller logs (lossy by design).
    pub fn try_enqueue(&self, job: AuditJob) -> Result<(), AuditError> {
        self.tx
            .try_send(job)
            .map_err(|e| AuditError::Unavailable(format!("audit queue: {e}")))
    }

    /// Bounded-wait enqueue for response-closed mode.
    pub async fn enqueue_timeout(&self, job: AuditJob, d: Duration) -> Result<(), AuditError> {
        self.tx
            .send_timeout(job, d)
            .await
            .map_err(|e| AuditError::Unavailable(format!("audit enqueue timed out: {e}")))
    }
}

/// Spawn the single writer task and return a handle. Must be called within a tokio
/// runtime.
pub fn spawn_writer(dir: PathBuf, key: AuditKey, capacity: usize) -> WriterHandle {
    let (tx, mut rx) = mpsc::channel::<AuditJob>(capacity.max(1));
    tokio::spawn(async move {
        // Seed monotonic state from the newest existing record so a restart can't
        // append an older-dated record after a newer one.
        let mut last_ts: Option<DateTime<Utc>> = seed_last_ts(&dir);
        while let Some(job) = rx.recv().await {
            process(&dir, &key, &mut last_ts, job).await;
        }
    });
    WriterHandle { tx }
}

/// Seed `last_ts` from the newest existing record (signet `query` is global
/// newest-first; `limit:1` returns the last appended record).
///
/// Returns the **upper bound** of that record's timestamps, version-aware (v2 →
/// `max(ts_request, ts_response)`; v3 → `ts_response`; else `ts`), so a restart can
/// never let a later `ts_request` regress below the prior record's response time
/// (R2-1 / C4 review Q1, Q5). Empty/corrupt logs degrade to `None` (first job uses
/// its own `t0`); a query error is logged rather than silently swallowed.
fn seed_last_ts(dir: &Path) -> Option<DateTime<Utc>> {
    let filter = signet_core::audit::AuditFilter {
        limit: Some(1),
        ..Default::default()
    };
    let records = match signet_core::audit::query(dir, &filter) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "audit: cannot read existing chain to seed writer; starting unseeded");
            return None;
        }
    };
    let rec = &records.first()?.receipt;
    let version = rec.get("v").and_then(|v| v.as_u64()).unwrap_or(1);
    let fields: &[&str] = match version {
        2 => &["ts_request", "ts_response"],
        3 => &["ts_response"],
        _ => &["ts"],
    };
    fields
        .iter()
        .filter_map(|f| rec.get(*f).and_then(|v| v.as_str()))
        .filter_map(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .max()
}

async fn process(dir: &Path, key: &AuditKey, last_ts: &mut Option<DateTime<Utc>>, job: AuditJob) {
    let dir = dir.to_path_buf();
    let signing = key.signing().clone();
    let owner = key.owner.clone();
    let prev_last = *last_ts;
    let AuditJob {
        action,
        response_env,
        request,
        t0,
        t1,
        ack,
    } = job;

    // Sign + sidecar + append happen together inside spawn_blocking under a
    // cross-process exclusive lock. Timestamps are re-clamped against BOTH the
    // in-process monotonic floor and the on-disk latest (read under the lock), so the
    // chain stays linear even with multiple processes sharing the dir (design §4.7).
    // Never aborted: the writer awaits this before taking the next job.
    let res =
        tokio::task::spawn_blocking(move || -> Result<(ReceiptOut, DateTime<Utc>), AuditError> {
            let _lock = DirLock::acquire(&dir)?;

            let floor = [prev_last, seed_last_ts(&dir)].into_iter().flatten().max();
            let ts_req = match floor {
                Some(f) => t0.max(f),
                None => t0,
            };
            let ts_resp = t1.max(ts_req);

            let receipt = signet_core::sign_compound(
                &signing,
                &action,
                &response_env,
                SIGNER_NAME,
                &owner,
                &ts_req.to_rfc3339(),
                &ts_resp.to_rfc3339(),
            )
            .map_err(|e| AuditError::Signet(e.to_string()))?;

            let receipt_value =
                serde_json::to_value(&receipt).map_err(|e| AuditError::Io(e.to_string()))?;
            let sidecar_value = json!({ "request": request, "response": response_env });
            sidecar::write(&dir, &receipt.id, &sidecar_value)?;
            let record = signet_core::audit::append(&dir, &receipt_value)
                .map_err(|e| AuditError::Signet(e.to_string()))?;

            Ok((
                ReceiptOut {
                    id: receipt.id,
                    record_hash: record.record_hash,
                },
                ts_resp,
            ))
        })
        .await;

    let out = match res {
        Ok(Ok((out, ts_resp))) => {
            *last_ts = Some(ts_resp);
            Ok(out)
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(AuditError::Io(format!("audit writer task: {e}"))),
    };
    // Log failures even when nobody is listening (open mode drops the receiver), so a
    // post-enqueue sign/sidecar/append error is never silent (final review MEDIUM).
    if let Err(e) = &out {
        tracing::error!(error = %e, "audit record failed to commit");
    }
    let _ = ack.send(out);
}

/// A process-wide exclusive lock over the whole audit dir, held across the
/// sign+sidecar+append sequence so concurrent processes can't fork the chain
/// (released on drop).
struct DirLock {
    file: std::fs::File,
}

impl DirLock {
    fn acquire(dir: &Path) -> Result<Self, AuditError> {
        std::fs::create_dir_all(dir).map_err(|e| AuditError::Io(e.to_string()))?;
        let file = std::fs::File::create(dir.join(".audit.lock"))
            .map_err(|e| AuditError::Io(e.to_string()))?;
        // std file locking (stable since Rust 1.89): blocks until the lock is held.
        file.lock().map_err(|e| AuditError::Io(e.to_string()))?;
        Ok(Self { file })
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::key::AuditKey;

    fn action(n: usize) -> Action {
        Action {
            tool: format!("site.res.act{n}"),
            params: json!({ "n": n }),
            params_hash: String::new(),
            target: "https://api.example".into(),
            transport: "https".into(),
            session: None,
            call_id: Some(format!("call{n}")),
            response_hash: None,
            trace_id: Some(format!("trace{n}")),
            parent_receipt_id: None,
        }
    }

    async fn enqueue_wait(
        h: &WriterHandle,
        action: Action,
        t0: DateTime<Utc>,
        t1: DateTime<Utc>,
    ) -> Result<ReceiptOut, AuditError> {
        let (ack, rx) = oneshot::channel();
        let job = AuditJob {
            request: action.params.clone(),
            response_env: json!({ "transport_ok": true, "data": {} }),
            action,
            t0,
            t1,
            ack,
        };
        h.enqueue_timeout(job, Duration::from_secs(5))
            .await
            .unwrap();
        rx.await.unwrap()
    }

    #[tokio::test]
    async fn writes_unbroken_chain_with_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        let h = spawn_writer(dir.path().to_path_buf(), key, 64);

        let now = Utc::now();
        let mut ids = Vec::new();
        for n in 0..5 {
            let out = enqueue_wait(&h, action(n), now, now).await.unwrap();
            assert!(out.id.starts_with("rec_"));
            assert!(!out.record_hash.is_empty());
            ids.push(out.id);
        }

        // chain integrity + a sidecar per receipt
        let status = signet_core::audit::verify_chain(dir.path()).unwrap();
        assert!(status.valid, "chain should be intact: {status:?}");
        for id in &ids {
            assert!(
                sidecar::read(dir.path(), id).is_ok(),
                "missing sidecar {id}"
            );
        }
    }

    #[tokio::test]
    async fn monotonic_ts_keeps_chain_linear_across_midnight() {
        let dir = tempfile::tempdir().unwrap();
        let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        let h = spawn_writer(dir.path().to_path_buf(), key, 64);

        // Enqueue a day-2 job FIRST, then a day-1 job (out of date order). The
        // monotonic clamp must keep the chain linear (no older-dated file after a
        // newer one).
        let day2 = DateTime::parse_from_rfc3339("2026-06-22T00:00:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let day1 = DateTime::parse_from_rfc3339("2026-06-21T23:59:30Z")
            .unwrap()
            .with_timezone(&Utc);
        enqueue_wait(&h, action(1), day2, day2).await.unwrap();
        enqueue_wait(&h, action(2), day1, day1).await.unwrap();

        let status = signet_core::audit::verify_chain(dir.path()).unwrap();
        assert!(
            status.valid,
            "chain must stay linear across reordered dates: {status:?}"
        );
    }

    #[tokio::test]
    async fn restart_seeds_from_chain_and_stays_linear() {
        let dir = tempfile::tempdir().unwrap();

        // Writer #1: append a record whose response crosses midnight.
        {
            let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
            let h = spawn_writer(dir.path().to_path_buf(), key, 16);
            let t0 = DateTime::parse_from_rfc3339("2026-06-21T23:59:59Z")
                .unwrap()
                .with_timezone(&Utc);
            let t1 = DateTime::parse_from_rfc3339("2026-06-22T00:00:05Z")
                .unwrap()
                .with_timezone(&Utc);
            enqueue_wait(&h, action(1), t0, t1).await.unwrap();
            // drop h → writer task drains and ends
        }

        // Writer #2 (simulated restart): seeds last_ts from the existing chain, then
        // appends a job whose t0 is between the prior ts_request and ts_response.
        {
            let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
            let h = spawn_writer(dir.path().to_path_buf(), key, 16);
            let t = DateTime::parse_from_rfc3339("2026-06-22T00:00:02Z")
                .unwrap()
                .with_timezone(&Utc);
            enqueue_wait(&h, action(2), t, t).await.unwrap();
        }

        let status = signet_core::audit::verify_chain(dir.path()).unwrap();
        assert!(
            status.valid,
            "chain must stay linear across restart: {status:?}"
        );
    }

    #[tokio::test]
    async fn try_enqueue_errs_when_full() {
        let dir = tempfile::tempdir().unwrap();
        let key = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        let h = spawn_writer(dir.path().to_path_buf(), key, 1);

        // Flood without draining acks; a bounded channel must eventually reject.
        let now = Utc::now();
        let mut rejected = false;
        for n in 0..256 {
            let (ack, _rx) = oneshot::channel();
            let a = action(n);
            let job = AuditJob {
                request: a.params.clone(),
                response_env: json!({ "transport_ok": true }),
                action: a,
                t0: now,
                t1: now,
                ack,
            };
            if h.try_enqueue(job).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected, "a bounded queue should reject under flood");
    }
}
