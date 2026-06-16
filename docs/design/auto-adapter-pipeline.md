# Design: Automating adapter creation (spec-driven adapter pipeline)

- **Status:** Draft v3 (revised after Codex review rounds 1 & 2)
- **Author:** willamhou (with Claude)
- **Date:** 2026-06-16
- **Tracking:** —

## 1. Problem

Today an adapter is added one of two ways:

1. **Hand-written per endpoint** (`relais-adapter-github`, `relais-adapter-hackernews`):
   one Rust function per resource/action. Linear cost in endpoints; doesn't scale
   to dozens of APIs.
2. **Generated, data-driven** (`relais-adapter-scs-legacy`): an offline script
   (`generate/gen_spec.py`) distills a Swagger file into `scs_legacy_spec.json`,
   embedded via `include_str!`; a constant-size engine builds the resource tree
   and routes `exec` purely from that spec. **1324 endpoints, zero per-endpoint
   code.**

Approach (2) already proves adapter creation can be automated — but it is welded
into one crate, hard-wired to one Swagger dialect, one auth scheme (`acs_token`),
and one base URL baked at build time. As we add more APIs we should *not* keep
writing bespoke crates.

**Goal:** make "support a new API" mean *drop in a spec + a small config*, not
*write a crate*. Cover the spectrum from machine-readable specs (fully automated)
down to doc-only and spec-less sites (LLM-assisted, then LLM fallback).

### 1.1 Goals

- A single **generic, data-driven engine** that serves *any* number of sites from
  spec files — no new crate per API.
- A documented, versioned **relais spec format** (generalized from
  `scs_legacy_spec.json`) that **provably subsumes scs-legacy** (the dogfood bar).
- **Spec generators** that emit that format from common contracts (OpenAPI first).
- An **LLM-assisted** path to produce a spec from human docs when no machine spec
  exists, with a human review gate.
- A reusable **verification harness** generalized from the scs-legacy sweep tests
  (L1/L2/L3/L4) so generated adapters are *proven*, not assumed.
- A **trust + network-egress model** that makes runtime-loaded specs safe by default.

### 1.2 Non-goals (v1)

- GraphQL / gRPC / SOAP generators (design the spec format to allow them; build
  OpenAPI only first).
- Auto-discovering APIs with no spec and no docs — that stays the existing
  **LLM Fallback** at runtime.
- Replacing the hand-written `github`/`hackernews` adapters (they stay; this is
  additive). They also become the **conformance benchmark**: the generic engine
  must reproduce their path-param routing, `returns`, and pagination, or those
  adapters are correctly *not* in scope for generation (see §4).
- Auth flows that need request signing (AWS SigV4/HMAC), token refresh (OAuth
  authorization-code), or cookie capture. v1 covers **static credentials**
  (API key / bearer) injected from the vault, and **fails closed** on anything
  else (§3.5.3).

> **This work is not pure-additive at the type layer.** Several mechanisms below
> require new fields on `crates/core/src/types.rs` (provenance, business outcome,
> action aliases). These are enumerated in **§3.8** and scheduled in **M1** — they
> are not optional.

## 2. Current state (what we reuse and must not regress)

- `relais_core::Adapter` trait: `manifest() -> SiteManifest`, `resources() -> Vec<Resource>`,
  `exec(&ExecContext) -> Result<Response, AdapterError>`.
- Core types (`crates/core/src/types.rs`): `SiteManifest { id, name, base_url, auth_type }`,
  `AuthType { OAuth, APIKey, Cookie, None }`, `Resource { id, description, actions, children }`,
  `Action { id, method, description, params, returns, pagination }`,
  `Method { Read, Write, Delete }`, `ResponseMeta { pagination, rate_limit, cached }`.
  `AdapterError` (`crates/core/src/error.rs`): non-2xx → `Other` preserves the body text.
- `scs-legacy` engine behaviors that **must round-trip identically** (pinned by
  `scs_legacy_http_test.rs`):
  - URL = `base_url + base_path + action.path`.
  - **Method-dependent credential injection**: `acs_token` into the **query
    string for GET**, into the **JSON body for POST** (and other non-GET).
  - **Response decoding**: empty body → `{}`; non-JSON body → `{ "raw": <text> }`;
    otherwise parsed JSON. *(Two orthogonal rules — see §3.2 `decode`.)*
  - **Business errors pass through verbatim**: HTTP 200 carrying
    `{ err_code, err_msg, data, ... }` is *not* an adapter error; callers inspect
    `err_code`. HTTP-level status still maps (401/403→Auth, 404→NotFound, …) and
    **non-2xx bodies are preserved** in `AdapterError::Other`.
