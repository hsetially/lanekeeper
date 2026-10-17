//! The schema walker of `support/openapi.rs` (plan A43), tested on its own: what it accepts, and a negative control for every rule it
//! enforces, so that a green contract test means something. Then the real spec: every reference resolves and every pattern compiles.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use serde_json::{Value, json};
use support::openapi::Spec;

fn spec_with(schemas: &Value) -> Spec {
    Spec::from_value(json!({"components": {"schemas": schemas}, "paths": {}}))
}

fn violations(schemas: &Value, schema: &Value, instance: &Value) -> Vec<String> {
    spec_with(schemas).check(schema, instance)
}

#[track_caller]
fn rejects(schema: &Value, instance: &Value, needle: &str) {
    let found = violations(&json!({}), schema, instance);
    assert!(
        found.iter().any(|v| v.contains(needle)),
        "expected a violation containing `{needle}`, found {found:?}"
    );
}

#[track_caller]
fn accepts(schema: &Value, instance: &Value) {
    let found = violations(&json!({}), schema, instance);
    assert!(found.is_empty(), "expected none, found {found:?}");
}

#[test]
fn types_are_checked_and_integers_are_whole_numbers() {
    accepts(&json!({"type": "string"}), &json!("a"));
    rejects(&json!({"type": "string"}), &json!(1), "expected");
    accepts(&json!({"type": "integer"}), &json!(3));
    rejects(&json!({"type": "integer"}), &json!(3.5), "expected");
    rejects(&json!({"type": "integer"}), &json!("3"), "expected");
    accepts(&json!({"type": "number"}), &json!(3.5));
    accepts(&json!({"type": "boolean"}), &json!(false));
    rejects(&json!({"type": "boolean"}), &json!("false"), "expected");
    accepts(&json!({"type": "null"}), &json!(null));
    rejects(&json!({"type": "null"}), &json!(0), "expected");
    accepts(&json!({"type": "array"}), &json!([]));
    rejects(&json!({"type": "array"}), &json!({}), "expected");
    accepts(&json!({"type": "object"}), &json!({}));
    rejects(&json!({"type": "object"}), &json!([]), "expected");
    rejects(
        &json!({"type": "integer", "format": "int64"}),
        &json!(1.5),
        "expected",
    );
}

#[test]
fn required_properties_and_additional_properties_are_enforced() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"type": "integer"}, "b": {"type": "string"}},
        "required": ["a"],
        "additionalProperties": false
    });
    accepts(&schema.clone(), &json!({"a": 1}));
    accepts(&schema.clone(), &json!({"a": 1, "b": "x"}));
    rejects(
        &schema.clone(),
        &json!({"b": "x"}),
        "required property `a` is missing",
    );
    rejects(
        &schema.clone(),
        &json!({"a": 1, "c": 1}),
        "property `c` is not in the schema",
    );
    rejects(&schema.clone(), &json!({"a": "1"}), "/a: expected");
    // Without `additionalProperties`, anything else is allowed; with a schema, it is checked.
    accepts(&json!({"type": "object", "properties": {}}), &json!({"x": 1}));
    let typed = json!({"type": "object", "additionalProperties": {"type": "integer"}});
    accepts(&typed.clone(), &json!({"x": 1}));
    rejects(&typed, &json!({"x": "1"}), "/x: expected");
    rejects(
        &json!({"type": "object", "minProperties": 1}),
        &json!({}),
        "fewer than 1 properties",
    );
}

#[test]
fn enum_const_and_nullable_one_of() {
    accepts(&json!({"enum": ["a", "b"]}), &json!("a"));
    rejects(
        &json!({"enum": ["a", "b"]}),
        &json!("c"),
        "not one of the enum values",
    );
    accepts(&json!({"const": "x"}), &json!("x"));
    rejects(&json!({"const": "x"}), &json!("y"), "not the constant");
    let nullable = json!({"oneOf": [{"type": "string"}, {"type": "null"}]});
    accepts(&nullable.clone(), &json!("x"));
    accepts(&nullable.clone(), &json!(null));
    rejects(&nullable, &json!(1), "0 of 2 oneOf branches match");
}

