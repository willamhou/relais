#!/usr/bin/env python3
"""Drive the SCS order flow through the relais CLI (legacy `scs` site).

A relais-native port of scripts/order_flow_test.ps1: every HTTP step goes through
`relais exec scs.<resource>.<action>`. The `acs_token` is injected by relais from
the vault, so it is never placed in `--data`. See skills/scs-order-flow/SKILL.md
for the step-by-step contract.

Prereqs:
  - relais built from current master (the legacy `scs` site must be present).
  - A token stored in the vault for site `scs`:
        relais exec scs.login.do --data '{"login_name":"...","password":"..."}'
        relais vault store scs <acs_token>
  - SCS endpoint via SCS_LEGACY_BASE_URL (the adapter appends the /1 base path).

Usage:
  SCS_LEGACY_BASE_URL=https://api.tffair.cn \
  python3 scripts/order_flow_relais.py \
    --customer-id 55 --goods "拉弗格10年" --amount 2 --receiver 测试 [--submit]

Without --submit it stops after preparing + verifying the cart (no order placed).
With --submit it calls orders.order_for_all and PLACES A REAL ORDER.
"""
import argparse
import json
import os
import random
import subprocess
import time

BIN = os.environ.get("RELAIS_BIN", "relais")


def call(resource, action, params):
    """One relais exec; returns the business body, aborting on any err_code."""
    p = subprocess.run(
        [BIN, "exec", f"scs.{resource}.{action}", "--data",
         json.dumps(params, ensure_ascii=False)],
        capture_output=True, text=True,
    )
    if p.returncode != 0:
        raise SystemExit(f"relais exec scs.{resource}.{action} failed: {p.stderr.strip()}")
    body = json.loads(p.stdout).get("data", {})
    ec = body.get("err_code")
    if ec not in (None, "", 0, "0"):
        raise SystemExit(
            f"scs.{resource}.{action} business error: err_code={ec} err_msg={body.get('err_msg')}"
        )
    return body


