#!/usr/bin/env python3
"""Create an SCS legacy customer onboarding bundle through relais.

Default mode is a dry run: it resolves existing data and prints planned writes.
Pass --execute to create missing area/customer/address/accounts.
"""

from __future__ import annotations

import argparse
import sys
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


def find_customer_node(keyword: str, leaf: int | None = None) -> dict[str, Any] | None:
    result = call("customers", "customer_by_tree_select_list", {"keyword": keyword})
    nodes = data_list(result)
    if leaf is not None:
        nodes = [node for node in nodes if str(get_prop(node, ["leaf"])) == str(leaf)]
    match = find_one_by_name(nodes, keyword, ("name", "customer_name"))
    if match:
        return match

    tree = call("customers", "all_customer_by_tree", {"keyword": keyword})
    nodes = flatten_tree(data_list(tree))
    if leaf is not None:
        nodes = [node for node in nodes if str(get_prop(node, ["leaf"])) == str(leaf)]
    return find_one_by_name(nodes, keyword, ("name", "customer_name"))


def ensure_area(args: argparse.Namespace, planned_writes: list[dict[str, Any]]) -> dict[str, Any]:
    area = find_customer_node(args.area_name, leaf=0)
    if area:
        return {"id": get_prop(area, ["id", "customer_id"]), "name": args.area_name, "existed": True}

    payload = {
        "menu_id": args.customer_menu_id,
        "leaf": 0,
        "parent_customer_id": args.root_customer_id,
        "name": args.area_name,
        "province_id": args.province_id,
        "city_id": args.city_id,
        "district_id": args.district_id,
        "receive_address": "",
        "contact_name": args.contact_name,
        "contact_phone": args.contact_phone,
    }
    planned_writes.append({"call": "scs.customers.create_or_update", "params": payload})
    if not args.execute:
        return {"id": "<created_area_id>", "name": args.area_name, "existed": False}

    created = call("customers", "create_or_update", payload)
    return {"id": get_prop(created, ["id", "customer_id"]), "name": args.area_name, "existed": False}


def ensure_child_customer(
    args: argparse.Namespace,
    parent_customer_id: str,
    planned_writes: list[dict[str, Any]],
) -> dict[str, Any]:
    customer = find_customer_node(args.customer_name, leaf=1)
    if customer:
        return {"id": get_prop(customer, ["id", "customer_id"]), "name": args.customer_name, "existed": True}

    payload = {
        "menu_id": args.customer_menu_id,
        "name": args.customer_name,
        "province_id": args.province_id,
        "city_id": args.city_id,
        "district_id": args.district_id,
        "receive_province_id": args.province_id,
        "receive_city_id": args.city_id,
        "receive_district_id": args.district_id,
        "receive_address": args.address,
        "address": args.address,
        "contact_name": args.contact_name,
        "contact_phone": args.contact_phone,
        "all_quota": 0,
        "available_quota": 0,
        "provisional_quota": 0,
        "account_period_type": args.account_period_type,
        "period_model": args.period_model,
        "account_period": 0,
        "can_order": args.can_order,
        "pay_type": args.pay_type,
        "parent_customer_id": parent_customer_id,
        "leaf": 1,
        "customer_type": 1,
        "customer_property": 1,
        "customer_property_type_id": args.customer_property_type_id,
        "is_need_check": args.is_need_check,
        "check_role": args.check_role,
        "customer_avatar": "",
        "platform_money_support_type": 0,
        "customer_agent": 0,
        "order_notice": "",
    }
    planned_writes.append({"call": "scs.customers.create_or_update_child", "params": payload})
    if not args.execute:
        return {"id": "<created_customer_id>", "name": args.customer_name, "existed": False}

    created = call("customers", "create_or_update_child", payload)
    return {"id": get_prop(created, ["id", "customer_id"]), "name": args.customer_name, "existed": False}


def ensure_receive_address(
    args: argparse.Namespace,
    customer_id: str,
    planned_writes: list[dict[str, Any]],
) -> dict[str, Any]:
    if not customer_id.startswith("<"):
        receive_list = call(
            "customers",
            "customers_receive_list",
            {"customer_id": customer_id, "page_num": 1, "page_size": 50, "receive_type": 0},
        )
        existing = next(
            (
                item
                for item in data_list(receive_list)
                if get_prop(item, ["contact_info"]) == args.contact_name
                and get_prop(item, ["tel"]) == args.contact_phone
                and get_prop(item, ["address"]) == args.address
            ),
            None,
        )
        if existing:
            return {
                "id": get_prop(existing, ["receive_id", "id"]),
                "existed": True,
                "contact_info": args.contact_name,
                "tel": args.contact_phone,
                "address": args.address,
            }

    payload = {
        "menu_id": args.customer_menu_id,
        "customers_id": customer_id,
        "province_id": args.province_id,
        "city_id": args.city_id,
        "district_id": args.district_id,
        "address": args.address,
        "contact_info": args.contact_name,
        "tel": args.contact_phone,
        "is_customer": 1,
    }
    planned_writes.append({"call": "scs.customers.create_or_update_receive_by_distributor", "params": payload})
    if not args.execute:
        return {
            "id": "<created_receive_id>",
            "existed": False,
            "contact_info": args.contact_name,
            "tel": args.contact_phone,
            "address": args.address,
        }

    created = call("customers", "create_or_update_receive_by_distributor", payload)
    return {
        "id": get_prop(created, ["id", "receive_id"]),
        "existed": False,
        "contact_info": args.contact_name,
        "tel": args.contact_phone,
        "address": args.address,
    }