#[test]
fn one_of_needs_exactly_one_branch() {
    let both = json!({"oneOf": [{"type": "integer"}, {"type": "number"}]});
    rejects(&both, &json!(1), "2 of 2 oneOf branches match");
    let tagged = json!({"oneOf": [
        {"type": "object", "properties": {"kind": {"const": "a"}}, "required": ["kind"], "additionalProperties": false},
        {"type": "object", "properties": {"kind": {"const": "b"}, "x": {"type": "integer"}}, "required": ["kind"], "additionalProperties": false}
    ]});
    accepts(&tagged.clone(), &json!({"kind": "a"}));
    accepts(&tagged.clone(), &json!({"kind": "b", "x": 1}));
    rejects(&tagged.clone(), &json!({"kind": "c"}), "0 of 2 oneOf");
    rejects(&tagged, &json!({"kind": "a", "x": 1}), "0 of 2 oneOf");
    rejects(
        &json!({"anyOf": [{"type": "integer"}, {"type": "string"}]}),
        &json!(true),
        "none of the anyOf",
    );
    accepts(
        &json!({"anyOf": [{"type": "integer"}, {"type": "string"}]}),
        &json!("x"),
    );
}

#[test]
fn strings_numbers_and_arrays_have_their_bounds() {
    rejects(
        &json!({"type": "string", "minLength": 2}),
        &json!("a"),
        "shorter than 2",
    );
    rejects(
        &json!({"type": "string", "maxLength": 2}),
        &json!("abc"),
        "longer than 2",
    );
    accepts(&json!({"type": "string", "maxLength": 2}), &json!("éé"));
    rejects(
        &json!({"type": "string", "maxLength": 2}),
        &json!("ééé"),
        "longer than 2",
    );
    rejects(
        &json!({"type": "string", "pattern": "^[a-z]+$"}),
        &json!("Abc"),
        "does not match",
    );
    accepts(&json!({"type": "string", "pattern": "^[a-z]+$"}), &json!("abc"));
    rejects(
        &json!({"type": "string", "pattern": "^[a-z]+$"}),
        &json!("abc\n"),
        "does not match",
    );
    rejects(&json!({"type": "integer", "minimum": 1}), &json!(0), "below 1");
    rejects(&json!({"type": "integer", "maximum": 5}), &json!(6), "above 5");
    accepts(&json!({"type": "integer", "minimum": 1, "maximum": 5}), &json!(5));
    rejects(
        &json!({"type": "array", "minItems": 2}),
        &json!([1]),
        "fewer than 2 items",
    );
    rejects(
        &json!({"type": "array", "maxItems": 1}),
        &json!([1, 2]),
        "more than 1 items",
    );
    rejects(
        &json!({"type": "array", "uniqueItems": true}),
        &json!([1, 1]),
        "not unique",
    );
    accepts(&json!({"type": "array", "uniqueItems": true}), &json!([1, "1"]));
    let items = json!({"type": "array", "items": {"type": "integer"}});
    accepts(&items.clone(), &json!([1, 2]));
    rejects(&items, &json!([1, "2"]), "/1: expected");
    let pair = json!({"type": "array", "prefixItems": [{"type": "string"}, {"type": "integer"}], "minItems": 2, "maxItems": 2});
    accepts(&pair.clone(), &json!(["a", 1]));
    rejects(&pair.clone(), &json!([1, "a"]), "/0: expected");
    rejects(&pair, &json!(["a"]), "fewer than 2 items");
}

