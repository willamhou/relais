#!/usr/bin/env python3
"""Generate a compact relais adapter spec from the legacy SCS Swagger 2.0 file.

The legacy SCS service (scs_old, Beego) exposes ~1324 action-based endpoints
under basePath `/1`. This script distills its `swagger.json` into a small
`scs_legacy_spec.json` that the `relais-adapter-scs-legacy` engine loads at
build time (via include_str!).

Mapping:
  - module  = first path segment (also the Swagger tag) -> relais resource
  - action  = remaining path segments joined with '.'    -> relais action id
  - method  = HTTP method (mostly POST, some GET)
  - params  = body $ref deref + query/path params, with `acs_token` removed
              (acs_token is the credential, injected by the adapter, not a
              parameter the agent supplies)

The core mapping lives in `generate_spec(swagger_dict)` so it can be golden-tested
(see test_gen_spec.py); `main()` only does file I/O.

Usage:
  python3 gen_spec.py <path/to/swagger.json> <path/to/scs_legacy_spec.json>
"""
import json
import sys


def resolve_ref(defs: dict, ref: str) -> dict:
    # "#/definitions/controllers.CreateAccountForm" -> the schema object
    return defs.get(ref.split("/")[-1], {})


def prop_schema(v: dict) -> dict:
    out = {"type": v.get("type", "string")}
    if v.get("description"):
        out["description"] = v["description"].strip()
    if v.get("type") == "array" and "items" in v:
        out["items"] = {"type": v["items"].get("type", "string")}
    return out


def build_params(op: dict, defs: dict) -> dict:
    """Merge body $ref props + query/path/formData params, dropping `acs_token`."""
    props: dict = {}
    required: list = []
    for pr in op.get("parameters", []):
        loc = pr.get("in")
        if loc == "body":
            schema = pr.get("schema", {})
            if "$ref" in schema:
                schema = resolve_ref(defs, schema["$ref"])
            for k, v in schema.get("properties", {}).items():
                if k == "acs_token":
                    continue
                props[k] = prop_schema(v)
            for r in schema.get("required", []):
                if r != "acs_token":
                    required.append(r)
        elif loc in ("query", "path", "formData"):
            name = pr.get("name")
            if not name or name == "acs_token":
                continue
            props[name] = prop_schema(pr)
            if pr.get("required"):
                required.append(name)
        elif loc in defs:
            # Swagger data glitch: `in` holds a definition name; treat as body.
            for k, v in defs[loc].get("properties", {}).items():
                if k == "acs_token":
                    continue
                props[k] = prop_schema(v)
    schema = {"type": "object", "properties": props}
    if required:
        schema["required"] = sorted(set(required))
    return schema


def generate_spec(swagger: dict) -> dict:
    """Pure swagger->spec transform. No file I/O — golden-testable."""
    defs = swagger.get("definitions", {})
    tag_desc = {t["name"]: t.get("description", "").strip() for t in swagger.get("tags", [])}

    modules: dict = {}
    for path, ops in swagger.get("paths", {}).items():
        parts = [p for p in path.split("/") if p]
        if not parts:
            continue
        module = parts[0]
        action = ".".join(parts[1:]) if len(parts) > 1 else "index"
        for method, op in ops.items():
            if method not in ("get", "post", "put", "delete"):
                continue
            m = modules.setdefault(module, {"description": tag_desc.get(module, ""), "actions": {}})
            key = action
            if key in m["actions"]:
                # same action id from a different HTTP method -> disambiguate
                key = f"{action}.{method}"
            m["actions"][key] = {
                "method": method.upper(),
                "path": path,
                "description": op.get("description", "").strip(),
                "params": build_params(op, defs),
            }

    return {
        "source": "scs_old/swagger/swagger.json",
        "base_path": swagger.get("basePath", "/1"),
        "modules": dict(sorted(modules.items())),
    }


def main(swagger_path: str, out_path: str) -> None:
    with open(swagger_path, encoding="utf-8") as f:
        swagger = json.load(f)

    spec = generate_spec(swagger)

    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(spec, f, ensure_ascii=False, indent=1)
        f.write("\n")

    total = sum(len(m["actions"]) for m in spec["modules"].values())
    print(f"modules: {len(spec['modules'])}")
    print(f"total actions: {total}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    main(sys.argv[1], sys.argv[2])
