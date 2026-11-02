#!/usr/bin/env python3
"""Extract the response shapes that FakeJira must match.

Usage: extract-jira-schemas.py <swagger-v3.json> [<output>]

The script reads the Jira Cloud REST v3 OpenAPI spec. It keeps the property
names and the JSON types of the response schemas in SCHEMAS. It keeps no
descriptions and no examples. It writes
crates/autumn-jira-fake/tests/fixtures/schemas.json, or <output>.

The test `fake_replies_match_the_spec_shapes` reads the output. A weekly CI
step runs this script on the current spec and fails on a diff.

Get the spec from
https://developer.atlassian.com/cloud/jira/platform/swagger-v3.v3.json
"""

import json
import os
import sys

DEFAULT_OUT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "crates",
    "autumn-jira-fake",
    "tests",
    "fixtures",
    "schemas.json",
)

# The response schemas of the endpoints that FakeJira serves, and the
# schemas that they refer to. Keep the list sorted.
SCHEMAS = [
    "AvatarUrlsBean",
    "Comment",
    "CreatedIssue",
    "EntityProperty",
    "ErrorCollection",
    "IssueBean",
    "IssueTransition",
    "IssueTypeDetails",
    "IssueTypeIssueCreateMetadata",
    "JQLCountResultsBean",
    "NestedResponse",
    "PageBeanProject",
    "PageOfComments",
    "PageOfCreateMetaIssueTypes",
    "Project",
    "SearchAndReconcileResults",
    "ServerInformation",
    "StatusCategory",
    "StatusDetails",
    "Transitions",
    "User",
    "UserDetails",
    "Visibility",
]

REF_PREFIX = "#/components/schemas/"


def ref_name(prop):
    """Returns the schema name that a property refers to, or None."""
    if "$ref" in prop:
        return prop["$ref"][len(REF_PREFIX):]
    for key in ("allOf", "oneOf", "anyOf"):
        parts = prop.get(key) or []
        if len(parts) == 1 and "$ref" in parts[0]:
            return parts[0]["$ref"][len(REF_PREFIX):]
    return None


def schema_type(schema):
    """Returns the JSON type of a schema object."""
    if "type" in schema:
        return schema["type"]
    if "properties" in schema or "additionalProperties" in schema:
        return "object"
    return "any"


def shape(prop, schemas):
    """Returns the type entry of one property: type, ref, items."""
    name = ref_name(prop)
    if name is not None:
        target = schemas.get(name, {})
        entry = {"type": schema_type(target)}
        if name in SCHEMAS:
            entry["ref"] = name
    else:
        entry = {"type": schema_type(prop)}
    if entry["type"] == "array":
        items = prop.get("items")
        if items is None and name is not None:
            items = schemas.get(name, {}).get("items")
        if isinstance(items, dict):
            entry["items"] = shape(items, schemas)
    if prop.get("nullable"):
        entry["nullable"] = True
    return entry


def extract(spec):
    """Returns the fixture object for a parsed spec."""
    schemas = spec["components"]["schemas"]
    out = {}
    for name in SCHEMAS:
        if name not in schemas:
            raise SystemExit(f"the spec has no schema {name}")
        props = schemas[name].get("properties", {})
        out[name] = {key: shape(props[key], schemas) for key in sorted(props)}
    return {
        "generator": "scripts/extract-jira-schemas.py",
        "source": "Jira Cloud REST v3 OpenAPI spec (swagger-v3.v3.json)",
        "schemas": out,
    }


def main():
    # The spec path has no default: a local download path works on one
    # machine only.
    if not 2 <= len(sys.argv) <= 3 or sys.argv[1] in ("-h", "--help"):
        print(__doc__, file=sys.stderr)
        return 2
    spec_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_OUT
    with open(spec_path, encoding="utf-8") as handle:
        spec = json.load(handle)
    fixture = extract(spec)
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(fixture, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
