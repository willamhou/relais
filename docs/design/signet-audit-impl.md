# Implementation spec: Signet call auditing

- **Status:** Draft v4 (GO — cleared Codex impl-review rounds 1, 2 & GO/NO-GO)
- **Companion design:** [`signet-audit-integration.md`](./signet-audit-integration.md) (Draft v4, 4 review rounds)
- **Author:** willamhou (with Claude)
- **Date:** 2026-06-21

This is the buildable plan for the design spec. It is organised into **independently
implementable chapters (C1…C8)**. Each chapter lists *files*, *exact
signatures/types*, *tests*, and *acceptance* so it can be implemented and
Codex-reviewed on its own, then this doc is updated before the next chapter.

> **Progress legend (updated as chapters land):** ☐ not started · ◐ in progress ·
> ☑ done & reviewed.
> C1 ☑ · C2 ☑ (RQ3 accepted) · C3 ☑ · C4 ☑ · C5 ☑ · C6 ☑ · C7 ☑ (closed-mode fail-hard via build_exec_router; tail limit-after-filter; server audit feature; --owner) · C8 ☐ (optional, deferred to M3)
>
> **C1–C7 implemented.** Final whole-feature Codex review + self-review + full local
> regression next; then push.
>
> **C5 scope note:** C5 lands the choke point (`Router::exec`) and the sink, and
> routes server+CLI through it, but `build_router()` still returns an **unaudited**
> router — **runtime enablement** (`with_audit(...)` driven by config, plus the
> `audit` feature passthrough on `relais-cli`/`relais-server`) is wired in **C7**
> alongside `relais audit init`. Open mode is **log-only** in v1 (a metric is added
> when metrics infra exists).
>
> **Impl note for C6 (pubkey format):** signet stores the `.pub`/`KeyInfo` pubkey as
> **raw STANDARD base64**, but receipts' `signer.pubkey` is **`ed25519:`-prefixed**
> (`format_pubkey`). `AuditKey.pubkey` is normalised to the `ed25519:` form so it
> matches receipts and `trusted_keys.json` directly. C6 trust comparison must use
> the same prefixed form (or compare raw `VerifyingKey` bytes).

## 0. Pinned facts & dependencies

- **`signet-core = "0.10"`** (workspace clone is 0.10.0). **C1 must confirm the
  crates.io-published 0.10 API matches** the signatures below (the clone may be
  ahead); if not, pin the exact published version and re-verify in C1.
- **Key types are `ed25519_dalek::{SigningKey, VerifyingKey}`** — signet-core does
  **not** re-export them (IR-1). Add `ed25519-dalek = "2"` under the audit feature
  and use `ed25519_dalek::{SigningKey, VerifyingKey}` everywhere.
- Hash recompute (C6) **must** match signet byte-for-byte → use the **same crates**:
  `json-canon = "0.1"`, `hex = "0.4"`, and **`sha2 = "0.10"` which is ALREADY a
  non-optional core dep** (used by `vault.rs`) — reuse it, do **not** re-add it as
  audit-optional (GO-B2). signet's `canonical::canonicalize` = `json_canon::to_string`.
- All audit code is behind a **`audit` cargo feature** on `relais-core`; default
  **off** for v1 (opt-in).
- `tokio::sync::{mpsc, oneshot}` + `tokio::task::spawn_blocking` for the writer (C4).
- `chrono` (already a core dep) for RFC-3339 timestamps.

## 1. Module layout (target)