#[test]
fn references_are_followed_and_a_dangling_one_is_loud() {
    let schemas = json!({"Id": {"type": "string", "pattern": "^[a-z]+$"}, "Thing": {"type": "object", "properties": {"id": {"$ref": "#/components/schemas/Id"}}, "required": ["id"], "additionalProperties": false}});
    let thing = json!({"$ref": "#/components/schemas/Thing"});
    assert!(violations(&schemas, &thing, &json!({"id": "abc"})).is_empty());
    let found = violations(&schemas, &thing, &json!({"id": "ABC"}));
    assert!(
        found.iter().any(|v| v.starts_with("/id: does not match")),
        "{found:?}"
    );
    let chain = json!({"A": {"$ref": "#/components/schemas/B"}, "B": {"type": "integer"}});
    assert!(violations(&chain, &json!({"$ref": "#/components/schemas/A"}), &json!(1)).is_empty());
    assert!(!violations(&chain, &json!({"$ref": "#/components/schemas/A"}), &json!("1")).is_empty());
}

#[test]
#[should_panic(expected = "dangling reference")]
fn a_dangling_reference_panics_instead_of_passing() {
    let _ = violations(
        &json!({}),
        &json!({"$ref": "#/components/schemas/Nope"}),
        &json!(1),
    );
}

#[test]
#[should_panic(expected = "cycle")]
fn a_reference_cycle_panics_instead_of_looping() {
    let schemas = json!({"A": {"$ref": "#/components/schemas/B"}, "B": {"$ref": "#/components/schemas/A"}});
    let _ = violations(&schemas, &json!({"$ref": "#/components/schemas/A"}), &json!(1));
}

#[test]
fn all_of_is_the_merge_the_spec_means_not_the_literal_intersection() {
    let schemas = json!({
        "Base": {"type": "object", "properties": {"a": {"type": "integer"}}, "required": ["a"], "additionalProperties": false},
        "Open": {"type": "object", "properties": {"a": {"type": "integer"}}, "required": ["a"], "additionalProperties": true},
        "Detail": {"allOf": [
            {"$ref": "#/components/schemas/Base"},
            {"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"], "additionalProperties": false}
        ]},
        "Problem": {"allOf": [
            {"$ref": "#/components/schemas/Open"},
            {"type": "object", "properties": {"current": {"type": "string"}}, "required": ["current"], "additionalProperties": false}
        ]}
    });
    let detail = json!({"$ref": "#/components/schemas/Detail"});
    assert!(
        violations(&schemas, &detail, &json!({"a": 1, "b": "x"})).is_empty(),
        "both members' properties are known"
    );
    let missing = violations(&schemas, &detail, &json!({"a": 1}));
    assert!(
        missing.iter().any(|v| v.contains("`b` is missing")),
        "{missing:?}"
    );
    let extra = violations(&schemas, &detail, &json!({"a": 1, "b": "x", "c": 1}));
    assert!(
        extra.iter().any(|v| v.contains("`c` is not in the schema")),
        "{extra:?}"
    );
    let wrong = violations(&schemas, &detail, &json!({"a": "1", "b": "x"}));
    assert!(wrong.iter().any(|v| v.starts_with("/a: expected")), "{wrong:?}");
    // One member that allows extra properties opens the merge, and the required names of both still apply.
    let problem = json!({"$ref": "#/components/schemas/Problem"});
    assert!(
        violations(
            &schemas,
            &problem,
            &json!({"a": 1, "current": "h", "request_id": "r"})
        )
        .is_empty()
    );
    assert!(!violations(&schemas, &problem, &json!({"a": 1})).is_empty());
}

