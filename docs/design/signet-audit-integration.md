# Design: Cryptographic call auditing via Signet

- **Status:** Draft v4 (revised after Codex review rounds 1, 2 & 3)
- **Author:** willamhou (with Claude)
- **Date:** 2026-06-21
- **Tracking:** —
- **Upstream:** [`Prismer-AI/signet`](https://github.com/Prismer-AI/signet) — `signet-core` on crates.io

## 1. Problem

relais is the gateway every agent action flows through: an agent calls `exec`,
relais injects a vault credential, hits an upstream API, and returns the result.
Today **nothing durable records that this happened.** There is no tamper-evident
trail of *who* asked relais to do *what*, against *which* upstream, with *what
outcome*. For an "agent internet gateway" that performs writes and deletes with
stored credentials, that is the missing accountability layer.

The ask: **record every API call**, and not as a mutable log a compromised host
can rewrite — as **proof**.

[Signet](https://github.com/Prismer-AI/signet) is a Rust-first
(`signet-core`, crates.io) cryptographic proof layer for agent tool calls:
each call becomes an **Ed25519-signed receipt**, receipts are **SHA-256
hash-chained** into an append-only audit log, and the whole chain is
**offline-verifiable** from a public key — altering or deleting any record breaks
the chain. It is the same Rust/crates.io ecosystem as relais, so integration is a
dependency, not a bridge.

**Goal:** every relais `exec` (one gateway action) emits a signed, hash-chained
receipt committing to the redacted request + response, written through one choke
point, with secrets never entering the receipt — and a CLI/CI path to verify the
chain against an *out-of-band trusted* gateway key.

### 1.1 Goals

- A **single choke point** so *every* exec path (HTTP server **and** CLI) is
  recorded, with zero per-adapter changes.
- **One CompoundReceipt per relais `exec`** binding the redacted request, a
  `sha256(JCS(response_envelope))` commitment, and request/response timestamps
  (signet `sign_compound`). Outcome is encoded inside the hashed envelope —
  `sign_compound` carries **no** separate signed `Outcome` (F-1). One receipt per
  *gateway action*, **not** per internal upstream/provider HTTP call (F-5; §6 Q2).
- A **sidecar preimage store** (§4.6): because the chain stores only the response
  *hash*, relais persists the redacted request+response **preimage** keyed by
  receipt id, so `relais audit verify` can recompute the hash and an auditor can
  actually read what happened (NF-6). The chain proves integrity; the sidecar makes
  it legible.
- A hard **redaction boundary**: vault credentials and secret-typed request/response
  fields never enter a receipt or sidecar. The receipt attests a **redacted
  semantic request**, not literal upstream wire bytes (F-3, NF-2).
- **Hash-chained, append-only** storage written through a **single sequential
  writer with a concrete enqueue/ack/timeout contract** so the chain can't fork or
  hang the request path (F-6/F-7, NF-1).
- A **response-closed** failure mode (vs default response-open), plus a genuine
  **pre-exec gate** for "deny before side effect" (F-2, §4.5/§4.8).
- A **trust-anchored, fail-closed** verification story: `relais audit verify`
  **requires an out-of-band trust anchor** (no trust-on-first-use), validates
  rotation in relais (signet's verify options are raw key vectors with no time
  window), and puts the gateway key in `trusted_agent_pubkeys` — the correct field
  for v1/v2 receipts (F-8, NF-4/NF-5, §4.11).
- Optional but designed-for: a **receipt handle** to the caller, **per-agent
  attribution** (JWT subject), and **policy gating**.

### 1.2 Non-goals (v1)

- **signet's MCP middleware / proxy.** relais *is* the gateway; embedding
  `signet-core` directly is cleaner than proxying.
- **Sub-call / fan-out receipts.** One receipt per relais exec; adapters that fan
  out (notably `relais-llm-fallback`: page fetch **and** LLM-provider call under one
  `exec`) get one gateway receipt. Per-upstream sub-receipts deferred (F-5, §6 Q2).
- **Field-level audit encryption.** signet's `append_encrypted` mutates
  `action.params` and only the *signing* key can restore it for verification (NF-3);
  encrypting fields with a separate key before `append` would invalidate the
  signature. v1 is **redaction-only**; at-rest confidentiality, if needed, is
  **transparent filesystem/volume encryption below signet** (signed bytes
  unchanged), not field-level (§4.3).
- **Bilateral (v3) receipts.** v1 signs as the gateway only (`sign_compound`, v2).
  Upstreams don't counter-sign (§6 R2).
- **Full policy engine.** signet policy hooks are wired as an *optional* pre-exec
  gate (§4.8), not a v1 requirement.
- **Replacing operational logging / metrics.** This is the *proof* layer.
- **Remote/centralized audit sink.** v1 is local; shipping is deployment-side (R4).
- **Redacting the caller-facing response.** This feature redacts the *audit log*
  only. The response relais returns to the calling agent is unchanged — it is the
  agent's own data over its own authenticated channel, and redacting it would break
  legitimate uses (an upstream may return a token the agent needs). An upstream that
  echoes an injected credential reaches the caller exactly as before this feature (no
  new exposure); only the *log* is sanitised.

## 2. Current state (what we integrate against)

- **Two exec call sites, no shared choke point.** `Router` (`crates/core/src/router.rs`)
  only registers/looks up adapters; each caller calls the adapter directly:
  - `crates/server/src/handlers.rs:165` — `adapter.exec(&ctx).await`
  - `crates/cli/src/commands/exec.rs:76` — `adapter.exec(&ctx).await`
- **`Adapter::exec(&ExecContext) -> Result<Response, AdapterError>`**
  (`crates/core/src/adapter.rs`) is the universal unit of work. It stays `pub`
  (tests call it directly); the choke point is enforced by routing + a CI guard, not
  by hiding the method (F-10, §4.1).
- **`ExecContext { site, resource, action, params: Value, credentials: Option<Credentials> }`**
  — carries **`credentials`** (the redaction hazard, §4.3).
- **`Response { data: Value, meta: ResponseMeta }`** — responses themselves may carry
  secrets (scs login → `acs_token`), so responses are redacted before hashing too
  (F-11, §4.3).
- **Adapters inject auth *after* `ExecContext`** (scs-legacy puts `acs_token` in
  query for GET / body for POST). So the sink never sees the injected secret, **and**
  cannot attest the literal wire request (F-3). The resolved endpoint path also lives
  *inside* adapter code, not in core metadata (NF-8, §4.2).
- **Vault** establishes the `~/.relais` on-host secret-store precedent.

### 2.1 Relevant signet-core API (exact signatures, verified against the source)

```rust
// receipt.rs — Action has a FIXED field set; there is no "descriptor" field (NF-2)
pub struct Action { pub tool: String, pub params: serde_json::Value, pub params_hash: String,
    pub target: String, pub transport: String, pub session: Option<String>,
    pub call_id: Option<String>, pub response_hash: Option<String>,
    pub trace_id: Option<String>, pub parent_receipt_id: Option<String> }
pub struct CompoundReceipt { pub v: u8 /*=2*/, pub id: String, pub action: Action,
    pub response: Response /* { content_hash, outcome: Option<Outcome> } */, pub signer: Signer,
    pub ts_request: String, pub ts_response: String, pub nonce: String, pub sig: String }

// sign.rs
pub fn sign_compound(key: &SigningKey, action: &Action, response_content: &serde_json::Value,
    signer_name: &str, signer_owner: &str, ts_request: &str, ts_response: &str)
    -> Result<CompoundReceipt, SignetError>;   // sets response.outcome = None (F-1)
pub fn sign(key: &SigningKey, action: &Action, signer_name: &str, signer_owner: &str)
    -> Result<Receipt, SignetError>;

// audit.rs
pub fn append(dir: &Path, receipt: &serde_json::Value) -> Result<AuditRecord, SignetError>;
pub fn append_encrypted(dir: &Path, receipt: &serde_json::Value, signing_key: &SigningKey)
    -> Result<AuditRecord, SignetError>;        // encrypts params UNDER THE SIGNING KEY — not used (NF-3)
pub fn verify_chain(dir: &Path) -> Result<ChainStatus, SignetError>;
pub fn verify_signatures(dir: &Path, filter: &AuditFilter) -> Result<VerifyResult, SignetError>;
pub fn verify_signatures_with_options(dir: &Path, filter: &AuditFilter, opts: &AuditVerifyOptions)
    -> Result<VerifyResult, SignetError>;
pub fn query(dir: &Path, filter: &AuditFilter) -> Result<Vec<AuditRecord>, SignetError>;
// AuditFilter has ONLY { since, tool, signer, limit } — no receipt-id, no end-time (NF-4)
pub struct AuditVerifyOptions {                       // EXACTLY two fields, no others (NF-5/RD-4)
    pub trusted_server_pubkeys: Vec<VerifyingKey>,    // v3 bilateral only
    pub trusted_agent_pubkeys: Vec<VerifyingKey>,     // v1/v2 signer constraint — what relais uses
}
// Per-receipt verification (for the rotation-window path, §4.11):
pub fn verify_compound(receipt: &CompoundReceipt, pubkey: &VerifyingKey) -> Result<(), SignetError>;
pub fn verify_any(receipt_json: &str, pubkey: &VerifyingKey) -> Result<(), SignetError>;

// identity::fs_ops — EXACT signatures; keys live under dir/keys/<name>.{key,pub} (NF-7)
pub fn generate_and_save(dir: &Path, name: &str, owner: Option<&str>, passphrase: Option<&str>,
    kdf_params: Option<KdfParams>) -> Result<KeyInfo, SignetError>;
pub fn load_signing_key(dir: &Path, name: &str, passphrase: Option<&str>)
    -> Result<SigningKey, SignetError>;
pub fn export_public_key(dir: &Path, name: &str) -> Result<PubKeyFile, SignetError>;
pub fn default_signet_dir() -> PathBuf;   // SIGNET_HOME or ~/.signet
```

Facts that shaped this design:

- **`sign_compound` always sets `response.outcome = None`** (`sign.rs:236-239`):
  outcome lives inside the hashed `response_content`, not a signed field (F-1, §4.2).
- **The chain stores the receipt verbatim, but the response is only a
  `content_hash`** — the response *preimage* is not in the chain, so it is
  unauditable without a sidecar (NF-6, §4.6).
- **`append` dates v2 receipts from `ts_request`** (`extract_timestamp`,
  `audit.rs:135-143`) — OPEN-1 closed (F-13); pinned by an M1 regression test.
- **`append` locks a per-*date* `<date>.jsonl.lock` and rereads the tail under it;
  `last_record_hash` scans newest-file-first without a global lock** — concurrent or
  multi-process / cross-midnight writes can fork the chain (F-6/F-7, NF-1, §4.7).
- **`append_encrypted` encrypts under the signing key and restoration for
  verification needs that key** (`audit.rs:485-511,608-615`) — unusable for a
  shareable log; v1 uses redaction (NF-3, §4.3).
- **Identity keys live under `dir/keys/<name>.{key,pub}`** and the fs_ops helpers
  take `dir`/`passphrase` and return `KeyInfo`/`PubKeyFile` (NF-7, §4.4).

## 3. Proposed design — overview

```
 server handler ─┐
                 ├─► Router::exec(ctx) ─► adapter.exec(ctx) ─► Response/Err
 cli exec    ────┘            │
                 [optional pre-exec policy gate §4.8 — before any side effect]
                             ▼ (audit feature on, sink configured)
   AuditSink.record(ctx, &result, t0, t1):
     1. envelope = { request: redact(ctx.params) + _relais_audit{auth_injection,
                       credential_ref(opaque)}, response: redact_response(...) }   §4.2,§4.3
     2. action = Action{ tool="{site}.{resource}.{action}", params=envelope.request,
                         target=manifest.base_url, transport, session, trace_id, call_id }
     3. receipt = sign_compound(key, action, envelope.response, "relais", owner, t0, t1)
     4. AuditCommand{ receipt, sidecar=envelope, ack:oneshot } ─► single writer task ─►
            persist sidecar(receipt.id) ; audit::append(dir, receipt)              §4.6,§4.7
     5. (optional) ResponseMeta.receipt = { id, record_hash }                      §4.6,§4.10
```

One埋点, two callers, one receipt per gateway action, integrity in the chain +
legibility in the sidecar.

## 4. Proposed design — detail

### 4.1 The choke point: `Router::exec`

```rust
// crates/core/src/router.rs
impl Router {
    pub async fn exec(&self, ctx: &ExecContext) -> Result<Response, AdapterError> {
        let adapter = self.get(&ctx.site).ok_or(AdapterError::SiteNotFound)?;
        // optional pre-exec policy gate (§4.8) runs HERE, before any side effect
        let t0 = Utc::now();
        let result = adapter.exec(ctx).await;
        let t1 = Utc::now();
        if let Some(sink) = &self.audit {
            self.audit_outcome(sink, ctx, &result, t0, t1).await?;  // response-open vs -closed §4.5
        }
        result
    }
}
```

- `handlers.rs:165` and `exec.rs:76` switch from `adapter.exec` to `router.exec`.
- **Convention, enforced.** `Adapter::exec` stays `pub`; the choke point is backed by
  (a) all *production* paths going through `Router::exec`, (b) a **CI guard**
  (grep/lint) forbidding direct `\.exec(` on an adapter outside adapter-internal
  tests, (c) **audit-aware test helpers** (F-10).
- **Adapters untouched.** Record after exec so the receipt binds the real outcome.

### 4.2 Data mapping (`ExecContext`/`Response` → signet)

| signet `Action` field | relais source |
|---|---|
| `tool` | `"{site}.{resource}.{action}"` (e.g. `scs.order.create`) — the routable identity |
| `params` | the **redacted request envelope** incl. `_relais_audit` (§4.3) |
| `target` | exactly **`adapter.manifest().base_url`** (one URL string) — *not* the resolved endpoint path, which lives in adapter code (NF-8). The site id is already in `tool`, so `target` carries no extra encoding (RD-5). A redacted endpoint template is a later optional adapter-provided field. |
| `transport` | `"https"` (upstream scheme) |
| `session` | server: JWT subject; CLI: local user / `null` |
| `trace_id` / `call_id` | per-request id / relais exec id |

**Outcome is in the hashed `response_content`** (F-1), and `ok` is named honestly as
**transport-level** (NF-6):

```jsonc
// response_content (the preimage we hash AND store as a sidecar, §4.6)
{ "transport_ok": true,  "data": <redact_response(response.data)>,
  "business_status": "ok" | "error" | "unclassified" }      // optional, §6 Q1
// failure
{ "transport_ok": false, "error": { "kind": "<AdapterError variant>", "message": "<text>" } }
```

> **Business-error nuance (Q1).** relais returns HTTP-200-plus-`err_code` bodies as
> `Ok(Response)` verbatim → `transport_ok:true`. The body (with `err_code`) is inside
> the hashed/sidecar'd `data`, so the business failure is *auditable*, and
> `transport_ok` no longer overstates success. Populating `business_status` waits on
> the auto-adapter-pipeline `ResponseMeta.business` work (§6 Q1).

### 4.3 Redaction boundary — the #1 hazard

The receipt+sidecar attest a **redacted semantic request and response**, explicitly.

1. **Credentials structurally excluded.** `Action` is built from `ctx` *without*
   `ctx.credentials`; adapters inject the real auth *after* the snapshot.
2. **Audit metadata rides inside `params`, since signet `Action` has no descriptor
   field** (NF-2). Under a reserved `params._relais_audit` key:
   ```jsonc
   "_relais_audit": {
     "auth_injection": "acs_token->query",   // which rule applied; non-secret
     "credential_ref": "kref_3f9c…"          // OPAQUE, non-reversible label (NF-2)
   }
   ```
   The `credential_ref` is **not** the vault site id or a hash of the token (a hash
   of a low-entropy token is brute-forceable, and raw vault/site ids leak
   tenant/env/prod-vs-staging). It is a random opaque handle minted per credential,
   resolvable only via a local, non-exported map. **Exported logs omit
   `credential_ref` by default.**
3. **Request redaction.** A configurable redactor masks sensitive keys before
   signing — default denylist `token`, `password`, `secret`, `authorization`,
   `api_key`, `acs_token`, `cookie`, `*_token`. The hash is over the redacted form.
4. **Response redaction (F-11).** `redact_response` applies the same denylist to
   `response.data` before it enters the hashed envelope, **or** the site opts the
   response out of retention. A hash over an unredacted secret-bearing body is itself
   a weak secret commitment and must not be shared.
5. **At-rest confidentiality, if required, is transparent filesystem/volume
   encryption *below* signet** (NF-3) — the bytes signet signs/appends are unchanged,
   so `verify_signatures`/`verify_chain` still work with the public key. signet's
   `append_encrypted` (encrypt-under-signing-key) is **not** used: it would force
   auditors to hold the forging key and would change the signed payload. v1 default
   is plain redaction.
6. **Belt-and-braces guard:** a CI assertion that the serialized receipt **and**
   sidecar for a representative credential-bearing `ExecContext` contain none of its
   secret bytes (request *and* response).

### 4.4 Key & audit storage layout

signet's `fs_ops` write keys under `dir/keys/<name>.{key,pub}` (NF-7); mirror that
and add the sidecar store:

```
~/.relais/signet/                       # RELAIS_SIGNET_DIR overrides; passed as `dir` to fs_ops
├── keys/
│   ├── relais.key                      # gateway signing key (generate_and_save, 0600, opt. passphrase)
│   └── relais.pub                      # verifying key — distributable
├── trusted_keys.json                   # OUT-OF-BAND trust anchor: accepted pubkeys + status/window (§4.11)
├── credential_refs.json                # opaque credential_ref → vault binding (local-only, never exported)
├── audit/
│   └── 2026-06-21.jsonl                # per-date hash-chained log (audit::append)
└── sidecars/
    └── <receipt-id>.json               # redacted request+response preimage (§4.6)
```

- Key generated once via `relais audit init` (not silently on first run — see §4.11
  on TOFU). Private key `0600`, optional passphrase.
- Signing identity: `signer_name="relais"`, `signer_owner=<configured org/host>`.

### 4.5 Failure model (config switch)

A post-exec receipt cannot un-happen a side effect, so no post-exec mode is truly
"no receipt ⇒ no action."

- **`response-open` (default).** Deliver the result; on sink failure emit a loud
  `error!` + metric.
- **`response-closed`.** If the receipt cannot be committed, the **response is
  withheld** (caller gets an error). The upstream effect may already have happened —
  this guarantees *the caller gets no result without a receipt*, not *no side effect
  without a receipt*. Bounded by the writer timeouts in §4.7 so it can't hang
  forever.
- **True pre-exec gating** ("deny before side effect") is **only** §4.8 — a signed
  intent/pending record written **before** `adapter.exec`, finalized after, with a
  recovery sweep for stuck `pending` entries.

`RELAIS_AUDIT_MODE=open|closed`, per-site override reserved.

### 4.6 Sidecar preimage store, receipt handles & verification

The chain stores only `response.content_hash`; to make receipts **legible and
re-verifiable** (NF-6):

- **Sidecar store.** The single writer persists `sidecars/<receipt-id>.json` =
  the exact redacted `{ request, response }` envelope it hashed, written **before**
  (or atomically with) the chain append. `sidecar.response` must be the *same
  `serde_json::Value`* passed to `sign_compound`, so the recompute is exact.
- **Exact recompute formula (RD-1, the crux).** signet stores the response
  commitment as `"sha256:" + hex(sha256(json_canon::to_string(&response_content)))`
  (`sign.rs:233-238`, JCS via `json_canon`). `relais audit verify` must reproduce
  **byte-for-byte, including the `sha256:` prefix**:
  ```rust
  let canon = json_canon::to_string(&sidecar.response)?;          // same JCS as signet
  let expected = format!("sha256:{}", hex::encode(Sha256::digest(canon.as_bytes())));
  assert_eq!(expected, receipt.response.content_hash);
  ```
  The request side is checked against `receipt.action.params_hash` using signet's
  *same* params-hashing rule (also `sha256:`-prefixed). A raw-digest comparison that
  omits the prefix always fails — pinned by an M2 cross-check test against a real
  `sign_compound` output. A missing/mismatched sidecar is a reported failure.
- **`ResponseMeta.receipt: Option<ReceiptHandle>`** (`{ id, record_hash }`), always
  present, serde-defaulted (§4.10) — a proof handle for the caller.
- **CLI / CI verification is trust-anchored & fail-closed** (NF-4/NF-5):
  - `relais audit verify` → `verify_chain` + `verify_signatures_with_options` with
    the gateway pubkey(s) in **`AuditVerifyOptions.trusted_agent_pubkeys`** (v1/v2
    receipts are checked against *agent* keys; `trusted_server_pubkeys` is v3-only).
    **An empty trusted set is a hard error** — no self-reported-key acceptance.
  - Rotation/time-window acceptance is evaluated **in relais** over `query()` (signet
    options are flat key vectors with no time semantics), selecting the key valid at
    each receipt's `ts_request` (§4.11).
  - `relais audit tail` / `relais audit init` / `relais audit pubkey` round out UX.
  - CI verifies an **exported sample** with a **pinned, out-of-band** trusted pubkey.

### 4.7 Single sequential writer — concrete contract (F-6/F-7, NF-1)

```rust
struct AuditCommand { receipt: serde_json::Value, sidecar: Envelope, ack: oneshot::Sender<AuditResult> }
// one writer task owns the chain:
//  - recv AuditCommand from a BOUNDED mpsc (capacity N)
//  - persist sidecar, then audit::append(dir, &receipt) on a spawn_blocking thread
//    (append takes a file lock + does blocking I/O — never on the async reactor)
//  - send AuditResult back on `ack`
```

- **The append is never aborted (RD-3).** `audit::append` takes a file lock and is
  not cancellable; aborting a timed-out append and letting the next command run would
  race two writers on one chain. So the **single writer task processes one
  `AuditCommand` at a time, start to finish, in order** — the append always runs to
  completion. Timeouts apply to the **caller's wait for the `ack`**, *not* to the
  writer's execution.
- **Enqueue:** `response-open` uses `try_send`; on full channel → drop-with-`error!`
  + metric (never blocks the request). `response-closed` uses `send_timeout`; on
  timeout → deterministic `AdapterError::AuditUnavailable`.
- **Caller ack timeout:** the request may stop *waiting* after a bounded deadline
  (response-closed → `AuditUnavailable`), but the in-flight append still completes in
  order on the writer; the receipt is not lost and no second writer starts. If the
  writer is durably wedged, a health signal trips the sink (open → log, closed →
  refuse new calls) rather than abandoning an append.
- **Crash/shutdown:** writer panics are surfaced (sender dropped → enqueue error);
  graceful shutdown **drains the channel with a bounded deadline** before exit; a
  dropped `ack` (writer gone) maps to audit failure, never a silent success.
- **Single chain owner across processes** (F-7): the server owns the chain. The CLI
  either (a) writes a **separate chain namespace** (own `signer_owner` + dir) — the
  v1 default, fork-proof — or (b) opts into a **global cross-process lock over the
  whole audit dir** (never per-date). Date rollover must not create independent lock
  domains.

### 4.8 Optional: policy gating (pre-exec)

```
if policy configured:                         // runs before adapter.exec
    eval = evaluate_policy(action_intent, signer, policy)
    Deny            → append_violation(...); return AdapterError::PolicyDenied   (no exec)
    RequireApproval → append_violation(...); return AdapterError::NeedsApproval  (no exec)
    Allow           → (optional signed pending record) exec, then sign_compound
```

The only mode that prevents a side effect. Natural fit with `Method::{Write,Delete}`
(default policy can require approval for destructive methods). v1 ships the hook + a
trivial allow-all policy.

### 4.9 Optional: per-agent attribution (server)

- **v1:** JWT subject into `Action.session`/`trace_id` (inside the signed payload).
- **Later:** signet **delegation chains** (`sign_authorized`) — needs agents to hold
  signet identities (§6 R3).

### 4.10 Required core-type changes (`crates/core/src/types.rs`)

Additive and **wire-compatible across audit / non-audit builds** (F-12) — field
always present, only its *population* is feature-gated:

```rust
pub struct ResponseMeta {
    pub pagination: Option<PaginationInfo>,
    pub rate_limit: Option<RateLimit>,
    pub cached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ReceiptHandle>,
}
pub struct ReceiptHandle { pub id: String, pub record_hash: String }
```

- **`Router` gains `audit: Option<AuditSink>`** + async `Router::exec`. `AuditSink`
  lives in `relais-core` behind the `audit` feature.
- No change to `Adapter`, `ExecContext`, `Credentials`, `AuthType`.

### 4.11 Trust anchor & key rotation (F-8, NF-4, NF-5)

signet `verify_signatures*` accept **raw key vectors with no status/time window**,
and v1/v2 receipts are only constrained when `trusted_agent_pubkeys` is non-empty
(else self-reported keys pass). So relais owns trust:

- **No trust-on-first-use.** `relais audit init` generates a key but does **not**
  auto-trust it for verification. Verification requires an **out-of-band trust
  anchor**: `trusted_keys.json` must be provisioned deliberately (copied to the
  verifier / pinned in CI). `relais audit verify` **fails closed** if no explicit
  anchor is supplied.
- **Field correctness (NF-5).** Gateway keys (v1/v2 signer) go in
  `trusted_agent_pubkeys`; `trusted_server_pubkeys` stays empty (v3-only).
- **Rotation in relais (NF-4, RD-2).** `trusted_keys.json` records each pubkey with
  an `active`/`retired` status and a validity window. signet can't enforce windows,
  **and `verify_signatures_with_options` selects records by `AuditFilter` (only
  `{since, tool, signer, limit}` — no receipt-id or end-time)**, so it can't be aimed
  at one receipt. The windowed path therefore does **not** go through that call.
  Instead `relais audit verify`:
  1. `query()` (or reads the chain) to enumerate records in order, plus
     `verify_chain` for hash-link integrity;
  2. per record, selects the trusted key whose window contains the receipt's
     `ts_request` (rejecting if none);
  3. verifies that single receipt directly with **`verify_compound` / `verify_any`**
     against the selected key;
  4. recomputes the sidecar hash (§4.6).

  `verify_signatures_with_options(trusted_agent_pubkeys = all currently-active keys)`
  remains the fast path when rotation windows aren't needed. A new key's activation is
  a **rotation event signed by the prior key** (or an offline root) appended to the
  chain.
- **Custody caveat (R6):** a host-local key proves "this gateway", not "an
  untampered host"; theft enables *future* forgery but not undetected past-rewrite.
  HSM/remote-signer out of scope.

## 5. Phasing

1. **M1 — core sink + choke point.** `audit` feature on `relais-core`; `AuditSink`
   (key load/gen via fs_ops with correct `dir`/layout, request+response redactor,
   `_relais_audit` metadata + opaque `credential_ref`, outcome envelope,
   `sign_compound`); the **writer task with the §4.7 contract**; sidecar store
   (§4.6); `Router::exec` + server/CLI rewired + CI guard (F-10); response-open/closed
   switch; redaction guard test (§4.3.6) + v2 `ts_request` dating regression test.
2. **M2 — verification, trust & UX.** `relais audit verify|tail|init|pubkey`;
   `trusted_keys.json` + **fail-closed** `verify_signatures_with_options`
   (`trusted_agent_pubkeys`) + relais-side rotation-window check; sidecar hash
   recompute in verify; `ResponseMeta.receipt`; CI verify job with a pinned
   out-of-band pubkey. Dogfood against scs adapters end-to-end.
3. **M3 — policy, attribution & rotation events (optional).** Pre-exec policy gate +
   pending/intent records (§4.8); JWT subject in receipts (§4.9 v1); signed key
   rotation events (§4.11); design note for delegation chains and per-upstream
   sub-receipts (§6 Q2).

## 6. Risks / open questions

- **Q1 — business-error classification.** Keep HTTP-200-`err_code` as
  `transport_ok:true` (v1), add `business_status` once `ResponseMeta.business`
  lands. *(open; v1 = passthrough, body auditable via sidecar)*
- **Q2 — receipt granularity.** Per-exec vs per-upstream-call (matters for
  `llm-fallback` fetch+LLM). v1 = per exec. *(open)*
- **Q3 — verify trust model.** Out-of-band trusted `trusted_agent_pubkeys`, fail
  closed on empty, relais-side rotation window. *(decided)*
- **OPEN-2 — response-open honesty.** Post-exec receipts can be lost after a side
  effect; only the pre-exec gate (§4.8) prevents it. *(open, named honestly)*
- **R2 — bilateral receipts.** Upstreams won't counter-sign. *(deferred)*
- **R3 — per-agent delegation.** Needs agent-held signet identities. *(deferred)*
- **R4 — log/sidecar growth & shipping.** Per-date jsonl + per-receipt sidecars grow
  unbounded; rotation/retention/shipping are deployment concerns. *(open)*
- **R8 — tail truncation.** Deleting the newest record(s) leaves a shorter,
  internally-valid chain. Mid-chain edits are detected; tail truncation needs an
  out-of-band checkpoint. **Mitigation:** `audit_verify` returns the chain `head`
  and accepts an `expected_head` (`relais audit verify --head <hash>`); operators
  who retain the head off-host detect truncation. *(mitigated; off-host retention
  is deployment-side)*
- **R9 — cross-process writers.** Multiple processes (server + CLI) sharing one
  audit dir are serialised by a **dir-wide exclusive lock** held across
  sign+sidecar+append, with the timestamp re-clamped to the on-disk latest under the
  lock — so the chain stays linear across processes. *(addressed)*
- **R5 — performance.** Ed25519 + JCS per call + a single serialized writer caps
  throughput; batching within the writer's total order is allowed, cross-writer
  batching is not. Measure in M1. *(open)*
- **R6 — key custody.** Host-local key ⇒ forgeable-going-forward if stolen; past
  chain still tamper-evident. *(open)*

## 7. Alternatives considered

- **signet MCP proxy in front of relais** — relais is already the choke point;
  proxying adds a hop + a process. Direct embedding is simpler.
- **Plain structured logging** — mutable, forgeable, not offline-verifiable.
- **Per-adapter receipts** — duplicated, easy to forget, misses the CLI path.
- **Sign request only (`sign`)** — loses the response commitment.
- **`append_encrypted` under the signing key** — rejected (NF-3): forces auditors to
  hold the forging key and breaks public-key-only verification. Use redaction or
  transparent below-signet encryption.
- **No sidecar (hash-only chain)** — rejected (NF-6): the response body would be
  unauditable; you could prove integrity but never read what came back.

---

## Appendix A — Codex review round 1: findings & resolutions

| # | Sev | Finding | Resolution |
|---|-----|---------|------------|
| F-1 | BLOCKER | `sign_compound` sets `outcome=None`; signed-Outcome unimplementable | §4.2 outcome in hashed `response_content`; §2.1 documents it |
| F-2 | HIGH | "fail-closed" still post-side-effect | §4.5 renamed response-open/closed; pre-exec gate §4.8 |
| F-3 | HIGH | Redacted params ≠ actual wire request | §4.3 redacted *semantic* request + injection descriptor |
| F-4 | HIGH | `append_encrypted` under signing key breaks pubkey verify | §4.3.5 redaction-only / below-signet encryption (see NF-3) |
| F-5 | HIGH | One exec ≠ every external call (llm-fallback) | §1.1/§1.2 one receipt per gateway action; sub-receipts deferred |
| F-6 | HIGH | `append` per-day lock + tail reread on hot path | §4.7 single sequential writer off the request path |
| F-7 | HIGH | Multi-process writers fork the chain | §4.7 single chain owner / global (not per-date) lock |
| F-8 | HIGH | Key rotation/trust underspecified; self-reported keys | §4.11 trusted anchor + relais-side rotation (see NF-4/5) |
| F-9 | MEDIUM | API signatures imprecise | §2.1 exact signatures (identity fixed in NF-7) |
| F-10 | MEDIUM | Choke point convention-only | §4.1 keep `pub` + CI guard + audit test helpers |
| F-11 | MEDIUM | Response redaction unspecified | §4.3.4 `redact_response` before hashing |
| F-12 | MEDIUM | `ResponseMeta.receipt` wire-compat | §4.10 serde-default, present in all builds |
| F-13 | LOW | OPEN-1 not real | §2.1 closed; v2 dated by `ts_request`, M1 regression test |

## Appendix B — Codex review round 2: verdicts & v3 closures

Round-2 verdicts on v2 (RESOLVED kept as-is in v3):

| # | R2 verdict | v3 closure |
|---|-----------|-----------|
| F-1, F-5, F-7, F-10, F-11, F-12, F-13 | RESOLVED | — |
| F-2 | RESOLVED | — |
| F-3 | PARTIAL | NF-2: `_relais_audit` under `params` (no signet descriptor field); opaque `credential_ref` |
| F-4 | PARTIAL | NF-3: drop field-level audit encryption; transparent below-signet encryption only |
| F-6 | PARTIAL | NF-1: concrete writer contract (enqueue/ack/timeout/`spawn_blocking`/drain) |
| F-8 | PARTIAL | NF-4/NF-5: out-of-band anchor, fail-closed, relais-side rotation window, `trusted_agent_pubkeys` |
| F-9 | PARTIAL | NF-7: identity signatures + `dir/keys/<name>.{key,pub}` layout corrected |

New problems round 2 introduced/surfaced, closed in v3:

| ID | Sev | Problem | v3 closure |
|----|-----|---------|-----------|
| NF-1 | HIGH | Response-closed writer could hang/lose acks | §4.7 `AuditCommand{ack:oneshot}`, bounded `send_timeout`, append timeout, `spawn_blocking`, bounded shutdown drain |
| NF-2 | HIGH | No signet descriptor field; credential refs leak | §4.3.2 `params._relais_audit` + opaque non-reversible `credential_ref`, omitted from exports |
| NF-3 | HIGH | Separate audit encryption breaks signet verify | §4.3.5 redaction-only v1; transparent filesystem encryption below signet if needed |
| NF-4 | HIGH | Trust bootstrap/rotation not specified (TOFU hole) | §4.11 out-of-band anchor required; `verify` fails closed on empty; rotation window enforced in relais over `query()` |
| NF-5 | HIGH | Wrong trusted-key field → silent v2 self-trust | §4.6/§4.11 gateway key → `trusted_agent_pubkeys`; empty set = hard error |
| NF-6 | HIGH | Outcome envelope is hash-only; `ok:true` misstates business failure | §4.6 sidecar preimage store + verify recompute; §4.2 `transport_ok` + `business_status` |
| NF-7 | HIGH | Identity API + key layout wrong | §2.1/§4.4 `generate_and_save(dir,name,owner,passphrase,kdf)->KeyInfo`; `dir/keys/<name>.{key,pub}` |
| NF-8 | MEDIUM | Router can't know resolved upstream path | §4.2 `target` = site id + `base_url` only; endpoint template a later optional adapter field |

**Still open (tracked, not closed):** Q1 (business classification), Q2 (receipt
granularity), OPEN-2 (response-open honesty), R2 (bilateral), R3 (delegation), R4
(growth/shipping), R5 (perf ceiling), R6 (key custody).

## Appendix C — Codex review round 3: verdicts & v4 closures

Round-3 verdicts on v3:

| # | R3 verdict | v4 closure |
|---|-----------|-----------|
| NF-2, NF-3, NF-5, NF-7, NF-8 | RESOLVED | — |
| NF-1 | PARTIAL | RD-3: append is **never aborted**; single writer runs each append to completion in order; only the *caller's ack wait* times out (§4.7) |
| NF-4 | PARTIAL | RD-2: windowed verify can't use `verify_signatures_with_options` (`AuditFilter` has no receipt-id/end-time); §4.11 switches to per-record `verify_compound`/`verify_any` over `query()` |
| NF-6 | PARTIAL | RD-1: §4.6 now gives the exact `"sha256:" + hex(sha256(JCS(...)))` recompute, prefix included |

New problems round 3 surfaced, closed in v4:

| ID | Sev | Problem | v4 closure |
|----|-----|---------|-----------|
| RD-1 | BLOCKER | Sidecar recompute omitted the literal `sha256:` prefix → verify always fails | §4.6 exact formula `"sha256:"+hex(sha256(json_canon::to_string))`, same rule for `params_hash`; M2 cross-check test |
| RD-2 | HIGH | Rotation-window verify not buildable via `verify_signatures_with_options` (filter can't target one receipt) | §4.11 per-record `query()` + `verify_compound`/`verify_any` with the window-selected key |
| RD-3 | HIGH | `spawn_blocking` append + timeout has no safe in-flight/abort semantics → 2 writers race | §4.7 append never aborted; single sequential writer; timeout only on caller's ack wait |
| RD-4 | LOW | `AuditVerifyOptions` trailing `..` implied non-existent fields | §2.1 spelled as the exact two-field struct, no `..` |
| RD-5 | LOW | `target` single-string encoding unspecified | §4.2 `target` = exactly `manifest().base_url`; site id stays in `tool` |

After three rounds the open items are all explicitly-deferred risks (Q1/Q2/OPEN-2/
R2–R6), not unresolved blockers. **The spec is ready to proceed to the
implementation doc.**