```
crates/core/
├── Cargo.toml                     # + [features] audit; optional deps incl ed25519-dalek (C1)
└── src/
    ├── types.rs                   # + ReceiptHandle, ResponseMeta.receipt (C1)
    ├── error.rs                   # + SiteNotFound, AuditUnavailable, (M3) PolicyDenied/NeedsApproval (C1)
    ├── router.rs                  # + (gated) audit field, async exec() present in BOTH builds (C5)
    └── audit/                     # all behind feature = "audit"
        ├── mod.rs                 # AuditSink, AuditConfig, AuditError (C1 skel → C5)
        ├── redact.rs              # value+key+secret-substring redaction, _relais_audit (C2)
        ├── envelope.rs            # ExecContext/Response → Action + response envelope (C2)
        ├── key.rs                 # AuditKey load/gen via signet fs_ops; CredRefStore (C3)
        ├── writer.rs              # single-writer task: sign + sidecar + append, monotonic dates (C4)
        ├── sidecar.rs             # sidecar preimage persist + path layout (C4/C6)
        └── verify.rs              # chain + per-record windowed verify + sidecar recompute (C6)
crates/cli/src/commands/audit.rs   # relais audit {init,verify,tail,pubkey} (C7)
crates/server/src/handlers.rs      # exec → router.exec (C5)
crates/cli/src/commands/exec.rs    # exec → router.exec (C5)
.github/workflows/ci.yml           # guard: no direct adapter .exec( outside tests (C5)
```

---

## C1 — Feature scaffold + core types

**Files:** `crates/core/Cargo.toml`, `crates/core/src/types.rs`,
`crates/core/src/error.rs`, `crates/core/src/audit/mod.rs`, `crates/core/src/lib.rs`,
**and every existing `ResponseMeta { … }` literal** (IR-8/R2-LOW): the implementer
**must `rg "ResponseMeta\s*\{"` to find them all** — known non-test sites include
`crates/adapters/github/src/lib.rs:110`, `crates/adapters/hackernews/src/lib.rs:53`,
**`:173`, `:208`**, `crates/adapters/scs/src/lib.rs:139`,
`crates/adapters/scs-legacy/src/lib.rs` (its `ResponseMeta` sites),
`crates/llm-fallback/src/lib.rs:145`, plus test literals.

**Cargo:**
```toml
[features]
default = []
audit = ["dep:signet-core","dep:ed25519-dalek","dep:json-canon","dep:hex","dep:tokio","dep:tracing"]

[dependencies]
# sha2 = "0.10" is ALREADY a non-optional core dep (vault.rs) — reuse it, do NOT list
# it here as optional (GO-B2). chrono/serde/serde_json/anyhow are already core deps too.
signet-core   = { version = "0.10", optional = true }
ed25519-dalek = { version = "2", optional = true }
json-canon    = { version = "0.1", optional = true }
hex           = { version = "0.4", optional = true }
tokio         = { version = "1", features = ["sync","rt","time"], optional = true }  # currently dev-only; add as optional runtime dep
tracing       = { version = "0.1", optional = true }   # used by the audit Router branch (C5, GO-B3)
```

**types.rs (additive, wire-compatible — present in ALL builds, F-12):**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptHandle {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_hash: Option<String>,   // None in response-open (append not yet acked) — IR-4
}
// in ResponseMeta:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub receipt: Option<ReceiptHandle>,
```
To avoid touching every literal twice, **add `#[derive(Default)]` to `ResponseMeta`**
and migrate the literals that set all old fields to `..Default::default()` where it
reads cleanly; otherwise add `receipt: None` explicitly. The field is **not**
feature-gated out of the wire type (only its population is).

**error.rs (additive):**
```rust
#[error("site not found: {0}")]   SiteNotFound(String),     // Router::exec (currently missing!)
#[error("audit unavailable: {0}")] AuditUnavailable(String), // response-closed sink failure
// M3 only (C8): PolicyDenied(String), NeedsApproval(String)
```

**audit/mod.rs:** skeleton behind `#![cfg(feature = "audit")]` — `AuditConfig`,
`AuditSink`, and an `AuditError` (thiserror) that `From`-converts into
`AdapterError::AuditUnavailable`. Compiles with `todo!()` bodies.

**Tests / acceptance:** `cargo build` (no features) and `--features audit` both
compile; default `cargo test` green after literal updates; serde round-trip of
`ResponseMeta` across builds (no `receipt`/`record_hash` field when `None`);
**a throwaway test calls the published `sign_compound`/`audit::append`/`verify_compound`
signatures to confirm the 0.10 API.**

