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
# Rust: spec-load + pure helpers + wiremock HTTP-path
cargo test -p relais-adapter-scs-legacy

# Python: generator golden tests — pin the swagger->spec mapping rules.
# Highest leverage: all 1324 endpoints share this transform, so these few
# cases pin the generated method/path/params for every endpoint.
cd crates/adapters/scs-legacy/generate && python3 -m unittest test_gen_spec
```

See [docs notes in the PR] for the full five-layer coverage plan (L0 structural
invariants, L1 generator golden — this file, L2 contract sweep vs live legacy,
L3 engine-shape samples, L4 core-module real CRUD).