def ensure_account(
    args: argparse.Namespace,
    customer_id: str,
    login_name: str,
    display_name: str,
    role_id: str,
    planned_writes: list[dict[str, Any]],
) -> dict[str, Any]:
    check = call("accounts", "name.check", {"account_name": login_name, "account_id": ""})
    if bool(check.get("has")):
        return {"id": "", "login_name": login_name, "role_id": role_id, "existed": True}

    payload = {
        "menu_id": args.account_menu_id,
        "login_name": login_name,
        "name": display_name,
        "role_ids": [role_id],
        "account_type": 5,
        "status": 1,
        "child_customer_ids": [customer_id],
        "storage_type_ids": [],
        "customer_attribute": 1,
    }
    planned_writes.append({"call": "scs.accounts.create.jt", "params": payload})
    if not args.execute:
        return {"id": "<created_account_id>", "login_name": login_name, "role_id": role_id, "existed": False}

    created = call("accounts", "create.jt", payload)
    return {
        "id": get_prop(created, ["id", "account_id"]),
        "login_name": login_name,
        "role_id": role_id,
        "existed": False,
    }


def test_account_login(login_name: str) -> dict[str, Any]:
    login = call("login", "do", {"login_name": login_name, "password": login_name})
    roles = login.get("roles") if isinstance(login, dict) else []
    return {
        "login_name": login_name,
        "account_id": get_prop(login, ["id", "account_id"]),
        "account_type": get_prop(login, ["account_type"]),
        "customer_id": get_prop(login, ["customer_id"]),
        "customer_name": get_prop(login, ["customer_name"]),
        "roles": [get_prop(role, ["role_name", "name"]) for role in roles if isinstance(role, dict)],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Onboard an SCS customer through relais (legacy scs site).")
    parser.add_argument("--login-name", default="", help="admin login used only with --store-token")
    parser.add_argument("--password", default="", help="admin password used only with --store-token")
    parser.add_argument("--store-token", action="store_true", help="login first and store acs_token in relais vault")
    parser.add_argument("--execute", action="store_true", help="perform writes; omitted means dry run")
    parser.add_argument("--customer-name", required=True)
    parser.add_argument("--customer-initials", required=True)
    parser.add_argument("--area-name", required=True)
    parser.add_argument("--root-customer-id", default="1")
    parser.add_argument("--province-id", required=True)
    parser.add_argument("--city-id", required=True)
    parser.add_argument("--district-id", required=True)
    parser.add_argument("--address", required=True)
    parser.add_argument("--contact-name", required=True)
    parser.add_argument("--contact-phone", required=True)
    parser.add_argument("--customer-property-type-id", type=int, default=2)
    parser.add_argument("--account-period-type", type=int, default=1)
    parser.add_argument("--period-model", type=int, default=1)
    parser.add_argument("--pay-type", type=int, default=2)
    parser.add_argument("--can-order", type=int, default=1)
    parser.add_argument("--is-need-check", type=int, default=1)
    parser.add_argument("--check-role", type=int, default=1)
    parser.add_argument("--customer-menu-id", default="18")
    parser.add_argument("--account-menu-id", default="12")
    parser.add_argument("--buyer-account-display-name", default="")
    parser.add_argument("--manager-account-display-name", default="")
    parser.add_argument("--skip-login-verify", action="store_true")
    return parser.parse_args()


def run(args: argparse.Namespace) -> dict[str, Any]:
    bootstrap = bootstrap_token_if_requested(args)
    planned_writes: list[dict[str, Any]] = []

    area = ensure_area(args, planned_writes)
    customer = ensure_child_customer(args, area["id"], planned_writes)
    receive_address = ensure_receive_address(args, customer["id"], planned_writes)

    buyer_login_name = f"{args.customer_initials}CGY"
    manager_login_name = f"{args.customer_initials}GLY"
    buyer_display_name = args.buyer_account_display_name or f"{args.customer_name}-CGY"
    manager_display_name = args.manager_account_display_name or f"{args.customer_name}-GLY"

    buyer_account = ensure_account(args, customer["id"], buyer_login_name, buyer_display_name, "18", planned_writes)
    manager_account = ensure_account(args, customer["id"], manager_login_name, manager_display_name, "17", planned_writes)

    login_verified: list[dict[str, Any]] = []
    if args.execute and not args.skip_login_verify:
        login_verified = [test_account_login(account["login_name"]) for account in (buyer_account, manager_account)]
        for verification in login_verified:
            if verification["customer_id"] != customer["id"]:
                raise ScsRelaisError(
                    f"account {verification['login_name']} logs into customer "
                    f"{verification['customer_name']}({verification['customer_id']}), expected "
                    f"{args.customer_name}({customer['id']})"
                )

    return {
        "dry_run": not args.execute,
        "token_bootstrapped": bool(bootstrap),
        "customer": customer,
        "area": area,
        "receive_address": receive_address,
        "accounts": [buyer_account, manager_account],
        "login_verified": login_verified,
        "planned_writes": planned_writes if not args.execute else [],
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
