# Remediation plan: security hardening (whole-repo Codex review)

- **Status:** Draft v2 (revised after Codex plan-review)
- **Author:** willamhou (with Claude)
- **Date:** 2026-06-22
- **Source:** whole-repo Codex review of `master` @ `123f8b4` — 5 HIGH, 7 MEDIUM, 3 LOW.

This proposes concrete fixes for every finding, grouped into independently
shippable workstreams (PR-0…PR-H), each with *problem → fix → compat/migration →
acceptance*. No code changes yet; this plan was itself Codex-reviewed (round 1) and
this v2 folds in those corrections.

> **Compatibility theme.** Several fixes intentionally **break insecure defaults**
> (default JWT secret, dev vault password, bind-all). These are security-positive
> breaking changes; each ships with a clear error message + migration note + a
> **per-area** opt-out (no single master "dev mode" — Codex Q4), and they land before
> relais has external users relying on the insecure default.

> **Per-area escape hatches (replaces a single `RELAIS_DEV`, Codex HIGH/Q4).** Each
> weakened default has its own explicit, loudly-warned opt-out so one leaked env var
> can't re-enable everything: `RELAIS_ALLOW_WEAK_JWT_SECRET`,
> `RELAIS_ALLOW_DEV_VAULT_PASSWORD`, `RELAIS_AUDIT_ALLOW_UNENCRYPTED_KEY`.

> **Shared crates introduced first:** PR-0 extracts a **pure, non-feature-gated
> `core::redact`** (only the generic value/key masking — `mask_secrets` +
> `secret_values_of`, NOT `AuditMeta`/envelope metadata, Codex H5). `core::http`
> (client builder) lands with PR-E and `core::net_guard` (egress) with PR-C; both are
> reused thereafter.

---

## PR-A — Auth hardening (H1 + L1)

### H1 — Default JWT secret + bind-all (`cli/src/lib.rs:40`, `cli/src/commands/serve.rs:23`)
- **Problem:** `--jwt-secret` defaults to `dev-secret`; `serve` binds `0.0.0.0:{port}`.
  A default launch is a forgeable-token gateway on all interfaces.
- **Fix:**
  - Remove the `default_value = "dev-secret"`. Make the secret come from
    `--jwt-secret` / `RELAIS_JWT_SECRET` with **no default**; if absent → hard error
    with guidance.
  - **Reject weak secrets:** refuse `dev-secret`, refuse `len < 32`. (A startup
    check in `serve::run`, returning `anyhow::Error`.)
  - **Default bind `127.0.0.1`.** Add `--host` (default `127.0.0.1`) +
    `RELAIS_HOST`; binding `0.0.0.0` becomes an explicit opt-in. Log the bind addr.
  - Per-area dev escape hatch: `RELAIS_ALLOW_WEAK_JWT_SECRET=1` permits the old weak
    default for local dev only, with a loud `warn!` (not a global `RELAIS_DEV`).
- **Compat:** scripts relying on the implicit secret/bind break → documented in
  README + a precise error ("set RELAIS_JWT_SECRET (≥32 chars); bind 0.0.0.0 via --host").
- **Acceptance:** `serve` with no secret exits non-zero with guidance; with
  `dev-secret`/short secret exits non-zero (unless `RELAIS_DEV=1`); default bind is
  loopback; a test asserts the weak-secret rejection.

### L1 — JWT algorithm pinning (`server/src/auth.rs:37`)
- **Problem:** `Validation::default()` — alg policy implicit (it is HS256 today, but
  unstated).
- **Fix:** `Validation::new(Algorithm::HS256)`; keep `exp` validation; set
  `leeway` explicitly. Document that HS256 is the only accepted alg.
- **Acceptance:** a test signs with HS256 → accepted; a token with `alg: none` or
  RS256 → rejected.

---

## PR-B — Vault key derivation + nonce (H2 + L3)

### H2 — Raw-SHA-256 vault key + dev-password fallback (`core/src/vault.rs:28`, `cli/src/commands/mod.rs:90`)
- **Problem:** AES-256-GCM key = `Sha256(password)` — no salt, no KDF hardness; CLI
  falls back to `relais-dev-password`.