---

## C2 — Redaction + envelope mapping (pure, no I/O)

**Files:** `crates/core/src/audit/redact.rs`, `crates/core/src/audit/envelope.rs`.

**Redaction (`redact.rs`) — masks by key name AND by secret value (IR-6):**
```rust
pub struct Redactor { denylist: Vec<String> }   // keys: token,password,secret,authorization,
//                                                  api_key,acs_token,cookie, + *_token suffix
impl Redactor {
    // `secret_values`: the actual credential strings pulled from ctx.credentials
    // (bearer_token(), cookie values). Any string leaf that EQUALS or CONTAINS one
    // is masked, regardless of its key — this is what passes the leak guard.
    pub fn redact_value(&self, v: &Value, secret_values: &[String]) -> Value;
}
pub fn secret_values_of(creds: &Option<Credentials>) -> Vec<String>;
//   Match ALL CredentialData variants DIRECTLY (R2-HIGH/IR-6) — do NOT use
//   bearer_token() (it omits refresh_token):
//     ApiKey  => [token]
//     OAuth   => [access_token, refresh_token?]        // BOTH
//     Cookie  => cookies.values()                       // every cookie value
pub struct AuditMeta { pub auth_injection: String, pub credential_ref: String,
                       pub t0: String, pub t1: String }   // TRUE request start/end (RFC3339)
//   credential_ref: OPAQUE random handle (kref_…), resolvable only via local CredRefStore (C3).
//   NEVER the vault site id or a token hash.
//   t0/t1: the true wall-clock request window. They live in `_relais_audit` (inside
//   action.params, covered by params_hash → signed/tamper-evident), so the receipt's
//   top-level `ts_request` can be the monotonic AUDIT-ORDER time (C4/R2-HIGH) without
//   losing the real request time auditors need.
```

**Envelope (`envelope.rs`):**
```rust
pub fn build_request(ctx: &ExecContext, meta: &AuditMeta, r: &Redactor, secrets: &[String]) -> Value;
//   redact_value(ctx.params) + { "_relais_audit": { auth_injection, credential_ref, t0, t1 } }
pub fn build_response_envelope(result: &Result<Response, AdapterError>, r: &Redactor, secrets: &[String]) -> Value;
//   Ok  => { "transport_ok": true,  "data": redact(data), "business_status": "unclassified" }
//   Err => { "transport_ok": false, "error": { "kind": "<variant>", "message": "<text>" } }
pub fn build_action(ctx: &ExecContext, request: Value, base_url: &str,
                    session: Option<String>, trace_id: String, call_id: String) -> signet_core::Action;
```
`build_action` constructs `signet_core::Action` with the **real field names** (IR-2):
```rust
signet_core::Action {
    tool: format!("{}.{}.{}", ctx.site, ctx.resource, ctx.action),
    params: request,                 // becomes BOTH Action.params and sidecar.request (same Value)
    params_hash: String::new(),      // signet fills it in sign_compound
    target: base_url.to_string(),    // exactly manifest().base_url
    transport: "https".into(),
    session, call_id: Some(call_id),
    response_hash: None,
    trace_id: Some(trace_id),
    parent_receipt_id: None,         // NOT `parent`
}
```

**Tests / acceptance:** key-name + `*_token` masking; **secret-value masking** (a
token echoed under an arbitrary key/string is masked); **secret-leak guard**: given
an `ExecContext` whose credential token also appears in a param and in the response
body, the serialized `Action` + request + response envelopes contain none of the
token bytes; `transport_ok` reflects Ok/Err; golden envelope test.

---

## C3 — Key management

**Files:** `crates/core/src/audit/key.rs`.

