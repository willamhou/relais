#!/usr/bin/env python3
"""Golden tests for gen_spec.generate_spec — the swagger->spec mapping.

Highest-leverage layer of the scs-legacy test plan: all 1324 endpoints share
this one transform, so pinning the mapping rules here pins the generated
method/path/params for every endpoint.

Run:  python3 -m unittest -v   (from this directory)
  or: python3 test_gen_spec.py
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_spec import build_params, generate_spec, prop_schema  # noqa: E402


# A tiny swagger that exercises every mapping rule.
SWAGGER = {
    "basePath": "/1",
    "tags": [{"name": "accounts", "description": "账号管理\n"}],
    "definitions": {
        "controllers.CreateForm": {
            "type": "object",
            "required": ["name", "acs_token"],
            "properties": {
                "name": {"type": "string", "description": "名称"},
                "acs_token": {"type": "string", "description": "凭证"},
                "ids": {"type": "array", "items": {"type": "string"}},
            },
        },
        "controllers.SupplierReq": {
            "type": "object",
            "properties": {"supplier_id": {"type": "string"}},
        },
    },
    "paths": {
        "/accounts/create": {
            "post": {
                "tags": ["accounts"],
                "description": "创建账号",
                "parameters": [{"in": "body", "schema": {"$ref": "#/definitions/controllers.CreateForm"}}],
            }
        },
        "/accounts/create/jt": {
            "post": {"tags": ["accounts"], "description": "创建jt", "parameters": []}
        },
        "/accounts/list": {
            "get": {
                "tags": ["accounts"],
                "parameters": [
                    {"in": "query", "name": "page", "type": "integer", "required": True},
                    {"in": "query", "name": "acs_token", "type": "string"},
                ],
            }
        },
        "/advice/": {
            "post": {"tags": ["advice"], "description": "", "parameters": []}
        },
        # Swagger data glitch: `in` holds a definition name instead of "body".
        "/pay/confirm": {
            "post": {"tags": ["pay"], "parameters": [{"in": "controllers.SupplierReq", "name": "x"}]}
        },
        # Same path with two HTTP methods -> action ids must not collide.
        "/goods/sync": {
            "get": {"tags": ["goods"], "parameters": []},
            "post": {"tags": ["goods"], "parameters": []},
        },
        # Non-operation keys must be ignored.
        "/ignored/x": {"parameters": [], "options": {}},
    },
}


class GenerateSpecGolden(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.spec = generate_spec(SWAGGER)
        cls.mods = cls.spec["modules"]

    def test_base_path_passthrough(self):
        self.assertEqual(self.spec["base_path"], "/1")

    def test_module_is_first_segment(self):
        self.assertIn("accounts", self.mods)
        self.assertIn("advice", self.mods)
        self.assertIn("goods", self.mods)

    def test_tag_description_becomes_module_description(self):
        self.assertEqual(self.mods["accounts"]["description"], "账号管理")

    def test_action_is_remaining_segments(self):
        acts = self.mods["accounts"]["actions"]
        self.assertEqual(acts["create"]["method"], "POST")
        self.assertEqual(acts["create"]["path"], "/accounts/create")

    def test_multi_segment_action_is_dotted(self):
        self.assertIn("create.jt", self.mods["accounts"]["actions"])
        self.assertEqual(self.mods["accounts"]["actions"]["create.jt"]["path"], "/accounts/create/jt")

    def test_single_segment_path_is_index(self):
        # "/advice/" -> segments ["advice"] -> action "index"
        self.assertIn("index", self.mods["advice"]["actions"])

    def test_body_ref_is_dereferenced(self):
        props = self.mods["accounts"]["actions"]["create"]["params"]["properties"]
        self.assertIn("name", props)
        self.assertIn("ids", props)

    def test_acs_token_stripped_from_body_and_required(self):
        params = self.mods["accounts"]["actions"]["create"]["params"]
        self.assertNotIn("acs_token", params["properties"])
        self.assertEqual(params["required"], ["name"])  # acs_token removed, sorted

    def test_acs_token_stripped_from_query(self):
        params = self.mods["accounts"]["actions"]["list"]["params"]
        self.assertIn("page", params["properties"])
        self.assertNotIn("acs_token", params["properties"])
        self.assertEqual(params["required"], ["page"])

    def test_get_method_uppercased(self):
        self.assertEqual(self.mods["accounts"]["actions"]["list"]["method"], "GET")

    def test_array_items_preserved(self):
        ids = self.mods["accounts"]["actions"]["create"]["params"]["properties"]["ids"]
        self.assertEqual(ids["type"], "array")
        self.assertEqual(ids["items"]["type"], "string")

    def test_anomalous_in_field_treated_as_body(self):
        props = self.mods["pay"]["actions"]["confirm"]["params"]["properties"]
        self.assertIn("supplier_id", props)

    def test_same_path_two_methods_disambiguated(self):
        acts = self.mods["goods"]["actions"]
        # one keeps "sync", the other gets a method suffix
        self.assertIn("sync", acts)
        self.assertIn("sync.post", acts)
        methods = {a["method"] for a in acts.values()}
        self.assertEqual(methods, {"GET", "POST"})

    def test_non_operation_keys_ignored(self):
        # "/ignored/x" has only "parameters"/"options" -> no real action
        acts = self.mods.get("ignored", {}).get("actions", {})
        self.assertEqual(acts, {})


class PureHelpers(unittest.TestCase):
    def test_prop_schema_keeps_description(self):
        self.assertEqual(
            prop_schema({"type": "integer", "description": " n "}),
            {"type": "integer", "description": "n"},
        )

    def test_prop_schema_defaults_to_string(self):
        self.assertEqual(prop_schema({}), {"type": "string"})

    def test_build_params_empty_when_no_parameters(self):
        self.assertEqual(build_params({}, {}), {"type": "object", "properties": {}})


if __name__ == "__main__":
    unittest.main(verbosity=2)