- **Fix:**
  - Derive the AEAD key with **Argon2id** via `argon2::Argon2::hash_password_into`
    (raw 32-byte key derivation — NOT the PHC `PasswordHasher` verifier API, Codex
    MEDIUM), with a **per-vault random salt** + **concrete pinned params** (e.g.
    `m=19456 KiB, t=2, p=1`, OWASP baseline). Add **`argon2`** and
    **`chacha20poly1305`** as **direct** deps of `relais-core` (today they are only
    transitive via optional `signet-core`).
  - **Per-record versioning, NOT a store-wide switch (Codex BLOCKER).** Today a record
    is `nonce(12) || ciphertext` with no version. v1 prefixes a **version byte** to
    every record: `0x01 || salt_ref || nonce(24) || ciphertext` (XChaCha20-Poly1305,
    L3). Reads dispatch on the leading byte: a record with **no/invalid v1 marker is
    treated as legacy v0** (`Sha256` key, 12-byte AES-GCM nonce) and decrypted with the
    old scheme. This makes mixed v0/v1 stores safe — untouched v0 records never become
    undecryptable.
  - **Migration is explicit, atomic, backed up (Codex BLOCKER/HIGH).**
    `relais vault migrate` rekeys all records v0→v1 in one pass: copy `vault.db` to
    `vault.db.bak`, write the new `kdf.json` **atomically** (temp + fsync + rename),
    re-encrypt every record, fsync. Lazy upgrade-on-write is also safe now (per-record
    version), but the explicit command is the documented path and avoids partial state.
  - `kdf.json` (`{ version, alg:"argon2id", salt, m, t, p }`) is written atomically;
    its absence ⇒ a v0-only store (all records read with the legacy key).
  - **Drop the dev-password fallback:** require `RELAIS_VAULT_PASSWORD`; allow the old
    `relais-dev-password` only under `RELAIS_ALLOW_DEV_VAULT_PASSWORD=1` with a `warn!`.
- **Compat:** v0 records remain readable indefinitely via the version-byte dispatch;
  `vault migrate` (or first write) upgrades them; a crash mid-migrate leaves
  `vault.db.bak` intact.
- **Acceptance:** new records carry the v1 version byte + XChaCha nonce; `kdf.json`
  has a unique salt + persisted params; a v0 fixture still decrypts (read path) and
  `vault migrate` upgrades it losslessly with a `.bak`; missing password (no opt-in)
  errors clearly; interrupted migrate is recoverable.

### L3 — AEAD nonce uniqueness (`core/src/vault.rs:35`) — **folded into H2's v1 format**
- **Decision (Codex Q1):** switch the vault AEAD to **XChaCha20-Poly1305** (192-bit
  random nonce — collision risk negligible for random generation). Ship in the **same
  v1 format bump** as H2 (one migration, not two). `chacha20poly1305` becomes a direct
  dep.
- **Acceptance:** store/retrieve round-trips under XChaCha; nonce is 24 random bytes;
  collision probability is negligibly low (not claimed "by construction").

---

## PR-C — Network egress / SSRF guard (H3)

### H3 — Arbitrary-URL fetch + cookie exfil in llm-fallback (`llm-fallback/src/lib.rs:117`, `browser.rs:26,35`)
- **Problem:** caller-supplied `params.url` → `client.get(url)`; imported cookies
  attached as a `Cookie` header regardless of host. SSRF + cookie theft if the
  fallback adapter is registered.
- **Fix:** a reusable **egress guard** in `relais-core` (generalize the design from
  `signet-audit-integration.md §3.5.1`):
  - Parse the URL; resolve the host; **validate the resolved IP(s)** before connecting;
    **always block** loopback / private / link-local / ULA / metadata
    (127/8, 10/8, 172.16/12, 192.168/16, 169.254/16 incl. 169.254.169.254, ::1,
    fc00::/7, fe80::/10).
  - **Pin** the validated IP for the connection so a second DNS answer can't swap in a
    blocked address (Codex MEDIUM — must really pin at connect time): resolve in
    relais, validate, then force reqwest to that IP via
    `ClientBuilder::resolve_to_addrs(host, &[validated_ip])` (or a custom connector),
    **and disable auto-redirects** (`redirect::Policy::none()`) so each hop is fetched
    explicitly and re-validated by relais. Manual per-hop revalidation, not reqwest's
    own redirect follow.
  - **Host allowlist** for the fallback (`RELAIS_FALLBACK_ALLOW` / config); empty =
    fallback refuses arbitrary hosts (fail-closed).
  - **Cookie scoping:** `fetch_html` currently receives only the cookie *values* (no
    domain) — its signature must change to take the stored cookie **domain** (present
    on `CredentialData::Cookie.domain`, `types.rs:125`). Attach stored cookies **only**
    when the request host matches that domain; never cross-host or after a redirect to
    a different host.
- **Compat:** llm-fallback is not registered by default today, so user-facing impact
  is minimal; behavior is fail-closed.
- **Acceptance:** requests to private/metadata IPs are refused (incl. via a hostname
  that resolves to them); a redirect to a private IP is refused; cookies are not sent
  to a non-matching host; an allowlisted public host works.

> Reuse note: the same guard SHOULD wrap all outbound adapter requests later, but
> v1 scopes enforcement to llm-fallback (the only arbitrary-URL path).

---

## PR-D — Server error hygiene (H5)

