---
name: scs-order-flow
description: Place an SCS (娱集市/天枫) order end-to-end through the relais gateway — login, find goods, add to cart, match the receiving address, fix quantity, clean up cart selection, and optionally submit the order. Use when a user asks to order goods, add goods to a cart, or run the verified order flow against the SCS legacy service.
---

# SCS Order Flow via Relais

This skill drives the **verified production order flow** of the legacy SCS
service through the [relais](../../README.md) gateway. Every HTTP step the
original `scripts/order_flow_test.ps1` performs maps to one `relais exec scs.*`
call against the `scs` (legacy) site. The agent supplies the orchestration
(matching goods, finding the cart item, building the selection list, verifying);
relais handles routing, auth injection, and transport.

Two reference implementations live alongside this skill:

- `scripts/order_flow_test.ps1` — the original PowerShell flow (source of truth
  for each call's exact field set).
- `scripts/order_flow_relais.py` — an executable relais-native port that drives
  the same flow through `relais exec` (used to verify this skill end-to-end
  against production). Run with `--submit` to place a real order, omit it for a
  dry cart run.

## When to use

- The user wants to order goods ("帮我下两瓶拉弗格10年 收货人：测试").
- The user wants to add goods to the cart without ordering ("加入购物车…请勿下单").
- The user wants to run the verified order flow / smoke-test it.

## Safety — read before running

The write steps **mutate a real account** and the final step **places a real
order**. Treat them as irreversible.

- Steps 1, 2, 4, 5 are **read-only** (login, goods query, address list, cart
  query) — safe to run for validation.
- Steps 3, 6, 7 **mutate the cart** (create/update/select item).
- Step 8 (`orders.order_for_all`) **submits a real order**.

Only run the mutating steps after the user has confirmed the concrete goods,
amount, receiver, and whether to submit. Never submit an order unless the user
explicitly asked to (`下单` / `提交订单`).

## Prerequisites

- `relais` installed from current master (the legacy `scs` site with its ~1324
  actions must be present — confirm with `relais apis scs`; if you only see an
  `accounts` resource you have the stale kratos-only build, rebuild from master).
- Point relais at the SCS endpoint with `SCS_LEGACY_BASE_URL`
  (e.g. `https://api.tffair.cn` — the adapter appends the `/1` base path).
- Auth: legacy SCS carries an `acs_token` **in the request**. relais injects it
  from the vault, so you never put `acs_token` in `--data`. Obtain a token via
  the login step below, then `relais vault store scs <acs_token>`.

```sh
export SCS_LEGACY_BASE_URL=https://api.tffair.cn
```

## Account

Do not hard-code credentials here. Take the login name / password from the
operator (env vars or a secret store) at run time, log in via step 1, and store
the returned `acs_token` in the vault. The default test account's
`customer_id` / `service_object_id` is `55`.

| Field | Value |
|-------|-------|
| login_name | `<from operator / env>` |
| password | `<from operator / env>` |
| customer_id / service_object_id | `55` |

## Parse the user request

Extract:

- **full_login** — true when the user says `完整流程`, `走完整测试`, or asks to log in first.
- **submit_order** — true when the user says `下单`, `提交订单`, or gives a direct order request.
- **goods_keyword** — the requested goods name/keyword.
- **amount** — the quantity. `两瓶` = `2`, `2瓶` = `2`.
- **receiver** — the exact receiving contact after `收货人` / `收货地址` / `地址`.

## Receiver rule

When selecting the receiving address:

1. Match `contact_info` **exactly**.
2. Do **not** match `customer_name`.
3. If no exact `contact_info` match exists, **ask the user** before any fuzzy match.

Known examples: `测试` → `receive_id = 93`; `咪咪` → `receive_id = 27`.

## Cart rule (before submission)

1. Query the cart with `receive_address_id`.
2. Find the target cart item (by `cart_id` from create, else newest item with the same `goods_id`).
3. If its quantity differs from `amount`, update it via `shoppingcarts.update`.
4. Call `shoppingcarts.select` for **all** visible cart items: target → `select_status = 1`, every other → `select_status = 2`.
5. Re-query and verify **exactly one** selected cart item (the target).

## Step → relais mapping

All calls are `relais exec scs.<resource>.<action> --data '<json>'`. Omit
`acs_token` from `--data` — relais injects it from the vault. Pass
`service_object_id` / `customer_id` = the customer id explicitly.

| # | Step | relais call | Mutates? |
|---|------|-------------|----------|
| 1 | Login | `scs.login.do` | no |
| 2 | Find goods | `scs.goods.website_goods_to` | no |
| 3 | Add to cart | `scs.shoppingcarts.create` | **cart** |
| 4 | Address list | `scs.customers.customers_receive_list` | no |
| 5 | Query cart | `scs.shoppingcarts.getShoppingCarts` | no |
| 6 | Fix quantity | `scs.shoppingcarts.update` | **cart** |
| 7 | Select target only | `scs.shoppingcarts.select` | **cart** |
| 8 | Submit order | `scs.orders.order_for_all` | **order** |

> **Path note (step 2).** The PowerShell script calls
> `/1/edi_api/goods/website_goods_to`, but the swagger-generated spec maps this
> action to `/1/goods/website_goods_to` (no `edi_api` prefix). relais uses the
> spec path — **verified equivalent against production** (`api.tffair.cn`): the
> `edi_api` prefix is a gateway alias and is not required. relais's
> `scs.goods.website_goods_to` returns the same goods list.
>
> **Type note (all steps).** Legacy binds the JSON body to a Go struct, so field
> types must match the spec exactly. In particular `customer_id` and
> `receive_address_id` are **strings** — sending them as numbers yields
> `err_code 201 请求json格式错误`. When unsure, run `relais spec scs.<res>.<action>`
> and pass each field with its declared type.

### 1. Login (read-only) — get token + customer_id

```sh
relais exec scs.login.do --data "{\"login_name\":\"$SCS_LOGIN\",\"password\":\"$SCS_PASSWORD\"}"
# -> response.acs_token, response.customer_id
relais vault store scs <acs_token>     # so later calls are authenticated
```

### 2. Find goods (read-only)

```sh
relais exec scs.goods.website_goods_to \
  --data '{"service_object_id":"55","page_num":1,"page_size":20,"keyword":"拉弗格10年"}'
```
Pick from `data_list`: exact `name` match first, else `name` contains the keyword.
Keep `goods_id, supplier_id, unit_id, rating_form_detail_id, flat_price_id,
agent_id, agent_price_id, agent_cust_price_id, supplier_alias_id`.

### 3. Add to cart (mutates) — confirm first

```sh
relais exec scs.shoppingcarts.create --data '{
  "service_object_id":"55","goods_id":<goods_id>,"supplier_id":<...>,"unit_id":<...>,
  "rating_form_detail_id":<...>,"flat_price_id":<...>,"agent_id":<...>,
  "agent_price_id":<...>,"agent_cust_price_id":<...>,"supplier_alias_id":<...>,
  "order_amount":2,"is_recycle_bottle":0}'
# -> response.id = new cart_id
```

### 4. Receiving address (read-only)

```sh
relais exec scs.customers.customers_receive_list \
  --data '{"customer_id":"55","page_num":1,"page_size":50,"receive_type":0}'
```
Apply the **receiver rule**: exact `contact_info` match → `receive_id`.

### 5. Query cart (read-only)

```sh
relais exec scs.shoppingcarts.getShoppingCarts \
  --data '{"customer_id":"55","is_recycle_bottle":0,"receive_address_id":"<receive_id>"}'
```
Find the target cart item per the **cart rule**.

### 6. Fix quantity if needed (mutates)

```sh
relais exec scs.shoppingcarts.update --data '{
  "service_object_id":"55","id":<cart_id>,"amount":2,"supplier_id":<...>,
  "goods_id":<...>,"goods_count_once":<goods_count_to_shopping_cart>,"order_memo":""}'
```

### 7. Select only the target (mutates)

```sh
relais exec scs.shoppingcarts.select --data '{
  "service_object_id":"55",
  "select":[{"id":<target_cart_id>,"select_status":1},{"id":<other>,"select_status":2}]}'
```
Re-query (step 5) and verify exactly one item has `select_status == 1`.

### 8. Submit order (places a real order) — explicit confirmation required

```sh
relais exec scs.orders.order_for_all --data '{
  "order_no":"<yyMMddHHmmss + 8 random digits>",
  "address_item":[{"agent_id":"","customer_id":"55","receive_info_id":<receive_id>}],
  "shopping_cart_ids":[<target_cart_id>],
  "total_amt":"<cart.total_amt>",
  "shopping_carts":[{"id":<target_cart_id>,"activity_customer_type":0,"activity_ids":[<...>]}],
  "service_object_id":"55","recycle_bottle_voucher":[]}'
```

## Notes for the agent

- **Business errors pass through.** Legacy returns HTTP 200 with
  `{err_code, err_msg, ...}`; a non-empty `err_code` means business failure.
  Stop the flow and surface `err_msg` — do not proceed to the next step.
- **acs_token is never a `--data` field.** It is a vault credential injected by
  relais; the spec strips it from every action's params.
- **IDs.** Pass `service_object_id`/`customer_id` as the customer id on every
  authenticated call. The login response also carries `customer_id`.
- **Token-only mode.** If the user provides a token instead of logging in, store
  it with `relais vault store scs <token>` and skip step 1; you still need the
  customer id (`55` for the default account).
- The reference flow and the verified field set are in
  `scripts/order_flow_test.ps1` — consult it when a call's parameters are unclear.
