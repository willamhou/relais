# relais-adapter-scs

Relais adapter for the **SCS** service (娱集市后台) — a Go/kratos microservice
exposing the `account.v1` REST API. It surfaces SCS's Account service as a
relais `accounts` resource so AI agents can manage accounts through the unified
relais gateway (CLI or HTTP).

This is a thin HTTP client over SCS's REST API (`/v1/accounts`); it does not use
gRPC and requires no SCS SDK.

## Configuration

| What | How | Default |
|------|-----|---------|
| SCS endpoint | `SCS_BASE_URL` env var, read by the relais process at startup | `http://127.0.0.1:8000` |
| Auth (optional) | store an `acs_token` in the relais vault under site `scs`; sent as `Authorization: Bearer <token>` | none |

> `SCS_BASE_URL` is read when the relais process starts (CLI invocation or
> `relais serve`). Changing it means re-running with the new value — no rebuild.
>
> The SCS Account service has **no auth** today; the Bearer injection is forward
> compatible. When SCS enables auth, confirm the header name it expects.

## Resource & actions

One resource, `accounts`, with five actions mapped to SCS's REST endpoints:

| Action | Method | SCS endpoint | Required params | Optional params |
|--------|--------|--------------|-----------------|-----------------|
| `list`   | Read   | `GET /v1/accounts`        | —    | `page`, `page_size`, `type` |
| `get`    | Read   | `GET /v1/accounts/{id}`   | `id` | — |
| `create` | Write  | `POST /v1/accounts`       | `name` | `phone`, `type` |
| `update` | Write  | `PUT /v1/accounts/{id}`   | `id` | `name`, `phone`, `type` |
| `delete` | Delete | `DELETE /v1/accounts/{id}`| `id` | — |

`type` is the `AccountType` enum: `0`=unspecified, `1`=center, `2`=supplier,
`3`=distributor.

Run `relais apis scs` or `relais spec scs.accounts.<action>` for the live JSON
Schemas.

## Usage — CLI

```sh
export SCS_BASE_URL=http://127.0.0.1:8000

relais apis scs                                    # list resources/actions
relais spec scs.accounts.create                    # inspect an action's schema

relais exec scs.accounts.list   --data '{"page":1,"page_size":10}'
relais exec scs.accounts.create --data '{"name":"Acme","phone":"13800000000","type":2}'
relais exec scs.accounts.get    --data '{"id":1}'
relais exec scs.accounts.update --data '{"id":1,"name":"Acme-Updated","phone":"139","type":2}'
relais exec scs.accounts.delete --data '{"id":1}'
```

## Usage — HTTP (relais serve)

```sh
SCS_BASE_URL=http://127.0.0.1:8000 relais serve --port 3000 --jwt-secret my-secret

curl -X POST http://localhost:3000/v1/exec \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"site":"scs","resource":"accounts","action":"list","params":{"page":1,"page_size":10}}'
```

The response is the standard relais envelope: `{ "data": ..., "meta": { "pagination", "rate_limit", "cached" } }`.

## Behavior notes

- **int64 as string.** SCS uses protobuf JSON, which serializes `int64` fields
  (`id`, `total`, timestamps) as **strings** (e.g. `"id":"1"`). The adapter passes
  the raw JSON through, and parses `id`/`total` leniently (accepts number or string).
- **Pagination.** `list` fills `meta.pagination` (`Offset`, `max_limit` 100). `total`
  comes from the reply; if it is missing or unparseable, `total` is `None` (not a
  fake `0`). `has_next` is computed from `page * page_size < total`.
- **Update is a full replace.** SCS overwrites `name`/`phone`/`type` with the
  request values, so omitted fields are reset to their zero value — pass the full
  field set when updating.
- **Errors.** `401/403` → `Auth`, `404` → `NotFound`, `429` → `RateLimited`
  (honors `Retry-After`, default 60s), other non-2xx → `Other` (preserves the
  SCS/kratos error body). `id <= 0` is rejected locally before any request.

## Tests

```sh
# unit + wiremock HTTP-path + pub-API behavior (no external deps)
cargo test -p relais-adapter-scs

# live end-to-end against a running SCS instance (ignored by default)
SCS_BASE_URL=http://127.0.0.1:8000 \
  cargo test -p relais-adapter-scs --test scs_e2e_test -- --ignored
```

To run the live e2e locally, start SCS first (in the scs repo):

```sh
docker compose -f deploy/docker-compose.yaml up -d postgres redis
# then run the account service (host go, or a golang container with --network host)
```