### H5 — Raw upstream error bodies returned to agents (`server/src/handlers.rs:167`)
- **Problem:** every `AdapterError` → HTTP 500 + `err.to_string()`; adapters preserve
  upstream bodies, which may echo credentials/PII, bypassing the audit redaction
  boundary on the caller path.
- **Fix:**
  - **Status mapping:** `Auth → 401`, `RateLimited → 429` (+ `Retry-After`),
    `NotFound → 404`, `Unsupported → 400`, `SiteNotFound → 404`,
    `AuditUnavailable → 503`, `Upstream/Other → 502/500`.
  - **Redact error text** before returning: mask the request's credential values
    (the same secret set the audit redactor uses). This requires a **non-feature-gated
    redaction helper** — extract the value-masking (`mask_secrets` + `secret_values_of`)
    from `core::audit::redact` into a small always-compiled `core::redact` module that
    `audit` re-exports, so the server can redact without the `audit` feature.
  - Return a structured body `{ "error": { "kind", "message" } }`.
- **Scope note:** success responses are unchanged (the agent's own data — see the
  audit non-goal). This fix is specifically the **error** path.
- **Acceptance:** a forced `Auth` error → 401 with a generic message; an error whose
  text contains a credential value → value masked in the response; status mapping
  unit tests.

---

## PR-E — Outbound HTTP client hardening (M1 + M2 + M3)

### M2 — Missing timeouts (`adapters/*`, `llm-fallback/providers/*`, `token_refresh`, **`cli/commands/auth.rs:147`**)
- **Fix:** a shared `relais-core::http::client(profile)` builder returning a
  `reqwest::Client` with connect timeout (~5s), a sane redirect policy, and pool
  limits. **Per-profile total/read timeout (Codex MEDIUM):** a short default
  (~30s) for adapters/OAuth, but a **longer profile (~120s+, configurable)** for
  LLM-provider completions (`openai`/`anthropic`/`ollama`), which are legitimately
  slow. Replace every ad-hoc `Client::new()` — including the CLI OAuth token exchange
  at `cli/src/commands/auth.rs:147` (Codex completeness).
- **Acceptance:** all outbound clients route through the builder; a stalled upstream
  fails by timeout (slow-mock test); LLM profile allows a long but bounded call.

### M3 — Large responses read fully before truncation (`llm-fallback/browser.rs:48`)
- **Fix:** cap by `Content-Length` when present; otherwise **enable reqwest's `stream`
  feature** (workspace `reqwest` currently enables only `json` — Codex MEDIUM) and read
  via `bytes_stream()` with a running byte cap (`MAX_HTML_LEN`), stopping early; decode
  the capped bytes with `from_utf8_lossy`.
- **Acceptance:** a multi-MB response is capped without full allocation (large-mock
  test); `reqwest` `stream` feature added in the workspace.

### M1 — UTF-8 byte-slice panic (`llm-fallback/extractor.rs:16`)
- **Fix:** truncate on a char boundary — `html.char_indices().take_while(|(i,_)| *i <
  MAX).last()` or `from_utf8_lossy` of the capped bytes from M3. Never index a `str`
  by raw byte length.
- **Acceptance:** a string with a multibyte char straddling the cap truncates without
  panic (unit test).

---

## PR-F — Legacy SCS credential transport (H4)

### H4 — `acs_token` in GET query + plaintext HTTP default (`scs-legacy/src/lib.rs:226,124,25`)
- **Constraint:** the legacy SCS API *requires* `acs_token` in the query for GET (it
  is the upstream contract); relais cannot unilaterally move it to a header without
  the upstream rejecting it. So the fix is about **transport + logging**, not moving
  the token.
- **Fix:**
  - **Require HTTPS** for any non-loopback `base_url` (reject `http://` to a remote
    host unless `RELAIS_SCS_ALLOW_INSECURE=1`, loud `warn!`).
  - **Scrub token from logs:** ensure no `tracing`/error path logs the full URL with
    the query; redact `acs_token` in any URL that is logged or surfaced.
  - Document the query-auth as an upstream-mandated property and the HTTPS requirement.
- **Acceptance:** a remote `http://` base_url is refused without the opt-in; no log
  line contains a raw `acs_token`; HTTPS base_url works unchanged.

---

## PR-G — Audit polish (M4 + M6 + L2)

### M4 — Audit transport hardcoded `"https"` (`core/src/audit/envelope.rs:87`)
- **Fix:** derive `Action.transport` from the parsed `base_url` scheme
  (`https`/`http`). Optionally flag insecure transport in the receipt.
- **Acceptance:** an `http://` site yields `transport:"http"` in the receipt; a unit
  test pins it.

