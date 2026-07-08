---
name: scs-customer-onboarding
description: Onboard customers in legacy SCS through Relais. Use when creating or verifying SCS customer areas, child customers, receive addresses, customer buyer/admin accounts, or when migrating the customer_onboarding PowerShell flow to relais.
---

# SCS Customer Onboarding

Use this skill to create a complete legacy SCS customer onboarding bundle through
the `scs` Relais site. The deterministic runner is
`scripts/customer_onboarding_relais.py`.

## Safety

The flow writes real SCS data when `--execute` is present. Run without
`--execute` first; dry-run mode resolves existing records and prints planned
`relais exec scs.*` writes.

Relais injects auth from the vault. Never pass `acs_token` in payloads.

```sh
export SCS_LEGACY_BASE_URL=https://api.tffair.cn
relais exec scs.login.do --data '{"login_name":"...","password":"..."}'
relais vault store scs <acs_token>
```

Alternatively, pass `--login-name ... --password ... --store-token` to the
script once.

## Workflow

1. Resolve or create the customer area with `scs.customers.create_or_update`.
2. Resolve or create the child customer with `scs.customers.create_or_update_child`.
3. Resolve or create the receive address with
   `scs.customers.create_or_update_receive_by_distributor`.
4. Create customer buyer/admin accounts with `scs.accounts.create.jt`.
5. In execute mode, verify both accounts can log in unless `--skip-login-verify`
   is set.

## Runner

Dry run:

```sh
python3 scripts/customer_onboarding_relais.py \
  --customer-name '广东20支测试客户5' \
  --customer-initials GD20ZCSKH5 \
  --area-name '测试组' \
  --province-id 9 \
  --city-id 75 \
  --district-id 788 \
  --address '上海普陀区长寿路中环现代大厦' \
  --contact-name '王刚' \
  --contact-phone 15200000000
```

Execute:

```sh
python3 scripts/customer_onboarding_relais.py ... --execute
```

Defaults mirror the verified PowerShell flow: customer menu `18`, account menu
`12`, buyer role `18`, manager role `17`, account type `5`, customer property
type `2`, offline payment, no account period, and order audit enabled.