def run(customer_id, keyword, amount, receiver, submit):
    # 2. find goods — exact name match first, else keyword contains.
    goods_res = call("goods", "website_goods_to", {
        "service_object_id": customer_id, "page_num": 1, "page_size": 20, "keyword": keyword,
    })
    dl = goods_res.get("data_list") or []
    goods = next((g for g in dl if g.get("name") == keyword), None) \
        or next((g for g in dl if keyword in (g.get("name") or "")), None)
    if not goods:
        raise SystemExit(f"Goods not found: {keyword}")
    print(f"[2] goods: {goods.get('name')} goods_id={goods.get('goods_id')}")

    # 3. add to cart (MUTATES)
    create = call("shoppingcarts", "create", {
        "service_object_id": customer_id,
        "goods_id": goods.get("goods_id"),
        "supplier_id": goods.get("supplier_id"),
        "unit_id": goods.get("unit_id"),
        "rating_form_detail_id": goods.get("rating_form_detail_id"),
        "flat_price_id": goods.get("flat_price_id"),
        "agent_id": goods.get("agent_id"),
        "agent_price_id": goods.get("agent_price_id"),
        "agent_cust_price_id": goods.get("agent_cust_price_id"),
        "supplier_alias_id": goods.get("supplier_alias_id"),
        "order_amount": amount,
        "is_recycle_bottle": 0,
    })
    new_cart_id = create.get("id")
    print(f"[3] cart created: id={new_cart_id}")

    # 4. receiving address — exact contact_info match (never customer_name).
    addr_res = call("customers", "customers_receive_list", {
        "customer_id": customer_id, "page_num": 1, "page_size": 50, "receive_type": 0,
    })
    address = next((a for a in (addr_res.get("data_list") or [])
                    if a.get("contact_info") == receiver), None)
    if not address:
        raise SystemExit(f"Receiver not found by exact contact_info: {receiver}")
    receive_id = address.get("receive_id") or address.get("id")
    print(f"[4] receiver: {receiver} -> receive_id={receive_id} ({address.get('customer_name')})")

    def query_cart():
        # NB: customer_id / receive_address_id are strings per the spec.
        return call("shoppingcarts", "getShoppingCarts", {
            "customer_id": customer_id, "is_recycle_bottle": 0,
            "receive_address_id": str(receive_id),
        })

    # 5. query cart, find target item (by new cart_id, else newest same goods).
    items = query_cart().get("cart_items") or []
    target = next((i for i in items if i.get("cart_id") == new_cart_id), None)
    if not target:
        same = sorted((i for i in items if i.get("goods_id") == goods.get("goods_id")),
                      key=lambda i: i.get("cart_id"), reverse=True)
        target = same[0] if same else None
    if not target:
        raise SystemExit("Target cart item not found after add.")
    target_cart_id = target.get("cart_id")
    print(f"[5] target cart item: cart_id={target_cart_id} amount={target.get('amount')}")

    # 6. fix quantity if needed (MUTATES)
    if int(target.get("amount")) != amount:
        call("shoppingcarts", "update", {
            "service_object_id": customer_id, "id": str(target_cart_id), "amount": amount,
            "supplier_id": target.get("supplier_id"), "goods_id": target.get("goods_id"),
            "goods_count_once": target.get("goods_count_to_shopping_cart"), "order_memo": "",
        })
        items = query_cart().get("cart_items") or []
        target = next((i for i in items if i.get("cart_id") == target_cart_id), None)
        if not target or int(target.get("amount")) != amount:
            raise SystemExit(f"amount after update = {target and target.get('amount')}, expected {amount}")
        print(f"[6] quantity updated -> {amount}")
    else:
        print("[6] quantity already correct, no update")

    # 7. select only the target (MUTATES); verify exactly one selected.
    call("shoppingcarts", "select", {
        "service_object_id": customer_id,
        "select": [{"id": i.get("cart_id"),
                    "select_status": 1 if i.get("cart_id") == target_cart_id else 2}
                   for i in items],
    })
    cart = query_cart()
    selected = [i for i in (cart.get("cart_items") or []) if i.get("select_status") == 1]
    if len(selected) != 1 or selected[0].get("cart_id") != target_cart_id:
        raise SystemExit(f"Unexpected selected items: {[i.get('cart_id') for i in selected]}")
    sel = selected[0]
    print(f"[7] selected exactly one: cart_id={sel.get('cart_id')} total_amt={cart.get('total_amt')}")

    if not submit:
        print("\n--submit not set -> cart prepared and verified, no order placed.")
        return

    # 8. submit order (PLACES A REAL ORDER)
    activity_ids = [a.get("activity_id") for a in (sel.get("activity_info") or [])]
    order_no = time.strftime("%y%m%d%H%M%S") + str(random.randint(10000000, 99999999))
    print(f"\n[8] submitting order_no={order_no} total_amt={cart.get('total_amt')} ...")
    order = call("orders", "order_for_all", {
        "order_no": order_no,
        "address_item": [{"agent_id": "", "customer_id": customer_id, "receive_info_id": receive_id}],
        "shopping_cart_ids": [sel.get("cart_id")],
        "total_amt": str(cart.get("total_amt")),
        "shopping_carts": [{"id": sel.get("cart_id"), "activity_customer_type": 0, "activity_ids": activity_ids}],
        "service_object_id": customer_id,
        "recycle_bottle_voucher": [],
    })
    print("[8] ORDER SUBMITTED:")
    print(json.dumps(order, ensure_ascii=False, indent=2))


def main():
    ap = argparse.ArgumentParser(description="Drive the SCS order flow via relais.")
    ap.add_argument("--customer-id", required=True)
    ap.add_argument("--goods", required=True, help="goods keyword")
    ap.add_argument("--amount", type=int, required=True)
    ap.add_argument("--receiver", required=True, help="exact contact_info of the receiving address")
    ap.add_argument("--submit", action="store_true", help="place a REAL order (omit for a dry cart run)")
    a = ap.parse_args()
    run(a.customer_id, a.goods, a.amount, a.receiver, a.submit)


if __name__ == "__main__":
    main()
