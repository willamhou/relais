#!/usr/bin/env python3
"""Create SCS legacy goods master data through relais.

Default mode is a dry run: it resolves catalog IDs and prints the create payload.
Pass --execute to call scs.goods.create.
"""

from __future__ import annotations

import argparse
import re
import sys
from typing import Any

from scs_relais_common import (
    ScsRelaisError,
    bootstrap_token_if_requested,
    call,
    data_list,
    dump_json,
    get_prop,
)


def normalize_catalog_name(value: Any) -> str:
    text = "" if value is None else str(value)
    text = text.strip().lower()
    text = re.sub(r"^[a-z]\.", "", text)
    text = re.sub(r"^\d+\.", "", text)
    text = text.replace("及其他", "")
    return re.sub(r"[\s\-/\\_()（）]", "", text)


def item_name(item: Any) -> str:
    return get_prop(item, ["name", "category_name", "specification_name", "unit_type_name"])


def item_id(item: Any) -> str:
    return get_prop(item, ["id", "category_id", "specification_id", "prd_prop_id", "prd_prop_dtl_id"])


def find_one_by_catalog_name(items: list[Any], name: str, label: str) -> Any:
    exact = [item for item in items if item_name(item) == name]
    if len(exact) == 1:
        return exact[0]

    target = normalize_catalog_name(name)
    normalized = [item for item in items if normalize_catalog_name(item_name(item)) == target]
    if len(normalized) == 1:
        return normalized[0]

    contains = [
        item
        for item in items
        if target
        and (
            normalize_catalog_name(item_name(item)) in target
            or target in normalize_catalog_name(item_name(item))
        )
    ]
    if len(contains) == 1:
        return contains[0]

    if len(items) == 1:
        return items[0]

    candidates = ", ".join(f"{item_id(item)}:{item_name(item)}" for item in items[:20])
    raise ScsRelaisError(f"cannot resolve {label} '{name}'. Candidates: {candidates}")


def select_unit(unit_response: Any, name: str, unit_type_name: str) -> dict[str, Any]:
    units: list[dict[str, Any]] = []
    for group in unit_response.get("data_select_goods_unit_list", []) if isinstance(unit_response, dict) else []:
        for unit in group.get("data_list", []) if isinstance(group, dict) else []:
            units.append(
                {
                    "id": get_prop(unit, ["id"]),
                    "name": get_prop(unit, ["name"]),
                    "unit_type_id": get_prop(unit, ["unit_type_id"]),
                    "unit_type_name": get_prop(group, ["unit_type_name"]),
                }
            )

    matches = [unit for unit in units if unit["name"] == name]
    if unit_type_name:
        preferred = [unit for unit in matches if unit["unit_type_name"] == unit_type_name]
        if len(preferred) == 1:
            return preferred[0]
    return find_one_by_catalog_name(matches, name, "unit")


def parse_property_selections(text: str) -> list[tuple[str, str]]:
    result: list[tuple[str, str]] = []
    for part in text.split(";"):
        trimmed = part.strip()
        if not trimmed:
            continue
        pieces = trimmed.split("=", 1)
        if len(pieces) != 2 or not pieces[0].strip() or not pieces[1].strip():
            raise ScsRelaisError(
                f"invalid --product-property-selections item '{trimmed}'. Use property=value;property=value"
            )
        result.append((pieces[0].strip(), pieces[1].strip()))
    if not result:
        raise ScsRelaisError("--product-property-selections cannot be empty")
    return result


def resolve_goods_props(
    args: argparse.Namespace,
    product_id: str,
    planned_writes: list[dict[str, Any]],
) -> dict[str, list[str]]:
    property_list = call("product_properties", "by_product", {"prd_id": product_id})
    props = data_list(property_list)
    goods_props: dict[str, list[str]] = {}

    for prop_name, detail_name in parse_property_selections(args.product_property_selections):
        prop = find_one_by_catalog_name(props, prop_name, "product property")
        details = prop.get("details", []) if isinstance(prop, dict) else []
        try:
            detail = find_one_by_catalog_name(list(details), detail_name, "product property detail")
        except ScsRelaisError:
            if not args.create_missing_property_details:
                raise
            create_payload = {"prd_prop_id": item_id(prop), "name": detail_name}
            planned_writes.append({"call": "scs.product_properties.details.create", "params": create_payload})
            if not args.execute:
                goods_props[item_id(prop)] = [f"<created_property_detail_id:{prop_name}={detail_name}>"]
                continue
            created = call("product_properties", "details.create", create_payload)
            refreshed = call("product_properties", "by_product", {"prd_id": product_id})
            prop = find_one_by_catalog_name(data_list(refreshed), prop_name, "product property")
            details = prop.get("details", []) if isinstance(prop, dict) else []
            try:
                detail = find_one_by_catalog_name(list(details), detail_name, "product property detail")
            except ScsRelaisError:
                detail = created
            if not item_id(detail):
                detail = created
        goods_props[item_id(prop)] = [item_id(detail)]

    return goods_props


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Create SCS goods master data through relais.")
    parser.add_argument("--login-name", default="", help="admin login used only with --store-token")
    parser.add_argument("--password", default="", help="admin password used only with --store-token")
    parser.add_argument("--store-token", action="store_true", help="login first and store acs_token in relais vault")
    parser.add_argument("--execute", action="store_true", help="perform writes; omitted means dry run")
    parser.add_argument("--goods-name", required=True)
    parser.add_argument("--cate-lv1-name", required=True)
    parser.add_argument("--cate-lv2-name", required=True)
    parser.add_argument("--product-name", required=True)
    parser.add_argument("--unit-name", required=True)
    parser.add_argument("--unit-type-name", default="")
    parser.add_argument("--specification-name", required=True)
    parser.add_argument("--category-name", required=True)
    parser.add_argument("--attribute-name", required=True)
    parser.add_argument("--product-property-selections", required=True)
    parser.add_argument("--base-description", default="Test goods for flow verification")
    parser.add_argument("--spec-scaling", type=int, default=1)
    parser.add_argument("--range-id", type=int, default=1)
    parser.add_argument("--goods-mark", type=int, default=1)
    parser.add_argument("--self-purchase", type=int, default=2)
    parser.add_argument("--is-support-invoicing", type=int, default=1)
    parser.add_argument("--menu-id", default="14")
    parser.add_argument(
        "--create-missing-property-details",
        action="store_true",
        help="create missing product property detail rows before goods.create",
    )
    return parser.parse_args()