- `scs_legacy_spec.json` **does not carry per-parameter location** — `gen_spec.py`
  flattens body/query/path/formData into one JSON-Schema-ish `params` object. This
  matters for migration (§3.8 / NEW-3).
- The scs-legacy **verification ladder**: L1 generator golden, L2 route sweep
  (live; `err_msg == "请求的服务不存在"` = route miss), L-B business reachability,
  L-C write-path reachability.

## 3. Proposed design

### 3.1 The pipeline (decision tree)

```
New API
  ├─ has OpenAPI/Swagger?  ──► generator: openapi → relais-spec ──┐
  ├─ has docs only?        ──► LLM drafts relais-spec ─► human review ──┤
  └─ nothing but a website ──► existing LLM Fallback (runtime, no spec)  │
                                                                         ▼
                                          relais-spec (.json) + trust tier
                                                                         │
                                                          generic spec-driven engine
                                                                         │
                                                       verification harness (L1–L4)
                                                                         │
                                                            registered site (active|pending|disabled)
```

### 3.2 The relais spec format (`relais-spec` v1)

One file fully describes a site: identity, transport, auth injection, decode/error
policy, and the resource/action tree. Published as a JSON Schema; `spec_version`
gates future changes.

```jsonc
{
  "spec_version": "1",
  "site": {
    "id": "scs",
    "name": "SCS (legacy)",
    "base_url": "http://127.0.0.1:8501",
    "base_url_env": "SCS_LEGACY_BASE_URL",   // override; STILL host-policy checked (§3.5.1)
    "base_path": "/1"
  },

  // --- AUTH: how the vault token is rendered into the request -------------
  "auth": {
    "auth_type": "APIKey",                   // load-time validated; fail-closed (§3.5.3)
    "vault_site": "scs",
    // Ordered injection rules; first whose `when` matches applies. Omit `when`
    // for an unconditional default. (BLOCKER-1)
    "inject": [
      { "when": { "http": "GET" }, "into": "query", "name": "acs_token", "template": "{token}" },
      { "into": "body",                          "name": "acs_token", "template": "{token}" }
    ]
  },

  // --- DECODE: two ORTHOGONAL rules, not one enum (HIGH-4 / NEW-5) --------
  "decode": {
    "parse": "json",                         // json | none
    "on_empty": "empty_object",              // empty_object | null | error
    "on_non_json": "raw_wrap",               // raw_wrap (=> {"raw": <text>}) | text | error
    "preserve_error_body": true              // non-2xx body text kept in AdapterError::Other
  },

  // --- BUSINESS classification (surfaced to ResponseMeta.business, §3.8) --
  "business": {
    "passthrough": true,                     // return 2xx bodies verbatim (scs default)
    "error_when": { "pointer": "/err_code", "not_in": ["", "0", 0, null] },
    "route_miss_when": { "pointer": "/err_msg", "equals": "请求的服务不存在" }
  },

  "modules": {
    "shoppingcarts": {
      "description": "购物车",
      "actions": {
        "getShoppingCarts": {
          "method": "Read",                  // semantic core Method (safety/UX)
          "http": "POST",                    // wire verb
          "path": "/shoppingcarts/getShoppingCarts",
          "request": { "content_type": "application/json" },
          // Per-action auth override: a full replacement `inject` list (same schema
          // as auth.inject). When present it REPLACES the site list for this action;
          // rules are matched top-to-bottom; absent => inherit site list. (finding #1)
          "auth_inject": null,
          // PER-PARAM BINDING (HIGH-6). `in` ∈ path|query|header|body|form.
          "params": {
            "customer_id":        { "in": "body", "type": "string", "coerce": "to_string" },
            "receive_address_id": { "in": "body", "type": "string", "coerce": "to_string" },
            "is_recycle_bottle":  { "in": "body", "type": "integer" }
          },
          "returns": { "schema_ref": null, "fidelity": "passthrough" },  // §3.5.4
          "pagination": null                  // or extraction rules, §3.5.5
        }
      }
    }
  }
}
```

Each generalization is tied to a finding; resolutions are tracked in Appendices A/B.

### 3.3 The generic engine: `relais-adapter-spec`

A data-driven `Adapter` impl not bound to one site. Loads specs from two sources:

