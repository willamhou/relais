#!/usr/bin/env python3
"""Align a legacy SCS test database to the code's `*Do` structs.

The bundled legacy DB dump (`scs_old/resource/archieve/scs.sql`) lags the code
schema: many tables/columns the code's xorm `*Do` structs reference are missing,
so `SELECT *` fails and business endpoints return "系统异常". This script closes
most of that gap so the live business sweep (tests/scs_legacy_business_test.rs)
can actually exercise business logic.

How it works:
  - Scans `<scs_old>/**/*.go` for every `type XxxDo struct { ... }`.
  - Derives the table name via the xorm CustMapper convention:
    `t_` + snake_case(StructName without the `Do` suffix`)  (e.g. CustomerDo -> t_customer).
  - Reads each field's column from its `json:"col"` tag and a Postgres type from
    the trailing xorm type token (VARCHAR(n)/INTEGER/DATETIME/...).
  - Diffs against the live DB and emits CREATE TABLE (for missing tables) and
    ALTER TABLE ADD COLUMN (nullable) for missing columns.

Columns are added nullable on purpose: the goal is that `SELECT *` finds every
column the struct maps, not to reproduce exact constraints.

Tables referenced only by raw SQL (no `*Do` struct) cannot be derived here — a
small tail (~1-2% of read endpoints) stays unreachable.

Usage:
  python3 schema_sync.py <path/to/scs_old> <postgres_container> [--apply]

Without --apply it prints a dry-run summary; with --apply it executes the DDL
against database `scsdb` in the given container.
"""
import glob
import os
import re
import subprocess
import sys

ROOT = sys.argv[1]
PG = sys.argv[2]
APPLY = "--apply" in sys.argv


def map_type(xorm: str) -> str:
    tok = xorm.strip().split()[-1] if xorm.strip() else "TEXT"
    t = tok.upper()
    if t.startswith("VARCHAR"):
        return "varchar" + t[len("VARCHAR"):].lower()
    if t.startswith("NUMERIC") or t.startswith("DECIMAL"):
        return "numeric"
    if t in ("INTEGER", "INT"):
        return "integer"
    if t in ("BIGINT", "INT64"):
        return "bigint"
    if t == "SMALLINT":
        return "smallint"
    if t in ("DATETIME", "TIMESTAMP"):
        return "timestamp"
    if t == "DATE":
        return "date"
    if t in ("BOOL", "BOOLEAN"):
        return "boolean"
    if t == "TEXT":
        return "text"
    if t.startswith("CHAR"):
        return "char" + t[len("CHAR"):].lower()
    if t in ("DOUBLE", "REAL", "FLOAT"):
        return "double precision"
    return "text"


def to_table(struct_name: str) -> str:
    # xorm CustMapper: snake_case(name), trim trailing "do", prefix "t_".
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", struct_name).lower()
    if s.endswith("_do"):
        s = s[: -len("_do")]
    elif s.endswith("do"):
        s = s[: -len("do")]
    return "t_" + s


def parse_desired(root: str) -> dict:
    desired = {}
    field_re = re.compile(r'json:"(\w+)"[^`]*xorm:"([^"]*)"')
    for path in glob.glob(os.path.join(root, "**", "*.go"), recursive=True):
        src = open(path, encoding="utf-8", errors="ignore").read()
        for sm in re.finditer(r"type (\w+Do) struct \{(.*?)\n\}", src, re.S):
            name, body = sm.group(1), sm.group(2)
            cols = {}
            for fm in field_re.finditer(body):
                col, xorm = fm.group(1), fm.group(2)
                if col != "-":
                    cols[col] = map_type(xorm)
            if cols:
                desired[to_table(name)] = cols
    return desired


def actual_schema(pg: str) -> dict:
    q = ("SELECT table_name, column_name FROM information_schema.columns "
         "WHERE table_schema='public';")
    out = subprocess.run(
        ["docker", "exec", pg, "psql", "-U", "postgres", "-d", "scsdb", "-tAc", q],
        capture_output=True, text=True,
    ).stdout
    actual = {}
    for line in out.splitlines():
        if "|" in line:
            t, c = line.split("|", 1)
            actual.setdefault(t.strip(), set()).add(c.strip())
    return actual


def build_ddl(desired: dict, actual: dict):
    ddl, missing_tables, missing_cols = [], 0, 0
    for table, cols in sorted(desired.items()):
        if table not in actual:
            missing_tables += 1
            defs = [f'"{c}" {t}' + (" PRIMARY KEY" if c == "id" else "") for c, t in cols.items()]
            ddl.append(f'CREATE TABLE IF NOT EXISTS "{table}" (' + ", ".join(defs) + ");")
        else:
            for c, t in cols.items():
                if c not in actual[table]:
                    missing_cols += 1
                    ddl.append(f'ALTER TABLE "{table}" ADD COLUMN IF NOT EXISTS "{c}" {t};')
    return ddl, missing_tables, missing_cols


def main():
    desired = parse_desired(ROOT)
    actual = actual_schema(PG)
    ddl, missing_tables, missing_cols = build_ddl(desired, actual)

    print(f"parsed tables: {len(desired)} | DB tables: {len(actual)}")
    print(f"missing tables: {missing_tables} | missing columns: {missing_cols}")
    print(f"DDL statements: {len(ddl)}")

    if APPLY and ddl:
        res = subprocess.run(
            ["docker", "exec", "-i", PG, "psql", "-U", "postgres", "-d", "scsdb"],
            input="\n".join(ddl), capture_output=True, text=True,
        )
        errs = [l for l in res.stderr.splitlines() if "ERROR" in l]
        print(f"applied. errors: {len(errs)}")
        for e in errs[:15]:
            print("  " + e)
    elif ddl:
        print("\n-- sample DDL (first 10) --")
        for s in ddl[:10]:
            print(s)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    main()
