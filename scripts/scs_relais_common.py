#!/usr/bin/env python3
"""Shared helpers for SCS legacy flows driven through the relais CLI."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from collections.abc import Iterable
from typing import Any


BIN = os.environ.get("RELAIS_BIN", "relais")
SITE = "scs"


class ScsRelaisError(RuntimeError):
    """Raised when relais transport or SCS business logic reports a failure."""


def dump_json(value: Any) -> None:
    print(json.dumps(value, ensure_ascii=False, indent=2))


def require_no_acs_token(params: Any) -> None:
    if isinstance(params, dict):
        if "acs_token" in params:
            raise ScsRelaisError("do not pass acs_token in --data; store it in relais vault for site 'scs'")
        for value in params.values():
            require_no_acs_token(value)
    elif isinstance(params, list):
        for value in params:
            require_no_acs_token(value)


def call(resource: str, action: str, params: dict[str, Any] | None = None) -> Any:
    """Execute one `relais exec scs.<resource>.<action>` call and return SCS data."""

    params = params or {}
    require_no_acs_token(params)
    payload = json.dumps(params, ensure_ascii=False, separators=(",", ":"))
    proc = subprocess.run(
        [BIN, "exec", f"{SITE}.{resource}.{action}", "--data", payload],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise ScsRelaisError(
            f"relais exec {SITE}.{resource}.{action} failed: {proc.stderr.strip() or proc.stdout.strip()}"
        )

    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise ScsRelaisError(
            f"relais exec {SITE}.{resource}.{action} returned non-JSON output: {proc.stdout[:300]}"
        ) from exc

    data = envelope.get("data") if isinstance(envelope, dict) and "data" in envelope else envelope
    err_code = data.get("err_code") if isinstance(data, dict) else None
    if err_code not in (None, "", 0, "0"):
        raise ScsRelaisError(
            f"{SITE}.{resource}.{action} business error: err_code={err_code} err_msg={data.get('err_msg')}"
        )
    return data


def login_and_store(login_name: str, password: str) -> dict[str, Any]:
    """Login through relais and store the returned acs_token under site `scs`."""

    login = call("login", "do", {"login_name": login_name, "password": password})
    token = login.get("acs_token") if isinstance(login, dict) else None
    if not token:
        raise ScsRelaisError("login response missing acs_token")

    token_file = ""
    try:
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", delete=False) as handle:
            token_file = handle.name
            os.chmod(token_file, 0o600)
            handle.write(token)

        proc = subprocess.run(
            [BIN, "vault", "store", SITE, "--token-file", token_file],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0 and "unexpected argument '--token-file'" in proc.stderr:
            proc = subprocess.run(
                [BIN, "vault", "store", SITE, token],
                capture_output=True,
                text=True,
            )
        if proc.returncode != 0:
            raise ScsRelaisError(f"relais vault store {SITE} failed: {proc.stderr.strip() or proc.stdout.strip()}")
    finally:
        if token_file:
            try:
                os.unlink(token_file)
            except FileNotFoundError:
                pass
    return login


def data_list(response: Any) -> list[Any]:
    if isinstance(response, dict):
        value = response.get("data_list")
        if isinstance(value, list):
            return value
        if value is None:
            return []
        return [value]
    return []


def get_prop(item: Any, names: Iterable[str]) -> str:
    if not isinstance(item, dict):
        return ""
    for name in names:
        value = item.get(name)
        if value is not None and str(value) != "":
            return str(value)
    return ""


def flatten_tree(roots: Any) -> list[Any]:
    result: list[Any] = []
    stack = list(roots if isinstance(roots, list) else [roots])
    while stack:
        node = stack.pop()
        if not isinstance(node, dict):
            continue
        result.append(node)
        children = node.get("children")
        if isinstance(children, list):
            stack.extend(children)
    return result


def normalize_name(value: Any) -> str:
    text = "" if value is None else str(value)
    return "".join(text.strip().lower().split())


def find_one_by_name(items: Iterable[Any], name: str, fields: Iterable[str] = ("name",)) -> Any | None:
    items = list(items)
    target = normalize_name(name)

    exact = [
        item
        for item in items
        if any(normalize_name(get_prop(item, [field])) == target for field in fields)
    ]
    if len(exact) == 1:
        return exact[0]
    if len(exact) > 1:
        names = ", ".join(get_prop(item, fields) for item in exact[:10])
        raise ScsRelaisError(f"multiple exact matches for '{name}': {names}")

    contains = [
        item
        for item in items
        if any(
            (target in normalize_name(get_prop(item, [field])) or normalize_name(get_prop(item, [field])) in target)
            and normalize_name(get_prop(item, [field]))
            for field in fields
        )
    ]
    if len(contains) == 1:
        return contains[0]
    if len(contains) > 1:
        names = ", ".join(get_prop(item, fields) for item in contains[:10])
        raise ScsRelaisError(f"multiple fuzzy matches for '{name}': {names}")
    return None


def bootstrap_token_if_requested(args: Any) -> dict[str, Any] | None:
    if not getattr(args, "store_token", False):
        return None
    if not getattr(args, "login_name", "") or not getattr(args, "password", ""):
        raise ScsRelaisError("--login-name and --password are required with --store-token")
    return login_and_store(args.login_name, args.password)