- **Embedded / first-party** (`include_str!`/`include_dir!`) — *trusted* tier;
  preserves scs's offline in-binary test story.
- **Runtime spec dir** (`$RELAIS_SPEC_DIR`, default `~/.config/relais/specs/*.json`)
  — *untrusted* by default (§3.5.1).

`manifest`/`resources` derive from the spec as scs-legacy does, and now record the
**spec source/trust** (provenance) — which requires new core fields (§3.8). `exec`
builds requests from `auth.inject` + per-param `in` + `decode`/`business`,
reproducing the scs round-trip identically (M2 dogfood, §5).

#### 3.3.1 Loading precedence & collisions (HIGH-5)

- **Duplicate `site.id` is rejected by default**, naming both sources. No silent shadowing.
- A trusted embedded site is overridable by a runtime spec **only** via explicit
  `--allow-override <site.id>`, surfaced in `relais sites` with provenance.

#### 3.3.2 Registration states (NEW-4)

Loading a spec never silently half-works. Each registered site has a state:

- **active** — fully usable.
- **pending** — loaded and visible in `relais sites`, but `exec` is **refused**
  with an actionable message, because a required approval is missing (untrusted
  spec with an unapproved `vault_site` binding, §3.5.1, or an unconfirmed host
  allowlist). The user resolves it via `relais spec trust …`.
- **disabled** — loaded but `exec` refused permanently, because the spec declares
  an **unsupported auth scheme** (§3.5.3). Distinct from `pending`: no user action
  short of editing the spec / writing a Rust hook will enable it.

This removes the v2 contradiction where fail-closed auth and "load then trust"
collided with no intermediate state.

### 3.4 Spec generators

- **`openapi → relais-spec`** (v1): generalize `gen_spec.py`.
  - **Naming** (MEDIUM-10): prefer OpenAPI `operationId`/`tags`; fall back to the
    scs path-segment heuristic only when absent; explicit, golden-tested collision
    policy; `--naming operationId|path`.
  - **Param binding** (HIGH-6): emit per-param `in` from OpenAPI `in:` and request
    media types; set `request.content_type`.
  - **Auth** (HIGH-7): infer `auth.inject` from `securitySchemes`; emit nothing and
    **flag for human decision** on unsupported schemes (oauth2 flows, mTLS).
  - **Returns** (HIGH-8): carry response schemas through; mark `fidelity:"schema"`
    vs `"passthrough"`.
  - Pure `generate_spec(openapi_dict) -> spec_dict` (golden-testable); OpenAPI 3.x + Swagger 2.0.

#### 3.4.1 Generated-id stability (MEDIUM-11 / NEW-6)

- The generator keeps an **id lockfile** (`<site>.ids.lock.json`) mapping a stable
  **upstream key** → relais `resource.action` id. Upstream-key precedence:
  `operationId` → `x-relais-id` (author-pinned) → `method+path`.
- **Rename detection** works only when the *key is stable*. If `operationId`/`path`
  themselves change upstream, the change is **indistinguishable from add+remove**;
  the generator emits it as a *new* id and a **`--alias old=new`** prompt, and the
  author confirms the alias. This limit is documented, not hidden.
- Aliases live in the lockfile **and** on the action: a new `Action.aliases:
  Vec<String>` (§3.8) so `relais exec` resolves a deprecated id (with a warning)
  without re-reading the lockfile at runtime.

### 3.5 Trust, network egress, auth, decode, fidelity

#### 3.5.1 Trust & network-egress model (BLOCKER-2, NEW-1, NEW-2)

A spec can otherwise point the engine at any host with a vault token — an SSRF /
credential-exfiltration boundary. The controls are split into two layers:

**(a) Network egress — applies to EVERY spec regardless of trust tier**, and to
**both** `base_url` and any `base_url_env` override (NEW-1):

- **Connect-time IP validation (NEW-2).** Resolve the host, and validate the
  **resolved IP(s)** at connect time — not just the hostname. **Always block**
  loopback/link-local/private/ULA/metadata ranges (127/8, 10/8, 172.16/12,
  192.168/16, 169.254/16 incl. 169.254.169.254, ::1, fc00::/7, fe80::/10) unless
  the spec carries an explicit, trusted `allow_private` opt-in.
- **Anti-rebinding.** Pin the validated IP for the life of the connection
  (resolve-then-connect-to-IP), so a second DNS answer can't swap in a blocked
  address after the check.