def run(args: argparse.Namespace) -> dict[str, Any]:
    bootstrap = bootstrap_token_if_requested(args)
    planned_writes: list[dict[str, Any]] = []

    cate_lv1 = find_one_by_catalog_name(
        data_list(call("product_categories", "by_parent", {"parent_id": "0"})),
        args.cate_lv1_name,
        "first category",
    )
    cate_lv2 = find_one_by_catalog_name(
        data_list(call("product_categories", "by_parent", {"parent_id": item_id(cate_lv1)})),
        args.cate_lv2_name,
        "second category",
    )
    product = find_one_by_catalog_name(
        data_list(
            call(
                "products",
                "by_category",
                {"cate_lv1_id": item_id(cate_lv1), "cate_lv2_id": item_id(cate_lv2)},
            )
        ),
        args.product_name,
        "product",
    )
    unit = select_unit(call("goods", "selectGoodsUnit", {}), args.unit_name, args.unit_type_name)
    specification = find_one_by_catalog_name(
        data_list(call("goods", "select_specification", {})),
        args.specification_name,
        "specification",
    )
    category = find_one_by_catalog_name(
        data_list(call("goods", "select_category", {})),
        args.category_name,
        "goods property category",
    )
    attribute = find_one_by_catalog_name(
        data_list(call("attribute_form", "GetAllAttribute", {})),
        args.attribute_name,
        "goods attribute",
    )
    goods_props = resolve_goods_props(args, item_id(product), planned_writes)

    payload = {
        "menu_id": args.menu_id,
        "cate_lv1_id": item_id(cate_lv1),
        "cate_lv2_id": item_id(cate_lv2),
        "prd_id": item_id(product),
        "name": args.goods_name,
        "unit": item_id(unit),
        "goods_code": "",
        "main_pic_url": "",
        "base_description": args.base_description,
        "dtl_description": "",
        "exam_rpt_pic_url": "",
        "goods_pics": [],
        "goods_props": goods_props,
        "spec_scaling": args.spec_scaling,
        "goods_detail_pics": [],
        "check_regenerant": [],
        "specification_id": item_id(specification),
        "category_id": item_id(category),
        "dealer_id": "",
        "attribute_id": item_id(attribute),
        "range_id": args.range_id,
        "goods_mark": args.goods_mark,
        "self_purchase": args.self_purchase,
        "bar_code": "",
        "en_name": "",
        "is_support_invoicing": args.is_support_invoicing,
    }
    planned_writes.append({"call": "scs.goods.create", "params": payload})

    resolved = {
        "cate_lv1": f"{item_id(cate_lv1)} {item_name(cate_lv1)}",
        "cate_lv2": f"{item_id(cate_lv2)} {item_name(cate_lv2)}",
        "product": f"{item_id(product)} {item_name(product)}",
        "unit": f"{unit['id']} {unit['name']} / {unit['unit_type_name']}",
        "specification": f"{item_id(specification)} {item_name(specification)}",
        "category": f"{item_id(category)} {item_name(category)}",
        "attribute": f"{item_id(attribute)} {item_name(attribute)}",
    }

    if not args.execute:
        return {
            "dry_run": True,
            "token_bootstrapped": bool(bootstrap),
            "resolved": resolved,
            "payload": payload,
            "planned_writes": planned_writes,
        }

    create_response = call("goods", "create", payload)
    created_id = get_prop(create_response, ["id"])
    if not created_id and isinstance(create_response, (str, int)):
        created_id = str(create_response)
    if not created_id:
        raise ScsRelaisError(f"goods.create response missing id: {create_response}")

    show = call("goods", "show", {"id": created_id})
    return {
        "dry_run": False,
        "token_bootstrapped": bool(bootstrap),
        "created": True,
        "resolved": resolved,
        "goods_id": get_prop(show, ["id", "goods_id"]),
        "goods_code": get_prop(show, ["goods_code"]),
        "goods_name": get_prop(show, ["name", "goods_name"]),
        "cate_lv1_id": get_prop(show, ["cate_lv1_id"]),
        "cate_lv2_id": get_prop(show, ["cate_lv2_id"]),
        "prd_id": get_prop(show, ["prd_id"]),
        "unit": get_prop(show, ["unit"]),
        "unit_id": get_prop(show, ["unit_id"]),
        "specification_id": get_prop(show, ["specification_id"]),
        "category_id": get_prop(show, ["category_id"]),
        "attribute_id": get_prop(show, ["attribute_id"]),
        "self_purchase": args.self_purchase,
        "is_support_invoicing": get_prop(show, ["is_support_invoicing"]),
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