```rust
use ed25519_dalek::SigningKey;                  // IR-1
pub struct AuditKey { signing: SigningKey, pub pubkey_b64: String, pub owner: String }
impl AuditKey {
    pub fn load_or_init(dir: &Path, owner: &str, passphrase: Option<&str>) -> Result<Self, AuditError>;
    pub fn signing(&self) -> &SigningKey;
}
pub struct CredRefStore { /* dir/credential_refs.json */ }
impl CredRefStore {
    pub fn load(dir: &Path) -> Result<Self, AuditError>;
    pub fn mint(&mut self, binding: CredBinding) -> Result<String /*kref_*/, AuditError>; // random ref
}
```
- `load_or_init` uses signet `identity::fs_ops::{load_signing_key(dir,"relais",pass),
  generate_and_save(dir,"relais",Some(owner),pass,None)}` → keys at
  `dir/keys/relais.{key,pub}` (NF-7). **Does NOT auto-trust** for verification (NF-4).
- `CredRefStore` maps opaque `kref_…` → vault binding; the reverse map is **never**
  serialized into receipts/sidecars and is omitted from exports.

**Tests / acceptance:** init creates `dir/keys/relais.key` (0600) + `.pub`; reload
reuses; pubkey stable; `mint` unique; cred-ref map never appears in any receipt JSON.

---

## C4 — Single-writer task: sign + sidecar + append (monotonic), the §4.7 contract

**Files:** `crates/core/src/audit/writer.rs`, `crates/core/src/audit/sidecar.rs`.

**The writer OWNS `sign_compound` + sidecar + `append`** (moved out of `record`) so a
single sequential task controls both ordering and timestamps — this fixes the
cross-midnight chain-fork (IR-5) and makes `record_hash` available (IR-4).

```rust
pub struct AuditJob {
    pub action: signet_core::Action,   // already built+redacted (C2)
    pub response_env: Value,           // hashed by sign_compound AND stored as sidecar.response
    pub request: Value,                // = action.params, stored as sidecar.request
    pub t0: DateTime<Utc>, pub t1: DateTime<Utc>,
    pub ack: oneshot::Sender<Result<ReceiptOut, AuditError>>,
}
pub struct ReceiptOut { pub id: String, pub record_hash: String }
pub struct WriterHandle { tx: mpsc::Sender<AuditJob> }
impl WriterHandle {
    pub fn try_enqueue(&self, job) -> Result<(), AuditError>;                 // open (try_send)
    pub async fn enqueue_timeout(&self, job, d: Duration) -> Result<(), AuditError>; // closed
}
pub fn spawn_writer(dir: PathBuf, key: AuditKey, cap: usize) -> WriterHandle;
```

**`spawn_writer` seeds `last_ts` from the existing chain BEFORE accepting jobs
(R2-HIGH):** read the newest existing audit record's `ts_request` (via
`query`/last-record scan) and initialise `last_ts` to it. Without this, after a
restart or backward clock skew the first new job could carry a `ts_request` older
than the on-disk tail and append an older-dated record after a newer one — forking
the chain. Seeding makes monotonicity hold across process lifetimes.

**Writer loop (exact):** for each `AuditJob`, sequentially:
1. **Monotonic AUDIT-ORDER timestamp (IR-5, R2-HIGH):** `ts_req = max(t0, last_ts)`;
   `ts_resp = max(t1, ts_req)`; update `last_ts = ts_resp`. Non-decreasing
   `ts_request` ⇒ signet's per-date file selection (`extract_timestamp`) never files
   an older date after a newer one ⇒ the chain stays linear across midnight/restart.
   **Semantics:** the receipt's top-level `ts_request`/`ts_response` are defined as
   the *audit ordering/signing* time, **not** necessarily the real request start.
   The **true** request window is carried in signed `_relais_audit.t0/t1` (C2), so no
   information is lost and key-rotation windows (C6) key on the same monotonic
   `ts_request` consistently. (Clamps are sub-second except at a real rollover race.)
2. `receipt = sign_compound(key.signing(), &job.action, &job.response_env, "relais",
   &key.owner, &ts_req.to_rfc3339(), &ts_resp.to_rfc3339())?` → has `id`.