- **Redirects.** Disabled by default; if enabled per-spec, **re-run the full host
  + IP check on every redirect hop**.

**(b) Credential trust — governs vault binding**, gated by tier:

- Runtime-dir specs are **untrusted** by default and load **without vault access**.
  Binding a `vault_site` (including cross-site claims like `vault_site: github`)
  requires explicit per-binding approval: `relais spec trust <site> --vault <vault_site>`
  → moves the site from `pending` to `active`.
- Embedded/first-party specs are trusted (shipped in the reviewed binary) and may
  be signed; unsigned runtime specs are always untrusted.

> Trust tier governs **only** vault binding and whether a host allowlist must be
> author-declared — it never disables layer (a). Private-range and connect-time IP
> checks run for trusted specs too.

#### 3.5.2 Engine response handling

Decode per §3.2 `decode` (the two orthogonal `on_empty`/`on_non_json` rules
reproduce scs's `{}` / `{"raw":…}`); map HTTP status to `AdapterError` by engine
default, **preserving non-2xx body text in `Other`** (NEW-5). For 2xx, pass the
body through (default) and attach the `business` classification to
`ResponseMeta.business` (§3.8) without forcing an error.

#### 3.5.3 Auth fail-closed (HIGH-7)

At **load time**, validate every `auth.inject`/`auth_type` is in the supported set
(static apiKey/bearer in header/query/body). Anything needing signing, refresh, or
cookie capture → the site registers as **disabled** (§3.3.2) with a clear message,
never silently 401-ing at runtime. Such sites need a small Rust hook (a per-site
`Adapter` delegating routing to the engine but supplying the auth step).

#### 3.5.4 Response-schema fidelity bar (HIGH-8)

`returns.fidelity` ∈ { `schema`, `passthrough` }. Generators preserve source
response schemas where present (`schema`); `passthrough` (scs's `{ "type":
"object" }`) is the explicit low-fidelity tier. **Enforceable bar:** the harness
writes a per-site `fidelity_baseline` (fraction of read actions at `schema` where
the source provides a response schema); **CI fails if a first-party spec regresses
below its recorded baseline.** New first-party OpenAPI specs must start at the
source-achievable maximum. `passthrough` remains acceptable only for action-style
APIs (scs) whose contract publishes no response schema.

#### 3.5.5 Pagination & metadata extraction (MEDIUM-12)

Today `Action.pagination` is *advertised* but `ResponseMeta.pagination` is often
`None` (e.g. GitHub cursor actions). v1 closes the loop with an **extraction
grammar** per action:

```jsonc
"pagination": {
  "style": "Cursor",                         // matches core PaginationStyle
  "next": { "from": "header", "name": "Link", "rel": "next" },   // or {from:"body", pointer:"/next_cursor"}
  "items": { "pointer": "/data" }
}
```
and a site-level `rate_limit` header map (`X-RateLimit-Remaining` / `…-Reset`) →
`ResponseMeta.rate_limit`. The engine fills `ResponseMeta` from these rather than
leaving them `None`. (Deep/edge extraction stays under R3.)

### 3.6 Verification harness (generalized sweep)

Tier names made precise so the offline gate isn't mistaken for live conformance:

- **L1 — generator golden:** `generate_spec` output pinned per fixture.
- **L2 — engine conformance (offline, always-on CI):** every action routes to the
  expected (verb, URL), injects auth per `auth.inject`, binds params per `in`,
  decodes per `decode`, and fills `ResponseMeta` per §3.5.5 — asserted with
  **wiremock**. Proves the engine obeys the spec; **not** an upstream-match claim.
- **L3 — live route conformance (gated/scheduled):** every (verb, path) hits a real
  instance, 0 mismatches; catches upstream drift. Scheduled for first-party specs.
- **L4 — business reachability (gated):** read endpoints execute; write-path reaches
  the business layer with zero side effects.

### 3.7 Adding a new API — the end state

```sh
relais gen openapi ./stripe-openapi.json > ~/.config/relais/specs/stripe.json
# site loads as `pending` (untrusted runtime spec, vault unbound):
relais spec trust stripe --vault stripe        # → active
relais verify stripe                            # L2 offline (+ L3/L4 if configured)
relais vault store stripe sk_live_...
relais exec stripe.charges.create --data '{...}'
```

### 3.8 Required core-type changes (`crates/core/src/types.rs`) — NOT optional

Several v2/v3 mechanisms depend on fields the core types don't have yet (round-2
NEW-3/6/7, HIGH-5). M1 must land these:

- **Provenance/trust on `SiteManifest`** (and surfaced per `Resource`): `source`
  (`embedded` | `runtime{path}`) and `trust` (`trusted` | `untrusted`) — HIGH-5.
- **Business outcome on `ResponseMeta`**: `business: Option<BusinessOutcome>`
  ( `{ code, message, classified: ok|error|route_miss }` ) — NEW-7.
- **Aliases on `Action`**: `aliases: Vec<String>` + router resolution of a
  deprecated id to its current action (with a deprecation warning) — NEW-6.
- **Registration state** plumbed through the manifest so `relais sites` shows
  `active|pending|disabled` and `exec` enforces it — NEW-4.
- Confirm `AdapterError::Other` body preservation is contractual (it is today) and
  test-pinned for the generic engine — NEW-5.

> **Migration note (corrects v2 M2).** `scs_legacy_spec.json` has no per-param
> `in`, so scs **cannot** migrate by a byte-for-byte data move. M2 must **re-run
> the generalized `gen_spec.py` against `scs_old/swagger/swagger.json`** to recover
> per-param locations (NEW-3). Acceptance is *behavioral*: the full scs sweep
> (L1–L4) passes identically; the spec file is regenerated, not hand-edited.

## 4. What stays manual / human-in-the-loop

- **Unsupported auth** (signing/refresh/cookie): a small per-site Rust hook (§3.5.3).
- **Spec-invisible quirks** (scs string-typed ids, HTTP 200 + `err_code`): L3/L4
  surface them; encode into `params.coerce` / `business`.
- **Naming/ergonomics polish** + **alias confirmation on key changes** (§3.4.1).
- **LLM-drafted specs** pass a human review gate.
- **Hand-tuned adapters** (github/hackernews-class) stay first-class (§6 R5).

## 5. Phasing

1. **M1 — core types + format + engine:** land §3.8 core-type changes; define
   `relais-spec` v1 + JSON Schema; build `relais-adapter-spec` (parser,
   `manifest`/`resources`/`exec`, dual load, registration states, `auth.inject` +
   per-action override, per-param `in`, split `decode`, `business`, the §3.5.1
   network-egress layer). Port scs HTTP/order-flow offline tests onto it.
2. **M2 — dogfood scs-legacy:** **regenerate** `scs` spec from
   `scs_old/swagger/swagger.json` via the generalized generator (recovering
   per-param `in`), retire the bespoke engine, prove the **full scs sweep passes
   identically**. Acceptance test for format completeness.
3. **M3 — OpenAPI generator:** operationId naming, param binding, securityScheme→
   auth, response fidelity, id lockfile; `relais gen openapi`; add one real
   public-OpenAPI site.
4. **M4 — verification + trust CLI:** `relais verify` (L2 always-on; L3/L4 gated),
   `relais spec trust`, network-egress enforcement + fidelity baseline gate.
5. **M5 — LLM-assisted authoring:** offline doc→spec drafting + review workflow.

## 6. Risks / open questions

- **R1 — auth coverage.** v1 fails closed beyond static tokens (§3.5.3). Is the
  Rust signing/refresh hook surface small enough to beat a full manual adapter? *(open)*
- **R3 — schema/pagination fidelity depth.** §3.5.4/§3.5.5 set a floor; deep
  `$ref`/`allOf`/polymorphism and exotic pagination remain unbounded. *(open)*
- **R4 — versioning.** `spec_version` migration tooling as the format evolves. *(open)*
- **R5 — when is a manual crate still better?** Kept first-class (§4). *(decided: keep)*
- **R7 — trust UX.** Does per-binding approval (§3.5.1b) push users to blanket-trust?
  Needs sane defaults + messaging. *(open)*

## 7. Alternatives considered

- **Code generator emitting Rust per endpoint** — reintroduces per-API compile
  units; data-driven engine proved constant-size suffices.
- **Runtime-only (no embedded specs)** — loses scs's offline in-binary guarantees.
- **Pure LLM fallback for everything** — too low-fidelity/expensive as the primary
  path for APIs that have specs.

---

## Appendix A — Codex review round 1: findings & resolutions

| # | Sev | Finding | Resolution |
|---|-----|---------|------------|
| 1 | BLOCKER | Method-dependent `acs_token` injection not representable | §3.2 `auth.inject` ordered rule list + §3.2 per-action `auth_inject` override |
| 2 | BLOCKER | Runtime specs can exfiltrate vault tokens / SSRF | §3.5.1 split into network-egress (all specs) + credential-trust tiers |
| 3 | HIGH | `err_code` business semantics underspecified | §3.2 `business` value-based `error_when`/`route_miss_when` |
| 4 | HIGH | Non-JSON / empty response handling missing | §3.2 `decode` (orthogonal `on_empty`/`on_non_json`) |
| 5 | HIGH | Duplicate `site.id` / precedence undefined | §3.3.1 reject-by-default + §3.8 provenance fields |
| 6 | HIGH | OpenAPI param locations flattened | §3.2 per-param `in` + `request.content_type`; §3.4 generator |
| 7 | HIGH | Auth must fail closed | §3.5.3 + §3.3.2 `disabled` state |
| 8 | HIGH | Response-schema fidelity has no bar | §3.5.4 enforceable baseline gate |
| 9 | MEDIUM | String-param coercion not enforced | §3.2 `coerce: to_string`; pinned in M2 |
| 10 | MEDIUM | Naming heuristic SCS-specific | §3.4 operationId/tags + fallback + `--naming` |
| 11 | MEDIUM | Generated id stability unmitigated | §3.4.1 lockfile + `Action.aliases` |
| 12 | MEDIUM | Pagination/meta extraction undesigned | §3.5.5 extraction grammar |
| 13 | LOW | Offline L2 weakens live guarantee | §3.6 tiers renamed (L2 offline / L3 live) |

## Appendix B — Codex review round 2: verdicts & v3 closures

Round-2 verdicts on the v2 draft (RESOLVED = kept as-is in v3):

| # | Sev | R2 verdict | v3 closure |
|---|-----|-----------|-----------|
| 1 | BLOCKER | PARTIAL | §3.2 now *specifies* per-action `auth_inject` (full-replacement list, top-to-bottom match) |
| 2 | BLOCKER | PARTIAL | §3.5.1 rewritten: connect-time IP checks, anti-rebinding, redirect re-check, `base_url_env` covered (see NEW-1/2) |
| 3 | HIGH | RESOLVED | — |
| 4 | HIGH | PARTIAL | §3.2 `decode` split into orthogonal `on_empty` + `on_non_json` (unambiguous scs reproduction) |
| 5 | HIGH | PARTIAL | §3.8 adds `SiteManifest`/`Resource` provenance fields |
| 6 | HIGH | RESOLVED | — |
| 7 | HIGH | RESOLVED | — |
| 8 | HIGH | PARTIAL | §3.5.4 now an enforceable CI baseline gate, not "should" |
| 9 | MEDIUM | RESOLVED | — |
| 10 | MEDIUM | RESOLVED | — |
| 11 | MEDIUM | PARTIAL | §3.4.1 defines key precedence, rename-detection limits, alias storage |
| 12 | MEDIUM | PARTIAL | §3.5.5 extraction grammar + rate-limit header map |
| 13 | LOW | RESOLVED | — |

New problems round 2 introduced by v2, closed in v3:

| ID | Sev | Problem | v3 closure |
|----|-----|---------|-----------|
| NEW-1 | BLOCKER | `base_url_env` could redirect a trusted token to a new host | §3.5.1(a): egress layer applies to `base_url_env` and all tiers |
| NEW-2 | BLOCKER | Host allowlist weak vs DNS-rebinding/redirects | §3.5.1(a): connect-time IP validation, IP pinning, per-hop redirect re-check |
| NEW-3 | HIGH | Per-param `in` unrecoverable from current scs spec | §3.8 migration note: M2 regenerates from `swagger.json`, not data move |
| NEW-4 | HIGH | Fail-closed auth vs runtime-trust flow conflict | §3.3.2 `pending`/`disabled` registration states |
| NEW-5 | MEDIUM | Error-body preservation not pinned | §3.2 `decode.preserve_error_body` + §3.5.2; §3.8 test-pin |
| NEW-6 | MEDIUM | Id lockfile/alias incomplete; no core alias surface | §3.4.1 + §3.8 `Action.aliases` |
| NEW-7 | MEDIUM | Business class claimed on `ResponseMeta` w/o a field | §3.8 `ResponseMeta.business` |

**Still open (tracked, not closed):** R1 (signing/refresh hook surface), R3
(deep schema + exotic pagination fidelity), R4 (`spec_version` migration tooling),
R7 (trust-approval UX).
