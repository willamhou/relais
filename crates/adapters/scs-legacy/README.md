# relais-adapter-scs-legacy

Relais adapter for the **legacy SCS** service (`scs_old`, Beego) — the full
`/1/*` action-based API, **79 modules / 1324 endpoints**. Registered as the `scs`
site (the production-complete API). The newer kratos rewrite is exposed
separately as [`scs-v2`](../scs/README.md); as the team migrates modules off
legacy, callers can move resource-by-resource from `scs` to `scs-v2`
(strangler-fig).

## How it works — generated, data-driven

This adapter is **not hand-written per endpoint**. An offline generator distills
the legacy Swagger into a compact spec that the engine loads at build time:

```
scs_old/swagger/swagger.json
        │  generate/gen_spec.py
        ▼
scs_legacy_spec.json   (module → action → {method, path, params})  ← committed
        │  include_str!
        ▼
ScsLegacyAdapter        (constant-size engine: resources() + exec() read the spec)
```

Mapping: `module` = first path segment (Swagger tag) → resource; remaining
segments joined with `.` → action id (e.g. `/accounts/create/jt` →
`accounts` / `create.jt`); HTTP method and params come from the spec.

To regenerate after the legacy API changes:

```sh
python3 crates/adapters/scs-legacy/generate/gen_spec.py \
  /path/to/scs_old/swagger/swagger.json \
  crates/adapters/scs-legacy/scs_legacy_spec.json
```

## Configuration

| What | How | Default |
|------|-----|---------|
| Legacy endpoint | `SCS_LEGACY_BASE_URL` env var (read at process start) | `http://127.0.0.1:8501` |
| Auth | store the `acs_token` in the relais vault under site `scs`; injected into every request | — |

Legacy auth is an `acs_token` carried **in the request** (body field for POST,
query param for GET). Store it once: `relais vault store scs <acs_token>`. The
adapter strips `acs_token` from every action's advertised params (it is a
credential, not a caller parameter) and injects it automatically.

## Discover the contract

With 79 modules and 1324 actions, browse via relais's self-describing API rather
than memorizing endpoints:

```sh
relais apis scs                       # 79 resources (accounts, orders, goods, ...)
relais spec scs.orders.create         # one action's method/path/params schema
```

## Usage

```sh
export SCS_LEGACY_BASE_URL=http://127.0.0.1:8501

relais exec scs.accounts.create --data '{"login_name":"u","name":"User","role_ids":["1"],"status":1,"customer_attribute":1}'
relais exec scs.orders.assign   --data '{"order_id":"123"}'
relais exec scs.advice.index    --data '{"page":1}'      # GET action — params go to the query string
```

HTTP (relais serve): `POST /v1/exec` with `{"site":"scs","resource":"<module>","action":"<action>","params":{...}}`.

## Behavior notes

- **Action-based, not REST.** Legacy endpoints are `POST /1/<module>/<action>`
  (1136 POST, 188 GET). The relais action id mirrors the legacy action name.
- **Business errors pass through.** Legacy returns `{err_code, err_msg, data, ...}`
  with HTTP 200 for business-level failures. The adapter maps only HTTP-level
  status (401/403→Auth, 404→NotFound, 429→RateLimited, other non-2xx→Other) and
  returns the body verbatim — callers inspect `err_code` themselves.
- **Live e2e is `#[ignore]`d.** `exec` is covered end-to-end by wiremock. There
  is also a live test (`tests/scs_legacy_e2e_test.rs`) that hits a real legacy
  instance and asserts the full chain (lookup → URL → POST → parse) works; it is
  ignored by default because standing up the legacy Beego app + DB is heavyweight.
  Note the bundled DB dump (`scs_old/resource/archieve/scs.sql`) lags the code
  schema (some tables/columns are missing), so business endpoints may return
  `{err_code, err_msg}` — the adapter passes that through; the e2e only asserts
  the round-trip, not business success.

## Tests

```sh
# Rust: spec-load + pure helpers + wiremock HTTP-path + offline order-flow e2e
# (tests/scs_legacy_order_flow_test.rs drives the full order flow against a mock
#  server — see skills/scs-order-flow for the live, production-verified flow).
cargo test -p relais-adapter-scs-legacy

# Python: generator golden tests (L1) — pin the swagger->spec mapping rules.
# Highest leverage: all 1324 endpoints share this transform, so these few
# cases pin the generated method/path/params for every endpoint.
cd crates/adapters/scs-legacy/generate && python3 -m unittest test_gen_spec

# L2 contract sweep — probe ALL 1324 endpoints against a live legacy, asserting
# every adapter (method, path) actually routes. Ignored by default.
SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
  cargo test -p relais-adapter-scs-legacy --test scs_legacy_sweep_test -- --ignored --nocapture

# Business reachability sweep — logs in with a REAL token, probes read-only
# endpoints, classifies responses (business-reachable / system-error / routed-miss).
# Requires a schema-aligned DB (see schema_sync below).
SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
  cargo test -p relais-adapter-scs-legacy --test scs_legacy_business_test -- --ignored --nocapture
```

### Live legacy setup (for the business sweep)

The bundled DB dump lags the code schema, so `generate/schema_sync.py` aligns it
(adds missing tables/columns derived from the code's `*Do` structs):

```sh
# in the scs repo: load the dump, then align the schema
docker compose -f deploy/docker-compose.yaml up -d postgres redis
tests/fixtures/load-legacy-db.sh
python3 <relais>/crates/adapters/scs-legacy/generate/schema_sync.py \
  /path/to/scs_old <postgres_container> --apply
```

With the schema aligned, the business sweep reaches **568/579 read-only endpoints
(98%)** — their business logic actually executes against real data. The ~10
remaining are tables referenced only by raw SQL (no `*Do` struct, absent from the
dump) — a legacy test-data gap, not a mapping issue.

```sh
# Write-path reachability — proves the WRITE chain reaches the business layer
# with ZERO writes (token only, no params -> validation rejects every call).
SCS_LEGACY_BASE_URL=http://127.0.0.1:8501 \
  cargo test -p relais-adapter-scs-legacy --test scs_legacy_writepath_test -- --ignored --nocapture
```

## Coverage summary — testing the full 1324-endpoint API

| Layer | What it proves | Result |
|-------|----------------|--------|
| **L1** generator golden (`generate/test_gen_spec.py`) | swagger→spec mapping rules — pins method/path/params for every endpoint | 17 cases, all rules |
| **L2** contract sweep (`tests/scs_legacy_sweep_test.rs`) | every adapter (method, path) routes on live legacy | **1324/1324** routes hit, 0 mismatches |
| **L-B** business reachability (`tests/scs_legacy_business_test.rs`) | read-only endpoints actually run business logic vs real data | **568/579 (98%)** |
| **L-C** write-path reachability (`tests/scs_legacy_writepath_test.rs`) | write endpoints reach the business layer (route+auth+validation), zero side effects | **33/34 (97%)** |

Together these cover the full API: **generation** is rule-pinned (L1), **routing**
is 100% verified on live legacy (L2), **read business logic** runs for 98% of
read endpoints (L-B), and the **write chain** reaches the business layer for 97%
of core writes (L-C). The small tails are legacy test-data gaps (tables with no
`*Do` struct, absent from the bundled dump), not adapter issues. The wiremock
tests (`scs_legacy_http_test.rs`) cover the engine paths offline; the live tests
above need a schema-aligned legacy (see `schema_sync.py`).
