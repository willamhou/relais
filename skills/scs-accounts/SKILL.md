---
name: scs-accounts
description: Manage SCS (娱集市后台) accounts — list, get, create, update, delete — through the relais gateway. Use when a user asks to look up, list, create, update, or delete an account in the SCS platform.
---

# SCS Accounts via Relais

This skill lets an agent operate the **SCS account service** through the
[relais](../../README.md) gateway. Relais wraps SCS's `account.v1` REST API as
the `scs` site with an `accounts` resource, reachable via the `relais` CLI or its
HTTP `/v1/exec` endpoint. One entry point, one auth, self-describing schemas.

## When to use

- The user wants to find an SCS account by id.
- The user wants to list accounts (optionally paginated / filtered by type).
- The user wants to create, update, or delete an account.

## Prerequisites

- `relais` is installed (`cargo install relais-cli`) — the `scs` adapter is built
  in, no plugin step needed.
- The SCS account service is running and reachable. Point relais at it with the
  `SCS_BASE_URL` env var (read at process start, default `http://127.0.0.1:8000`).
- Optional: if SCS later requires auth, store the `acs_token` once with
  `relais vault store scs <acs_token>`; relais injects it as `Authorization: Bearer`.

## Discover the contract first

Relais is self-describing — query it instead of guessing parameters:

```sh
relais sites                     # confirm "scs" is available
relais apis scs                  # the accounts resource + all actions
relais spec scs.accounts.create  # one action's JSON Schema (params/returns)
```

## Option A — CLI (preferred for one-off / scripted operations)

```sh
export SCS_BASE_URL=http://127.0.0.1:8000

relais exec scs.accounts.list   --data '{"page":1,"page_size":10}'
relais exec scs.accounts.get    --data '{"id":1}'
relais exec scs.accounts.create --data '{"name":"Acme","phone":"13800000000","type":2}'
relais exec scs.accounts.update --data '{"id":1,"name":"Acme-Updated","phone":"139","type":2}'
relais exec scs.accounts.delete --data '{"id":1}'
```

## Option B — HTTP (when relais runs as a server)

```sh
SCS_BASE_URL=http://127.0.0.1:8000 relais serve --port 3000 --jwt-secret my-secret

curl -X POST http://localhost:3000/v1/exec \
  -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"site":"scs","resource":"accounts","action":"list","params":{"page":1,"page_size":10}}'
```

Responses use the relais envelope: `{ "data": ..., "meta": { "pagination", "rate_limit", "cached" } }`.

## Actions

| Action | Method | Required | Optional | Notes |
|--------|--------|----------|----------|-------|
| `list`   | read   | —    | `page`, `page_size`, `type` | `meta.pagination` carries `total` / `has_next` |
| `get`    | read   | `id` | — | |
| `create` | write  | `name` | `phone`, `type` | returns the created account |
| `update` | write  | `id` | `name`, `phone`, `type` | **full replace** — omitted fields are reset |
| `delete` | delete | `id` | — | returns `{ "success": true }` |

`type` is the `AccountType` enum: `0`=unspecified, `1`=center, `2`=supplier,
`3`=distributor.

## Notes for the agent

- **IDs and totals are strings.** SCS uses protobuf JSON, so `id`, `total`, and
  timestamps come back as JSON strings (e.g. `"id":"1"`). Parse before doing math.
- **`update` is a full replace.** SCS overwrites `name`/`phone`/`type` with the
  request values; pass the complete field set or omitted fields are reset to their
  zero value (empty string / `0`).
- **Errors.** "resource not found" / 404 → the id does not exist. An invalid /
  missing required field → 400. `id <= 0` is rejected before any request.
- Always confirm the target with `relais spec scs.accounts.<action>` if unsure
  about parameters — the schema is authoritative.
