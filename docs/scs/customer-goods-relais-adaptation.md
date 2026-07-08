# SCS Customer And Goods Relais Adaptation

This note records how the imported PowerShell validation flows map onto Relais.
The legacy SCS adapter already exposes the needed `/1/*` endpoints as site
`scs`, so the adaptation layer is a set of thin orchestration scripts rather
than new Rust adapter code.

## Scripts

| Flow | Relais-native runner | Original flow |
|---|---|---|
| Customer onboarding | `scripts/customer_onboarding_relais.py` | `customer_onboarding_test(1).ps1` |
| Goods create | `scripts/goods_create_relais.py` | `goods_create_test(1).ps1` |
| Goods shelf submit | `scripts/goods_shelf_submit_relais.py` | `goods_shelf_submit_test(1).ps1` |

All runners default to dry-run mode. Add `--execute` to perform writes.

## Auth

Store the legacy `acs_token` in the Relais vault:

```sh
export SCS_LEGACY_BASE_URL=https://api.tffair.cn
relais exec scs.login.do --data '{"login_name":"...","password":"..."}'
relais vault store scs <acs_token>
```

The scripts never pass `acs_token` in payloads. Relais injects it for site `scs`.
Each script also supports `--login-name ... --password ... --store-token` to
bootstrap the vault before running.

## Endpoint Mapping

| Legacy endpoint | Relais call |
|---|---|
| `/customers/customer_by_tree_select_list` | `scs.customers.customer_by_tree_select_list` |
| `/customers/all_customer_by_tree` | `scs.customers.all_customer_by_tree` |
| `/customers/create_or_update` | `scs.customers.create_or_update` |
| `/customers/create_or_update_child` | `scs.customers.create_or_update_child` |
| `/customers/create_or_update_receive_by_distributor` | `scs.customers.create_or_update_receive_by_distributor` |
| `/accounts/name/check` | `scs.accounts.name.check` |
| `/accounts/create/jt` | `scs.accounts.create.jt` |
| `/goods/create` | `scs.goods.create` |
| `/goods/show` | `scs.goods.show` |
| `/goods/` | `scs.goods.index` |
| `/supplier_goods_config/change` | `scs.supplier_goods_config.change` |
| `/suppliers/select_by_supplier_goods_config` | `scs.suppliers.select_by_supplier_goods_config` |
| `/rating_form/add_rating_form_by_cus` | `scs.rating_form.add_rating_form_by_cus` |
| `/rating_form/update_rating_form_status` | `scs.rating_form.update_rating_form_status` |

## Skill Entry Points

- `skills/scs-customer-onboarding/SKILL.md`
- `skills/scs-goods-flow/SKILL.md`

The shelf-submit flow intentionally stops at pending audit. Audit remains a
separate human or agent step using an account with rating-form audit permission.