#[test]
fn response_lookup_follows_references_and_falls_back_to_default() {
    let spec = Spec::from_value(json!({
        "components": {"responses": {"NotFound": {"description": "x", "content": {"application/problem+json": {"schema": {"type": "object"}}}}}},
        "paths": {"/x": {"get": {
            "operationId": "getX",
            "parameters": [{"name": "q", "in": "query", "required": true, "schema": {"type": "string"}}],
            "responses": {
                "200": {"description": "ok", "headers": {"ETag": {"schema": {"type": "string"}}}, "content": {"application/json": {"schema": {"type": "object"}}}},
                "404": {"$ref": "#/components/responses/NotFound"},
                "default": {"description": "other"}
            }}}}
    }));
    let op = spec.operation("GET", "/x").unwrap();
    assert!(spec.names_status(op, 404) && !spec.names_status(op, 503));
    assert_eq!(
        spec.media_types(spec.response(op, 404).unwrap()),
        ["application/problem+json"]
    );
    assert!(
        spec.media_types(spec.response(op, 503).unwrap()).is_empty(),
        "the default has no body"
    );
    assert_eq!(spec.header_names(spec.response(op, 200).unwrap()), ["etag"]);
    let params = spec.parameters(op);
    assert_eq!(
        (
            params[0].name.as_str(),
            params[0].location.as_str(),
            params[0].required
        ),
        ("q", "query", true)
    );
    assert!(spec.operation("POST", "/x").is_none());
}

// ------------------------------------------------------------------------------------------------ the real spec

fn all_patterns(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "pattern" {
                    if let Some(p) = value.as_str() {
                        out.push(p.to_owned());
                    }
                }
                all_patterns(value, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| all_patterns(i, out)),
        _ => {}
    }
}

fn all_refs(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "$ref" {
                    if let Some(r) = value.as_str() {
                        out.push(r.to_owned());
                    }
                }
                all_refs(value, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| all_refs(i, out)),
        _ => {}
    }
}

#[test]
fn every_reference_of_the_real_spec_resolves_and_every_pattern_compiles() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/openapi.yaml"),
    )
    .unwrap();
    let doc: Value = serde_saphyr::from_str(&text).unwrap();
    let mut refs = Vec::new();
    all_refs(&doc, &mut refs);
    assert!(refs.len() > 300, "{}", refs.len());
    for r in &refs {
        let pointer = r.strip_prefix('#').unwrap();
        assert!(doc.pointer(pointer).is_some(), "{r} does not resolve");
    }
    let mut patterns = Vec::new();
    all_patterns(&doc, &mut patterns);
    assert!(patterns.len() >= 10, "{}", patterns.len());
    for p in &patterns {
        grep_regex::RegexMatcher::new(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
}

#[test]
fn the_real_spec_schemas_say_what_the_tests_rely_on() {
    let spec = Spec::load();
    // Strictness of the shapes the contract test walks: a planted extra property is found.
    let summary = json!({
        "id": "sit1", "display_name": "sit1", "tenants": ["sit1"], "agent": "disconnected", "last_scan_at": null,
        "base_version": null, "drift_counts": [], "finding_count": 0
    });
    let ok = spec.check(spec.schema("SwimlaneSummary"), &summary);
    assert!(ok.is_empty(), "{ok:?}");
    let mut extra = summary.clone();
    extra["surprise"] = json!(1);
    assert!(!spec.check(spec.schema("SwimlaneSummary"), &extra).is_empty());
    let mut wrong = summary.clone();
    wrong["finding_count"] = json!("0");
    assert!(!spec.check(spec.schema("SwimlaneSummary"), &wrong).is_empty());
    let mut missing = summary;
    missing.as_object_mut().unwrap().remove("agent");
    assert!(!spec.check(spec.schema("SwimlaneSummary"), &missing).is_empty());
    // A Problem is open for extra members, and needs `type`, `title` and `status`.
    assert!(
        spec.check(
            spec.schema("Problem"),
            &json!({"type": "urn:x", "title": "t", "status": 404, "code": "not_found"})
        )
        .is_empty()
    );
    assert!(
        !spec
            .check(spec.schema("Problem"), &json!({"type": "urn:x", "title": "t"}))
            .is_empty()
    );
    assert!(
        !spec
            .check(
                spec.schema("Problem"),
                &json!({"type": "urn:x", "title": "t", "status": 200})
            )
            .is_empty()
    );
}
