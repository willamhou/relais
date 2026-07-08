---
name: scs-goods-flow
description: Create and shelf goods in legacy SCS through Relais. Use when creating goods master data, resolving product/category/unit/property IDs, configuring supplier goods, creating/submitting rating forms, or migrating goods_create/goods_shelf PowerShell flows to relais.
---

# SCS Goods Flow

Use this skill for legacy SCS goods master-data creation and shelf submission
through the `scs` Relais site. The deterministic runners are:

- `scripts/goods_create_relais.py`
- `scripts/goods_shelf_submit_relais.py`

## Safety

Both runners default to dry-run mode. Add `--execute` only after reviewing the
resolved IDs and planned payloads. Relais injects `acs_token` from the `scs`
vault; never include it in `--data`.

```sh
export SCS_LEGACY_BASE_URL=https://api.tffair.cn
relais vault store scs <acs_token>
```

## Goods Create

Use `goods_create_relais.py` to resolve category/product/unit/specification/
attribute IDs and build the `scs.goods.create` payload. This creates only goods
master data; it does not configure supplier supply, price, customer availability,
stock, or shelf status.

Dry run:

```sh
python3 scripts/goods_create_relais.py \
  --goods-name '测试无效舌兰酒50ml' \
  --cate-lv1-name '烈酒及基酒' \
  --cate-lv2-name '龙舌兰' \
  --product-name '豪帅JC' \
  --unit-name '瓶' \
  --unit-type-name '酒水' \
  --specification-name '50ml*120' \
  --category-name '酒水' \
  --attribute-name '洋酒' \
  --product-property-selections '规格=50ml*120'
```

If a product property detail is missing, rerun with
`--create-missing-property-details`; in dry-run mode this only prints the planned
`scs.product_properties.details.create` call.

## Shelf Submit

Use `goods_shelf_submit_relais.py` after goods master data exists. The runner:

1. Resolves goods, supplier, and customer scopes.
2. Checks whether the supplier can supply the goods for each customer.
3. Configures supplier goods with `scs.supplier_goods_config.change` if needed.
4. Creates a rating form with `scs.rating_form.add_rating_form_by_cus`.
5. Submits the rating form with `scs.rating_form.update_rating_form_status`.
6. Stops at pending audit and prints handoff JSON.

Dry run:

```sh
python3 scripts/goods_shelf_submit_relais.py \
  --goods-name '测试无效舌兰酒50ml' \
  --supplier-name '广东20支' \
  --price 99 \
  --customer-names '广东20支测试客户3,广东20支测试客户4'
```

Execute:

```sh
python3 scripts/goods_shelf_submit_relais.py ... --execute
```

Do not audit in this flow. Audit requires a separate reviewer/account with rating
form audit permission; the submit runner intentionally never calls
`scs.rating_form.auditing_rating_form`.
