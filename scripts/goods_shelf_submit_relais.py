#!/usr/bin/env python3
"""Submit an SCS goods shelf/rating-form flow through relais.

Default mode is a dry run: it resolves goods, supplier, customers, and prints
planned writes. Pass --execute to configure supplier goods and submit a rating
form to pending audit. This script never audits the rating form.
"""

from __future__ import annotations

import argparse
import re
import sys
from datetime import datetime, timedelta
from decimal import Decimal, ROUND_HALF_UP
from typing import Any

from scs_relais_common import (
    ScsRelaisError,
    bootstrap_token_if_requested,
    call,
    data_list,
    dump_json,
    find_one_by_name,
    flatten_tree,
    get_prop,
)


def split_customer_names(values: list[str]) -> list[str]:
    result: list[str] = []
    for value in values:
        for part in re.split(r"[,，、]", value):
            name = part.strip()
            if name:
                result.append(name)
    if not result:
        raise ScsRelaisError("--customer-names cannot be empty")
    return result


def find_goods(name: str) -> dict[str, Any]:
    result = call("goods", "index", {"page_num": 1, "page_size": 50, "keyword": name})
    items = data_list(result)
    goods = find_one_by_name(items, name, ("name", "goods_name"))
    if not goods and len(items) == 1:
        goods = items[0]
    if not goods:
        raise ScsRelaisError(f"goods not found: {name}")
    return goods


def find_customer(name: str) -> dict[str, Any]:
    select = call("customers", "customer_by_tree_select_list", {"keyword": name})
    match = find_one_by_name(data_list(select), name, ("name", "customer_name"))
    if match:
        return match

    tree = call("customers", "all_customer_by_tree", {"keyword": name})
    match = find_one_by_name(flatten_tree(data_list(tree)), name, ("name", "customer_name"))
    if not match:
        raise ScsRelaisError(f"customer not found: {name}")
    return match


def supplier_search_terms(name: str) -> list[str]:
    terms = [name]
    match = re.search(r"\d+\s*支", name)
    if match and match.group(0) not in terms:
        terms.append(match.group(0))
    return terms


def find_supplier(name: str) -> dict[str, Any]:
    all_nodes: list[Any] = []
    for term in supplier_search_terms(name):
        tree = call("suppliers", "supplier_by_tree_select", {"keyword": term})
        all_nodes.extend(flatten_tree(data_list(tree)))

    seen: set[tuple[str, str, str]] = set()
    items: list[Any] = []
    for node in all_nodes:
        supplier_id = get_prop(node, ["supplier_id", "id"])
        if not supplier_id:
            continue
        supplier_alias_id = get_prop(node, ["supplier_alias_id"]) or "0"
        supplier_name = get_prop(node, ["name", "supplier_name"])
        key = (supplier_id, supplier_alias_id, supplier_name)
        if key not in seen:
            seen.add(key)
            items.append(node)

    match = find_one_by_name(items, name, ("name", "supplier_name"))
    if not match:
        for term in supplier_search_terms(name):
            match = find_one_by_name(items, term, ("name", "supplier_name"))
            if match:
                break
    if not match:
        raise ScsRelaisError(f"supplier not found: {name}")
    return match


def supplier_available(goods_id: str, customer_id: str, supplier_id: str, supplier_alias_id: str) -> bool:
    available = call(
        "suppliers",
        "select_by_supplier_goods_config",
        {"goods_id": goods_id, "customer_id": customer_id},
    )
    for item in data_list(available):
        alias_id = get_prop(item, ["supplier_alias_id"]) or "0"
        if get_prop(item, ["supplier_id", "id"]) == supplier_id and alias_id == supplier_alias_id:
            return True
    return False


def find_rating_form_by_name(name: str, category_id: str, menu_id: str) -> dict[str, Any]:
    matches: list[Any] = []
    for page in range(1, 11):
        result = call(
            "rating_form",
            "getQueryRatingForm",
            {"menu_id": menu_id, "page_num": page, "page_size": 50, "category_id": category_id, "keyword": name},
        )
        page_items = data_list(result)
        matches.extend(
            item
            for item in page_items
            if get_prop(item, ["rating_form_name", "name"]) == name
        )
        if len(page_items) < 50:
            break
    if not matches:
        raise ScsRelaisError(f"created rating form not found by name: {name}")
    return sorted(matches, key=lambda item: int(get_prop(item, ["rating_form_id", "id"]) or 0), reverse=True)[0]


