---
name: scs-legacy
description: Operate the full SCS (娱集市后台) platform — accounts, orders, goods, inventory, suppliers, customers, financials, and more (79 modules / 1324 endpoints) — through the relais gateway. Use whenever a user wants to read or change data in the SCS platform and there isn't a more specific scs skill.
---

# SCS (full platform) via Relais

This skill lets an agent operate the **entire legacy SCS API** through the
[relais](../../README.md) gateway. Relais wraps SCS's 1324 endpoints as the `scs`
site, with one resource per module (accounts, orders, goods, inventory,
suppliers, customers, financials, storage, reports, ...).

Because the surface is huge, **do not memorize endpoints — discover them**. Relais
is self-describing: list resources, inspect an action's schema, then execute.

## When to use

- Any SCS business operation: look up / create / update accounts, orders, goods,
  inventory, suppliers, customers, financials, etc.
- Prefer a more specific skill (e.g. `scs-accounts` for the kratos `scs-v2`
  account service) when one exists and fits; otherwise use this.

## Prerequisites

- `relais` is installed (`cargo install relais-cli`) — the `scs` adapter is built in.
- The SCS service is reachable; point relais at it with `SCS_LEGACY_BASE_URL`
  (read at process start, default `http://127.0.0.1:8501`).
- Auth: store the login token once — `relais vault store scs <acs_token>`. Relais
  injects it into every request automatically; never put `acs_token` in `--data`.

## Workflow: discover, then execute

```sh
# 1. find the right module (resource)
relais apis scs | jq '.[].id'            # 79 modules: accounts, orders, goods, ...

# 2. find the action and its exact parameters
relais spec scs.orders.create            # method, path, params JSON Schema

# 3. execute (params come straight from the spec; acs_token is injected for you)
relais exec scs.orders.create --data '{"...": "..."}'
```

HTTP form (when relais runs as a server):

```sh
curl -X POST http://localhost:3000/v1/exec -H "Authorization: Bearer $JWT" \
  -H 'Content-Type: application/json' \
  -d '{"site":"scs","resource":"orders","action":"create","params":{}}'
```

## Notes for the agent

- **`site` is always `scs`.** The resource is the module name; the action is the
  legacy action name (e.g. `accounts.create`, `orders.assign`, `goods.update`).
  Multi-segment endpoints use dotted actions (e.g. `accounts.create.jt`).
- **Always check `relais spec scs.<module>.<action>` before calling** — the schema
  is authoritative and parameters vary widely per action.
- **Business vs transport errors.** A 404 / "not found" is transport-level. SCS
  also returns business failures as `{err_code, err_msg}` with HTTP 200 — inspect
  `err_code` in the returned `data`.
- **GET vs POST is handled for you** — relais sends params as a query string for
  GET actions and as a JSON body for POST actions.