### M6 — Audit signing key unencrypted at rest (`core/src/audit/key.rs:42`)
- **Fix:** add a passphrase field to `AuditConfig` and the CLI; read it from
  `RELAIS_AUDIT_PASSPHRASE` (or OS keyring) and pass it to `generate_and_save` /
  `load_signing_key` (both already accept `Option<&str>`, `key.rs:46`); if none
  provided, keep the unencrypted fallback only under
  `RELAIS_AUDIT_ALLOW_UNENCRYPTED_KEY=1` with a `warn!`.
- **Acceptance:** with a passphrase set, `keys/relais.key` is encrypted (signet kdf
  field present) and reloads with the passphrase; without it (non-dev) → clear error
  or explicit opt-in.

### L2 — Credential ref per-site, not per-version (`core/src/audit/mod.rs:127`)
- **Fix (concrete source, Codex MEDIUM):** bind `CredBinding` to
  `{ site, cred_fingerprint }` where `cred_fingerprint = first 16 hex of
  HMAC-SHA256(local_audit_salt, credential_bytes)` — a **non-reversible, salted**
  id that changes on rotation but never reveals the secret. The `local_audit_salt`
  lives in the signet dir (local-only, never exported), so the fingerprint is not
  brute-forceable across hosts.
- **Acceptance:** rotating a site's credential yields a different `credential_ref`;
  the binding still leaks no secret.

---

## PR-H — CLI secret intake + fan-out cap (M5 + M7)

### M5 — Secrets as CLI args (`cli/src/lib.rs:96,128,145`)
- **Fix:** add non-arg intake for vault tokens / OAuth client secrets / cookies:
  read from stdin (`-`), a prompt (no echo), `--token-file`, or an env var. Keep the
  positional arg as deprecated with a `warn!` about shell-history exposure.
- **Acceptance:** `relais vault store <site> --stdin` reads the token from stdin and
  does not appear in `ps`/history; existing positional usage still works with a warning.

### M7 — Unbounded HN comment fan-out (`adapters/hackernews/src/lib.rs:189`)
- **Fix:** enforce a `limit` (default cap, e.g. 50) on `kids`; fetch with **bounded
  concurrency** (`futures::stream::iter(...).buffer_unordered(N)`) and per-request
  timeouts (PR-E client).
- **Acceptance:** a story with thousands of kids fetches at most `limit` comments with
  bounded concurrency; a test asserts the cap.

---

## Phasing & priority (revised per Codex plan-review)

Each is an independent PR + Codex code-review + local regression, per the project's
standard loop. Order reflects severity, dependencies, and Codex's reordering:

0. **PR-0** (prerequisite) — extract a pure, non-feature-gated `core::redact`
   (`mask_secrets` + `secret_values_of` only; `audit` re-exports it). Unblocks PR-D & PR-F.
1. **PR-A** (auth: H1, L1) — highest impact, smallest surface; per-area weak-secret
   opt-out.
2. **PR-D** (server error hygiene: H5) — status mapping + error-body redaction via PR-0.
3. **PR-F** (scs-legacy transport: H4) — **moved earlier** (HIGH); reuses PR-0 to scrub
   surfaced URLs/errors.
4. **PR-B** (vault: H2 + L3) — **only after the migration design here is built as
   revised** (per-record version byte + explicit atomic `vault migrate`); one v1 format
   bump (Argon2id + XChaCha20-Poly1305).
5. **PR-C** (SSRF guard: H3) — `core::net_guard`, connect-time IP pinning + cookie scope.
6. **PR-E** (HTTP client hardening: M1–M3) — `core::http` builder (per-profile timeouts)
   + streaming cap + UTF-8-safe truncation.
7. **PR-G** (audit polish: M4, M6, L2).
8. **PR-H** (CLI secrets: M5; HN fan-out cap: M7 — its timeout part depends on PR-E's
   `core::http`).

Cross-cutting utilities: `core::redact` (PR-0), `core::http` (PR-E), `core::net_guard`
(PR-C), reused thereafter.

## Open questions — resolved (Codex round 1)
- **Q1 (vault AEAD):** **XChaCha20-Poly1305** (add direct dep; no `hkdf` in tree). ✅
- **Q2 (vault migration):** **explicit `relais vault migrate` + per-record version
  byte** — NOT a store-wide `kdf.json` switch (would lock out untouched v0 records).
  Atomic write + `.bak` backup. ✅ (was the plan's BLOCKER)
- **Q3 (success-response redaction):** **keep success bodies verbatim** by default
  (the agent's own data); offer opt-in success redaction only if operators ask. ✅
- **Q4 (escape hatches):** **per-area flags**, not one `RELAIS_DEV`:
  `RELAIS_ALLOW_WEAK_JWT_SECRET`, `RELAIS_ALLOW_DEV_VAULT_PASSWORD`,
  `RELAIS_AUDIT_ALLOW_UNENCRYPTED_KEY`. ✅
