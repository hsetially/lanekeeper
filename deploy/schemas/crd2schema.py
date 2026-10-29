"""Turn a CustomResourceDefinition into the strict JSON Schemas that kubeconform reads (09/T3; S19).

kubeconform validates a custom resource against a JSON Schema per kind and version. The authoritative description of a custom
resource is the `openAPIV3Schema` inside its CRD, so the schemas are made from the CRD files of the projects themselves
(CloudNativePG, Gateway API, Argo CD, Config Connector) at a pinned commit, with the same two changes that kubeconform's own
`openapi2jsonschema.py` makes:

- an object that lists `properties` refuses any other field (`additionalProperties: false`), which is what `kubectl` does and
  what `kubeconform -strict` is for; maps (`additionalProperties` already set) and objects marked
  `x-kubernetes-preserve-unknown-fields` stay open;
- `x-kubernetes-int-or-string` becomes a `oneOf` of string and integer.

The walk follows the JSON Schema keywords, not every key, so a field that happens to be called `properties` is a field.
Used by fetch.py on a developer machine; CI never converts anything, it only checks the committed files against the lock
(verify.mjs).
"""

import copy

# Keywords whose value is one schema, a list of schemas, or a map of name to schema.
_ONE = ("items", "additionalProperties", "not")
_LIST = ("allOf", "anyOf", "oneOf")
_MAP = ("properties",)


def _walk(node, strict=True):
    """Return a converted copy of one schema node.

    `strict` is true where the node describes the shape of the resource itself (the root, `properties`, `items`, a map's values).
    Inside `allOf`, `anyOf`, `oneOf` and `not` a schema only checks a value, it does not say which fields exist (Gateway API's
    `addresses` has `oneOf` branches that each name one property and leave the others to the parent), so nothing below them is
    closed: closing a branch would reject the fields it does not mention.
    """
    if not isinstance(node, dict):
        return node
    out = dict(node)
    for key in _MAP:
        if isinstance(out.get(key), dict):
            out[key] = {name: _walk(sub, strict) for name, sub in out[key].items()}
    for key in _ONE:
        if isinstance(out.get(key), dict):
            out[key] = _walk(out[key], strict and key != "not")
    if isinstance(out.get("items"), list):
        out["items"] = [_walk(sub, strict) for sub in out["items"]]
    for key in _LIST:
        if isinstance(out.get(key), list):
            out[key] = [_walk(sub, False) for sub in out[key]]
    if out.pop("x-kubernetes-int-or-string", False) is True:
        out["oneOf"] = [{"type": "string"}, {"type": "integer"}]
    if (
        strict
        and "properties" in out
        and "additionalProperties" not in out
        and out.get("x-kubernetes-preserve-unknown-fields") is not True
    ):
        out["additionalProperties"] = False
    return out


def schema_path(group, kind, version):
    """Where kubeconform looks: `<group>/<lower-case kind>_<version>.json` under the schema root."""
    return f"{group}/{kind.lower()}_{version}.json"


def convert_crd(crd):
    """The `(group, lower-case kind, version, schema)` of every version of a CRD that has a schema. Not a CRD: nothing."""
    if not isinstance(crd, dict) or crd.get("kind") != "CustomResourceDefinition":
        return []
    spec = crd["spec"]
    found = []
    for version in spec.get("versions") or []:
        schema = (version.get("schema") or {}).get("openAPIV3Schema")
        if schema is None:
            continue
        found.append((spec["group"], spec["names"]["kind"].lower(), version["name"], _walk(copy.deepcopy(schema))))
    return found
