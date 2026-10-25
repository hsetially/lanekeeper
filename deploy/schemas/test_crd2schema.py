"""Tests for crd2schema.py: the conversion of a CustomResourceDefinition into the strict JSON Schemas that kubeconform reads.

Run: python3 -B -I -m unittest discover -s deploy/schemas -p 'test_*.py'
"""

import unittest

import crd2schema

CRD = {
    "apiVersion": "apiextensions.k8s.io/v1",
    "kind": "CustomResourceDefinition",
    "spec": {
        "group": "example.dev",
        "names": {"kind": "Widget", "plural": "widgets"},
        "versions": [
            {
                "name": "v1",
                "schema": {
                    "openAPIV3Schema": {
                        "type": "object",
                        "properties": {
                            "spec": {
                                "type": "object",
                                "properties": {
                                    "size": {"x-kubernetes-int-or-string": True, "description": "a size"},
                                    "labels": {"type": "object", "additionalProperties": {"type": "string"}},
                                    "free": {"type": "object", "x-kubernetes-preserve-unknown-fields": True, "properties": {"a": {"type": "string"}}},
                                    "items": {
                                        "type": "array",
                                        "items": {"type": "object", "properties": {"name": {"type": "string"}}},
                                    },
                                    "properties": {"type": "string"},
                                },
                            }
                        },
                    }
                },
            },
            {"name": "v2", "served": True, "schema": {"openAPIV3Schema": {"type": "object", "properties": {"a": {"type": "string"}}}}},
            {"name": "v3"},
        ],
    },
}


class ConvertTest(unittest.TestCase):
    def converted(self):
        return {(g, k, v): s for g, k, v, s in crd2schema.convert_crd(CRD)}

    def test_one_schema_per_version_that_has_one(self):
        self.assertEqual(set(self.converted()), {("example.dev", "widget", "v1"), ("example.dev", "widget", "v2")})

    def test_objects_with_properties_refuse_unknown_fields(self):
        s = self.converted()[("example.dev", "widget", "v1")]
        self.assertIs(s["additionalProperties"], False)
        self.assertIs(s["properties"]["spec"]["additionalProperties"], False)
        self.assertIs(s["properties"]["spec"]["properties"]["items"]["items"]["additionalProperties"], False)

    def test_a_field_named_properties_is_not_mistaken_for_the_keyword(self):
        s = self.converted()[("example.dev", "widget", "v1")]
        self.assertNotIn("additionalProperties", s["properties"]["spec"]["properties"]["properties"])
        self.assertEqual(s["properties"]["spec"]["properties"]["properties"], {"type": "string"})

    def test_maps_and_preserved_unknown_fields_are_left_open(self):
        spec = self.converted()[("example.dev", "widget", "v1")]["properties"]["spec"]["properties"]
        self.assertEqual(spec["labels"]["additionalProperties"], {"type": "string"})
        self.assertNotIn("additionalProperties", spec["free"])

    def test_int_or_string_becomes_a_one_of(self):
        size = self.converted()[("example.dev", "widget", "v1")]["properties"]["spec"]["properties"]["size"]
        self.assertEqual(size["oneOf"], [{"type": "string"}, {"type": "integer"}])
        self.assertEqual(size["description"], "a size")
        self.assertNotIn("x-kubernetes-int-or-string", size)

    def test_branches_of_one_of_that_only_check_a_value_are_not_closed(self):
        # Gateway API's addresses: each oneOf branch lists the one property it constrains, and the parent lists the others.
        crd = {
            "kind": "CustomResourceDefinition",
            "spec": {
                "group": "example.dev",
                "names": {"kind": "Thing"},
                "versions": [
                    {
                        "name": "v1",
                        "schema": {
                            "openAPIV3Schema": {
                                "type": "object",
                                "properties": {
                                    "type": {"type": "string"},
                                    "value": {"type": "string"},
                                },
                                "oneOf": [
                                    {"properties": {"type": {"enum": ["A"]}, "value": {"format": "ipv4"}}},
                                    {"properties": {"type": {"not": {"enum": ["A"]}}}, "anyOf": [{"properties": {"value": {"minLength": 1}}}]},
                                ],
                            }
                        },
                    }
                ],
            },
        }
        (_, _, _, schema), = crd2schema.convert_crd(crd)
        self.assertIs(schema["additionalProperties"], False)
        for branch in schema["oneOf"]:
            self.assertNotIn("additionalProperties", branch)
        self.assertNotIn("additionalProperties", schema["oneOf"][1]["anyOf"][0])

    def test_the_input_is_not_changed(self):
        before = repr(CRD)
        crd2schema.convert_crd(CRD)
        self.assertEqual(repr(CRD), before)

    def test_a_document_that_is_not_a_crd_gives_nothing(self):
        self.assertEqual(crd2schema.convert_crd({"kind": "ConfigMap"}), [])

    def test_the_file_name_is_group_dir_and_lowercase_kind_and_version(self):
        self.assertEqual(crd2schema.schema_path("example.dev", "widget", "v1"), "example.dev/widget_v1.json")


if __name__ == "__main__":
    unittest.main()