def cents(price: str) -> int:
    value = Decimal(price)
    return int((value * Decimal("100")).quantize(Decimal("1"), rounding=ROUND_HALF_UP))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Create and submit an SCS goods shelf rating form through relais.")
    parser.add_argument("--login-name", default="", help="admin login used only with --store-token")
    parser.add_argument("--password", default="", help="admin password used only with --store-token")
    parser.add_argument("--store-token", action="store_true", help="login first and store acs_token in relais vault")
    parser.add_argument("--execute", action="store_true", help="perform writes; omitted means dry run")
    parser.add_argument("--goods-name", required=True)
    parser.add_argument("--supplier-name", required=True)
    parser.add_argument("--price", required=True, help="price in yuan; converted to cents for SCS")
    parser.add_argument("--customer-names", required=True, nargs="+", help="one or more names, comma/Chinese-comma allowed")
    parser.add_argument("--rating-form-name", default="")
    parser.add_argument("--category-id", default="")
    parser.add_argument("--take-effect-time", default="")
    parser.add_argument("--expired-time", default="")
    parser.add_argument("--rating-form-menu-id", default="127")
    parser.add_argument("--supplier-goods-menu-id", default="127")
    return parser.parse_args()


def run(args: argparse.Namespace) -> dict[str, Any]:
    bootstrap = bootstrap_token_if_requested(args)
    planned_writes: list[dict[str, Any]] = []

    customer_names = split_customer_names(args.customer_names)
    goods = find_goods(args.goods_name)
    goods_id = get_prop(goods, ["goods_id", "id"])
    if not goods_id:
        raise ScsRelaisError("resolved goods is missing goods_id/id")

    goods_show = call("goods", "show", {"id": goods_id})
    goods_code = get_prop(goods_show, ["goods_code"]) or get_prop(goods, ["goods_code"])
    resolved_goods_name = get_prop(goods_show, ["name", "goods_name"]) or get_prop(goods, ["name", "goods_name"])
    unit_id = get_prop(goods_show, ["unit_id"]) or get_prop(goods, ["unit_id"])
    unit_name = get_prop(goods, ["unit_name", "unit"]) or get_prop(goods_show, ["unit_name", "unit"])
    goods_category_id = get_prop(goods_show, ["category_id", "goods_category_id"]) or get_prop(
        goods, ["category_id", "goods_category_id"]
    )
    goods_status = get_prop(goods, ["status", "goods_status"])
    if not unit_id:
        raise ScsRelaisError("resolved goods is missing unit_id")
    if not args.category_id:
        args.category_id = goods_category_id or "1"
    if goods_status and goods_status not in ("1", "有效"):
        raise ScsRelaisError(f"goods '{resolved_goods_name}' status is '{goods_status}', expected valid status 1")

    supplier = find_supplier(args.supplier_name)
    supplier_id = get_prop(supplier, ["supplier_id", "id"])
    supplier_alias_id = get_prop(supplier, ["supplier_alias_id"]) or "0"
    resolved_supplier_name = get_prop(supplier, ["name", "supplier_name"])

    customers: list[dict[str, str]] = []
    for name in customer_names:
        customer = find_customer(name)
        customer_id = get_prop(customer, ["customer_id", "id"])
        customer_name = get_prop(customer, ["name", "customer_name"])
        if not customer_id:
            raise ScsRelaisError(f"resolved customer '{name}' is missing customer_id/id")
        customers.append({"customer_id": customer_id, "customer_name": customer_name})

    customer_ids = [customer["customer_id"] for customer in customers]
    resolved_customer_names = [customer["customer_name"] for customer in customers]
    if not args.rating_form_name:
        args.rating_form_name = "、".join(resolved_customer_names) + " " + datetime.now().strftime("%Y%m%d")
    if not args.take_effect_time:
        args.take_effect_time = datetime.now().strftime("%Y-%m-%d") + " 00:00:00"
    if not args.expired_time:
        args.expired_time = (datetime.now() + timedelta(days=365)).strftime("%Y-%m-%d") + " 23:59:59"

    price_cents = cents(args.price)
    missing_supplier_customers = [
        customer
        for customer in customers
        if not supplier_available(goods_id, customer["customer_id"], supplier_id, supplier_alias_id)
    ]
    if missing_supplier_customers:
        change_body = {
            "menu_id": args.supplier_goods_menu_id,
            "supplier_id": supplier_id,
            "supplier_alias_id": supplier_alias_id,
            "goods_ids": [goods_id],
            "set_status": 1,
        }
        planned_writes.append({"call": "scs.supplier_goods_config.change", "params": change_body})
        if args.execute:
            call("supplier_goods_config", "change", change_body)

    still_missing: list[dict[str, str]] = []
    if args.execute:
        still_missing = [
            customer
            for customer in customers
            if not supplier_available(goods_id, customer["customer_id"], supplier_id, supplier_alias_id)
        ]
        if still_missing:
            names = ", ".join(f"{item['customer_name']}({item['customer_id']})" for item in still_missing)
            raise ScsRelaisError(
                f"supplier '{resolved_supplier_name}' still cannot supply goods '{resolved_goods_name}' "
                f"for customer scope after goods config. Configure supplier customer scope first: {names}"
            )

    rating_form_customers = [
        {"customer_id": customer["customer_id"], "customer_name": customer["customer_name"]}
        for customer in customers
    ]
    supplier_data = [
        {
            "supplier_id": supplier_id,
            "supplier_alias_id": supplier_alias_id,
            "supplier_name": resolved_supplier_name,
            "is_default": True,
            "goods_count_once": 0,
            "order_type": 0,
            "order_requirement": 0,
        }
    ]
    goods_data = [
        {
            "goods_id": goods_id,
            "goods_name": resolved_goods_name,
            "goods_code": goods_code,
            "unit_id": unit_id,
            "unit_name": unit_name,
            "ex_sale_to_customer_price": 0,
            "limit_sale_to_customer_price": 0,
            "report_sale_to_customer_price": price_cents,
            "sale_to_customer_price": price_cents,
            "cost_price": price_cents,
            "minimum_price": price_cents,
            "need_ratity_amount": 2,
            "can_allopatric_allot": 2,
            "suppliers": supplier_data,
            "memo": "",
            "is_current_price": False,
            "is_new_goods_recommend": False,
            "is_special_price": False,
            "is_purchase": 2,
            "coin": 0,
        }
    ]
    create_body = {
        "menu_id": args.rating_form_menu_id,
        "name": args.rating_form_name,
        "category_id": args.category_id,
        "take_effect_time": args.take_effect_time,
        "expired_time": args.expired_time,
        "customer_id": customers[0]["customer_id"],
        "customer_ids": customer_ids,
        "rating_form_customers": rating_form_customers,
        "goods_data": goods_data,
        "memo": "",
    }
    planned_writes.append({"call": "scs.rating_form.add_rating_form_by_cus", "params": create_body})
    submit_body = {
        "menu_id": args.rating_form_menu_id,
        "rating_form_ids": ["<created_rating_form_id>"],
        "type": 1,
    }
    planned_writes.append({"call": "scs.rating_form.update_rating_form_status", "params": submit_body})

    base_output = {
        "dry_run": not args.execute,
        "token_bootstrapped": bool(bootstrap),
        "goods_id": goods_id,
        "goods_name": resolved_goods_name,
        "goods_code": goods_code,
        "unit_id": unit_id,
        "unit_name": unit_name,
        "supplier_id": supplier_id,
        "supplier_alias_id": supplier_alias_id,
        "supplier_name": resolved_supplier_name,
        "customer_ids": customer_ids,
        "customer_names": resolved_customer_names,
        "price_cents": price_cents,
        "take_effect_time": args.take_effect_time,
        "expired_time": args.expired_time,
    }

    if not args.execute:
        return {
            **base_output,
            "missing_supplier_customers_before_planned_config": missing_supplier_customers,
            "rating_form_create_payload": create_body,
            "planned_writes": planned_writes,
            "next_step": "Run with --execute to create and submit the rating form; audit remains separate.",
        }

    call("rating_form", "add_rating_form_by_cus", create_body)
    rating_form = find_rating_form_by_name(args.rating_form_name, args.category_id, args.rating_form_menu_id)
    rating_form_id = get_prop(rating_form, ["rating_form_id", "id"])
    if not rating_form_id:
        raise ScsRelaisError("created rating form missing rating_form_id")

    detail = call(
        "rating_form",
        "getQueryRatingFormDetail",
        {
            "menu_id": args.rating_form_menu_id,
            "rating_form_id": rating_form_id,
            "category_id": args.category_id,
            "page_num": 1,
            "page_size": 50,
        },
    )
    detail_item = next((item for item in data_list(detail) if get_prop(item, ["goods_id"]) == goods_id), None)
    rating_form_detail_id = get_prop(detail_item, ["rating_form_detail_id"]) if detail_item else ""

    call(
        "rating_form",
        "update_rating_form_status",
        {"menu_id": args.rating_form_menu_id, "rating_form_ids": [rating_form_id], "type": 1},
    )
    submitted = find_rating_form_by_name(args.rating_form_name, args.category_id, args.rating_form_menu_id)

    return {
        **base_output,
        "rating_form_id": rating_form_id,
        "rating_form_detail_id": rating_form_detail_id,
        "rating_form_name": args.rating_form_name,
        "rating_form_status": get_prop(submitted, ["rating_form_status"]),
        "rating_form_status_desc": get_prop(submitted, ["rating_form_status_desc"]),
        "report_sale_to_customer_price": price_cents,
        "sale_to_customer_price": price_cents,
        "cost_price": price_cents,
        "minimum_price": price_cents,
        "next_step": "Hand off to audit agent or manual reviewer. Do not audit in this script.",
    }


def main() -> int:
    try:
        dump_json(run(parse_args()))
        return 0
    except ScsRelaisError as exc:
        print(str(exc), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