3. `sidecar::write(dir, &receipt.id, &json!({ "request": job.request, "response": job.response_env }))`
   (atomic tmp+rename), **before** the append.
4. `let rec = spawn_blocking(move || signet_core::audit::append(&dir, &receipt_value)).await??;`
   — **never aborted**; sequential processing guarantees order.
5. `let _ = ack.send(Ok(ReceiptOut { id: receipt.id, record_hash: rec.record_hash }));`
   — **tolerate a dropped receiver** (open mode ignores the ack): once the append has
   succeeded, a `send` failure is **not** an append failure (R2-MEDIUM).

Caller-side timeouts (C5) bound the request's *wait*, never the append. Bounded
channel: full → `try_enqueue` errs (open, lossy + log), `enqueue_timeout` errs after
deadline (closed). Graceful shutdown drains with a bounded deadline.

**Cross-process chain ownership (design §4.7, R2-HIGH).** A single in-process writer
prevents intra-process forks, but the **server and CLI are separate processes** and
must not both write one chain via per-date locks. v1 default: **the server owns the
chain; the CLI writes a SEPARATE namespace** (its own subdir, e.g.
`dir/cli/audit/…`, with its own `signer_owner`). The unified-chain alternative is an
opt-in **global cross-process lock over the whole audit dir** (never per-date). C5
acceptance must assert server and CLI never share one per-date-locked chain.

**sidecar.rs:** `write(dir,id,&Value)`, `read(dir,id)->Value`, `dir/sidecars/<id>.json`.

**Tests / acceptance:** 500 concurrent enqueues → single unbroken chain
(`verify_chain` Ok) + a sidecar per id; **cross-midnight test** (feed t0/t1 straddling
00:00 out of order) still yields a linear chain; backpressure errors, no deadlock;
no-abort probe (slow append doesn't let the next start early); append runs on a
blocking thread.

---

## C5 — AuditSink + `Router::exec` + call-site rewire

**Files:** `crates/core/src/audit/mod.rs`, `crates/core/src/router.rs`,
`crates/server/src/handlers.rs`, `crates/cli/src/commands/exec.rs`,
`.github/workflows/ci.yml`.

```rust
pub enum AuditMode { Open, Closed }
pub struct AuditConfig { pub dir: PathBuf, pub owner: String, pub mode: AuditMode,
                         pub capacity: usize, pub ack_timeout: Duration }
pub struct AuditSink { writer: WriterHandle, credrefs: Mutex<CredRefStore>,
                       redactor: Redactor, mode: AuditMode, ack_timeout: Duration }
impl AuditSink {
    pub fn new(cfg: AuditConfig) -> Result<Self, AuditError>;   // load_or_init key + spawn_writer
    // base_url passed IN (ExecContext has none) — IR-3
    pub async fn record(&self, ctx: &ExecContext, base_url: &str,
                        result: &Result<Response,AdapterError>,
                        t0: DateTime<Utc>, t1: DateTime<Utc>) -> Result<Option<ReceiptHandle>, AuditError>;
    pub fn closed(&self) -> bool;
}
```
`record`: mint the `credential_ref` in a **tight synchronous scope** —
`{ let mut g = self.credrefs.lock().unwrap(); g.mint(binding) }` — and **drop the
guard before any `.await`** (R2-MEDIUM), so the handler future stays `Send` and the
non-async `std::sync::Mutex` is never held across await. Then build
`secrets`/`meta`/`request`/`response_env`/`action` (C2) → assemble `AuditJob` →
- **Open:** `try_enqueue`; on full → log + return `Ok(None)` (best-effort, no handle —
  IR-4). On success **do not await** → return `Ok(Some(ReceiptHandle{ id: <known after
  sign? no> }))`. Since signing now happens in the writer, **open mode returns
  `Ok(None)`** (no id/hash without awaiting). *(If a handle is desired in open mode,
  await the ack with a short timeout; default v1 = `None`.)*
- **Closed:** `enqueue_timeout(ack_timeout)` then `await ack` (also bounded) →
  `Ok(Some(ReceiptHandle{ id, record_hash: Some(hash) }))`; any failure →
  `Err(AuditUnavailable)`.

**Router (present in BOTH builds; only the field/sink are gated — IR-7):**
```rust
pub struct Router {
    adapters: HashMap<String, Box<dyn Adapter>>,
    #[cfg(feature = "audit")] audit: Option<AuditSink>,
}
impl Router {
    #[cfg(feature = "audit")] pub fn with_audit(mut self, s: AuditSink) -> Self { self.audit = Some(s); self }
    pub async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let adapter = self.get(&ctx.site).ok_or_else(|| AdapterError::SiteNotFound(ctx.site.clone()))?;
        // base_url + timing live ONLY inside the audit path so non-audit builds emit
        // no unused-variable warnings (R2-MEDIUM):
        #[cfg(feature = "audit")]
        if let Some(sink) = &self.audit {
            let base_url = adapter.manifest().base_url;
            let t0 = Utc::now();
            let mut result = adapter.exec(ctx).await;
            let t1 = Utc::now();
            match sink.record(ctx, &base_url, &result, t0, t1).await {
                Ok(h)  => if let Ok(resp) = &mut result { resp.meta.receipt = h; },
                Err(e) => if sink.closed() { return Err(AdapterError::AuditUnavailable(e.to_string())); }
                          else { tracing::error!(error=%e, "audit sink failed (response-open)"); }
            }
            return result;
        }
        adapter.exec(ctx).await        // non-audit build, or audit feature on but no sink configured
    }
}
```
`exec` exists in both builds; the audit branch is fully `#[cfg]`-gated, so non-audit
builds reduce to today's `adapter.exec(ctx).await` with no dead bindings, and both
call sites compile with and without the feature.

**Rewire:** `handlers.rs:165` and `exec.rs:76` → `router.exec(&ctx).await`. **CI
guard:** grep for adapter-handle `\.exec(` in `crates/server/src/**` and
`crates/cli/src/**` (production dirs only, excluding `tests/`), failing on a direct
call — scoped to avoid false positives on adapter-internal code.

**Tests / acceptance:** with a stub adapter + sink, `router.exec` writes one record +
populates `ResponseMeta.receipt` (closed mode); without the feature, behaviour
unchanged; closed-mode forced failure → `AuditUnavailable`, response withheld;
open-mode forced failure → logged, response returned; server + CLI integration green.

---

## C6 — Verification (chain + windowed per-record + sidecar recompute)

**Files:** `crates/core/src/audit/verify.rs`.

```rust
use ed25519_dalek::VerifyingKey;                 // IR-1
pub struct TrustedKey { pub pubkey_b64: String, pub status: KeyStatus,
                        pub not_before: DateTime<Utc>, pub not_after: Option<DateTime<Utc>> }
pub struct TrustAnchor { keys: Vec<TrustedKey> }
impl TrustAnchor {
    pub fn load(dir: &Path) -> Result<Self, AuditError>;     // ERROR if missing/empty (NF-4)
    fn key_for(&self, ts: DateTime<Utc>) -> Option<VerifyingKey>;
}
pub struct VerifyReport { pub records: usize, pub chain_ok: bool, pub failures: Vec<String> }
pub fn audit_verify(dir: &Path, anchor: &TrustAnchor) -> Result<VerifyReport, AuditError>;
```
`audit_verify`:
1. `verify_chain(dir)?` → `chain_ok`.
2. `let records = query(dir, &AuditFilter::default())?;` (ordered).
3. per record: `ts = record.receipt["ts_request"]`; `key = anchor.key_for(ts)` (fail
   if none); **deserialize then verify (IR-9):**
   ```rust
   let cr: signet_core::CompoundReceipt = serde_json::from_value(record.receipt.clone())?;
   signet_core::verify_compound(&cr, &key)?;   // or verify_any(&record.receipt.to_string(), &key)
   ```
4. **sidecar recompute (RD-1, exact):**
   ```rust
   let s = sidecar::read(dir, &cr.id)?;
   let canon = json_canon::to_string(&s["response"])?;
   let expect = format!("sha256:{}", hex::encode(sha2::Sha256::digest(canon.as_bytes())));
   if expect != cr.response.content_hash { failures.push(...) }
   // same prefixed rule: hash s["request"] vs cr.action.params_hash
   ```
5. collect failures → report.

**Tests / acceptance:** happy path verifies; byte-flip → `chain_ok=false`; sidecar
mutation → recompute failure; missing/empty anchor → `load` errors (no self-trust);
rotation-window positive/negative; **cross-impl pin:** a receipt from real
`sign_compound` with its `response_content` as sidecar recomputes equal to
`content_hash` (guards prefix/JCS/bytes).

---

## C7 — CLI: `relais audit {init,verify,tail,pubkey}`

**Files:** `crates/cli/src/commands/audit.rs`, CLI arg wiring, `relais-cli` `audit`
feature passthrough.

- `relais audit init` → `AuditKey::load_or_init` (+ `--owner`, passphrase prompt);
  prints pubkey + dir.
- `relais audit pubkey` → prints `dir/keys/relais.pub`.
- `relais audit verify` → `TrustAnchor::load` + `audit_verify`; **non-zero exit on any
  failure or empty anchor**.
- `relais audit tail [--site <id>] [--since <rfc3339>] [--limit N]` → `query` with an
  `AuditFilter` (`since`/`tool`/`signer`/`limit`); `--site` maps to a `tool`-prefix
  filter applied in relais.

**Tests / acceptance:** `init`→`verify` (with anchor) on a fresh chain → exit 0;
`verify` no anchor → non-zero + message; `tail` filters; CLI exec path (C5) writes
records `verify` accepts.

---

## C8 — (Optional, M3) Policy gate + JWT attribution

**Files:** `crates/core/src/audit/policy.rs`, `crates/core/src/router.rs`,
`crates/server/src/handlers.rs`.

- Pre-exec gate in `Router::exec` **before** `adapter.exec` using
  `signet_core::evaluate_policy` + `signet_core::audit::append_violation` (note the
  paths — `append_violation` is under `audit`, not a root export — IR-10);
  Deny/RequireApproval → `AdapterError::{PolicyDenied,NeedsApproval}` (no exec). v1
  ships an allow-all default + the hook only.
- Server fills `Action.session` with the JWT subject; CLI with the local user.

**Acceptance:** deny blocks `exec` + writes a violation; allow-all is a no-op.
**Optional** — ship C1–C7 first.

---

## Implementation order & loop protocol

Build **C1 → C2 → C3 → C4 → C5 → C6 → C7** (C8 optional). Per chapter:
1. Implement it.
2. Update this doc's progress legend (☐→◐→☑) and any signature that changed in reality.
3. Codex-review **the code**; fix; re-review until clean.
4. Next chapter.

After C7 (and C8 if included): one **final** Codex review + self code-review, then
`cargo test` / `cargo build --features audit` regression, then push.

### Cross-cutting acceptance (whole feature)
- `cargo build` (default) and `--features audit` clean; `cargo clippy --features audit` clean.
- `cargo test` (default) unchanged; `cargo test --features audit` green.
- Full exec→receipt→verify round-trip (C5+C6) incl. tamper + sidecar + rotation +
  cross-midnight negatives.
- Secret-leak guard (C2) passes for request *and* response, incl. value-substring leaks.
- No production path calls `adapter.exec` directly (CI guard, C5).
- Release: **no new crate** — `signet-core` is a transitive dep; existing 8-crate
  tag-triggered release (RELEASING.md) unchanged, minor bump.

---

## Appendix — Codex impl-review round 1: findings & resolutions

| # | Sev | Finding | Resolution in v2 |
|---|-----|---------|------------------|
| IR-1 | BLOCKER | `SigningKey`/`VerifyingKey` not signet root exports | §0/C1 add `ed25519-dalek = "2"`; use `ed25519_dalek::{SigningKey,VerifyingKey}` (C3/C4/C6) |
| IR-2 | BLOCKER | `Action.parent` doesn't exist | C2 `build_action` uses `parent_receipt_id: None` + real `trace_id`/`call_id` names |
| IR-3 | BLOCKER | `record` can't get `base_url` (not on `ExecContext`) | C5 `Router::exec` reads `adapter.manifest().base_url` and passes it into `record(ctx, base_url, …)` |
| IR-4 | BLOCKER | open mode can't populate `record_hash` (append not done) | C4 writer owns sign+append and returns `record_hash` on ack; `ReceiptHandle.record_hash: Option`; open mode returns `None` |
| IR-5 | HIGH | sequential writer ≠ cross-day chain integrity | C4 writer stamps **monotonic non-decreasing `ts_request`** → date files never regress; cross-midnight test |
| IR-6 | HIGH | key-only redaction fails the secret-leak guard | C2 redactor also masks **secret values** from `ctx.credentials` (equals/contains), any key |
| IR-7 | HIGH | Router feature-gating under-specified for no-audit | C5 `Router::exec` exists in **both** builds; only the `audit` field + `with_audit` + record block are `#[cfg]` |
| IR-8 | MEDIUM | `ResponseMeta` literal sites beyond C1's list | C1 file list expanded to all literals; `#[derive(Default)]` + `..Default::default()` migration |
| IR-9 | MEDIUM | `verify_compound` needs `&CompoundReceipt`, not `Value` | C6 `serde_json::from_value::<CompoundReceipt>` (or `verify_any(&json_string, key)`) |
| IR-10 | LOW | `append_violation` not a root export | C8 uses `signet_core::audit::append_violation` + `signet_core::evaluate_policy` |

## Appendix — Codex impl-review round 2: verdicts & v3 closures

Round-2 verdicts on v2 (RESOLVED kept as-is): IR-1, IR-2, IR-3, IR-4, IR-9, IR-10
RESOLVED. IR-5/6/7/8 were PARTIAL → closed below.

| ID | Sev | Problem | v3 closure |
|----|-----|---------|-----------|
| R2-1 | HIGH | Writer `last_ts` not seeded from existing logs → post-restart/skew fork | C4 `spawn_writer` seeds `last_ts` from the newest on-disk record before accepting jobs |
| R2-2 | HIGH | Clamped `ts_request` silently changes signed timestamp meaning | C2/C4: `ts_request` defined as audit-order time; **true `t0`/`t1` stored in signed `_relais_audit`** |
| R2-3 | HIGH | Cross-process chain ownership rule (design §4.7) missing from impl doc | C4: server owns chain; CLI = separate namespace; or global audit-dir lock; C5 acceptance asserts it |
| R2-4 | HIGH | OAuth `refresh_token` escapes value masking (`bearer_token()` omits it) | C2 `secret_values_of` matches all `CredentialData` variants (ApiKey/OAuth access+refresh/Cookie values) |
| R2-5 | MEDIUM | `Mutex<CredRefStore>` guard could cross `.await` (non-Send/deadlock) | C5 `record` mints in a tight sync scope and drops the guard before any await |
| R2-6 | MEDIUM | Open-mode dropped ack receiver could look like append failure | C4 `let _ = ack.send(...)`; post-append send failure is not an append failure |
| R2-7 | MEDIUM | Non-audit `base_url` binding unused → warning/error | C5 `base_url`+timing computed only inside the `#[cfg(audit)]`+`Some(sink)` branch |
| R2-8 | LOW | `ResponseMeta` literal inventory incomplete | C1: `rg "ResponseMeta\s*\{"`; added hackernews `:173`/`:208` |
