//! contract/08-web, `CG10`: the branches and tags of the mirrored repositories, so that the web app can offer a ref instead of
//! a free-text box. The operation is a bounded keyset list over what `GitReader::branches` and `tags` already return, and its
//! `branch_kind` is the domain's `BranchKind`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use domain::BranchKind;
use serde_json::Value;

const SPEC: &str = include_str!("../../../api/openapi.yaml");
const SCHEMA_REF: &str = "#/components/schemas/";

fn doc() -> Value {
    serde_saphyr::from_str(SPEC).expect("api/openapi.yaml is valid YAML")
}

fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let pointer = r.strip_prefix('#').unwrap_or_else(|| panic!("external $ref {r}"));
        cur = doc
            .pointer(pointer)
            .unwrap_or_else(|| panic!("dangling $ref {r}"));
    }
    panic!("$ref chain too deep");
}

fn ref_name(v: &Value) -> &str {
    let r = v["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a $ref, got {v}"));
    r.strip_prefix(SCHEMA_REF)
        .unwrap_or_else(|| panic!("not a schema $ref: {r}"))
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("not an array: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn required_of(schema: &Value) -> BTreeSet<String> {
    schema.get("required").map(strings).unwrap_or_default()
}

fn declared_of(schema: &Value) -> BTreeSet<String> {
    schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no properties: {schema}"))
        .keys()
        .cloned()
        .collect()
}

/// A property schema as one short string, so that a table can say what the document must hold.
fn describe(s: &Value) -> String {
    if s.get("$ref").is_some() {
        return format!("ref:{}", ref_name(s));
    }
    if let Some(arms) = s.get("oneOf").and_then(Value::as_array) {
        return arms.iter().map(describe).collect::<Vec<_>>().join("|");
    }
    match s["type"].as_str() {
        Some("null") => "null".to_owned(),
        Some("array") => format!("array<{}>", describe(&s["items"])),
        Some(other) => other.to_owned(),
        None => panic!("a property with neither $ref, oneOf nor type: {s}"),
    }
}

/// The operation with this id: its method, path and definition.
fn operation<'a>(d: &'a Value, id: &str) -> (String, String, &'a Value) {
    for (path, item) in d["paths"].as_object().expect("paths") {
        for m in ["get", "put", "post", "delete", "patch"] {
            if let Some(op) = item.get(m)
                && op["operationId"] == id
            {
                return (m.to_uppercase(), path.clone(), op);
            }
        }
    }
    panic!("operation {id} is not in the spec");
}

/// A query parameter by name, references resolved.
fn query_param<'a>(d: &'a Value, op: &'a Value, name: &str) -> Option<&'a Value> {
    op["parameters"]
        .as_array()?
        .iter()
        .map(|p| resolve(d, p))
        .find(|p| p["in"] == "query" && p["name"] == name)
}

/// `domain::BranchKind` by exhaustive `match`: a variant added to the domain fails to compile here until the document and this
/// list say what it is.
fn branch_kind_name(k: BranchKind) -> &'static str {
    match k {
        BranchKind::Default => "default",
        BranchKind::Deployable => "deployable",
        BranchKind::Template => "template",
        BranchKind::CommonDocs => "common_docs",
        BranchKind::Other => "other",
    }
}

const BRANCH_KINDS: [BranchKind; 5] = [
    BranchKind::Default,
    BranchKind::Deployable,
    BranchKind::Template,
    BranchKind::CommonDocs,
    BranchKind::Other,
];

/// The list is a bounded keyset list that the conventions test will walk, a Viewer read of names and commit ids, and its
/// `branch_kind` is the domain's `BranchKind`.
#[test]
fn git_refs_are_a_bounded_keyset_list_and_branch_kind_matches_the_domain() {
    let d = doc();
    let (method, path, op) = operation(&d, "listGitRefs");
    assert_eq!((method.as_str(), path.as_str()), ("GET", "/api/v1/git/refs"));
    assert_eq!(op["x-required-role"], "viewer");
    assert!(op["summary"].as_str().is_some_and(|s| !s.is_empty()));

    // A new tag, described (the `tag-description` rule), and the operation uses it.
    assert_eq!(op["tags"], serde_json::json!(["git"]));
    let tag = d["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "git")
        .expect("the `git` tag is declared");
    assert!(
        tag["description"].as_str().is_some_and(|s| s.contains("Refs")),
        "the `git` tag describes the refs of the mirrored repositories"
    );

    // Parameters: the repository is required, everything else narrows.
    let repo = query_param(&d, op, "repo").expect("repo");
    assert_eq!(repo["required"], true);
    assert_eq!(describe(&repo["schema"]), "ref:RepoKind");
    let kind = query_param(&d, op, "kind").expect("kind");
    assert_eq!(kind["required"], false);
    assert_eq!(describe(&kind["schema"]), "ref:GitRefKind");
    let q = query_param(&d, op, "q").expect("q");
    assert_eq!(q["required"], false);
    assert_eq!(q["schema"]["type"], "string");
    assert_eq!(
        q["schema"]["maxLength"], 64,
        "a prefix of at most 64 characters (S11)"
    );
    let q_said = q["description"].as_str().unwrap_or("").to_lowercase();
    assert!(
        q_said.contains("prefix"),
        "q is a prefix, not a pattern: nothing the client sends is a regex (S11)"
    );
}

/// Keyset paging as every list has it: a cursor, a limit of at most 500, and `next` in the body. The same detection the
/// conventions test applies, so the operation is walked there; and the words of the description.
#[test]
fn git_refs_page_by_keyset_and_say_what_they_hold() {
    let d = doc();
    let schemas = &d["components"]["schemas"];
    let (_, _, op) = operation(&d, "listGitRefs");

    // Keyset paging, as every list: a cursor, a limit of at most 500, `next` in the body. The same detection the conventions
    // test applies, so the operation is walked there.
    let cursor = query_param(&d, op, "cursor").expect("cursor");
    assert_eq!(cursor["required"], false);
    let limit = query_param(&d, op, "limit").expect("limit");
    let limit_schema = resolve(&d, &limit["schema"]);
    assert_eq!(limit_schema["minimum"], 1);
    assert!(limit_schema["maximum"].as_u64().is_some_and(|m| m <= 500));
    for banned in ["offset", "page", "page_size", "skip"] {
        assert!(
            query_param(&d, op, banned).is_none(),
            "no offset paging: {banned}"
        );
    }
    let ok = resolve(&d, &op["responses"]["200"]);
    let page = &ok["content"]["application/json"]["schema"];
    assert_eq!(page["$ref"], format!("{SCHEMA_REF}GitRefPage"));
    let page = &schemas["GitRefPage"];
    assert_eq!(
        declared_of(page),
        set(&["items", "next"]),
        "detected as a list: `items` and `next`"
    );
    assert_eq!(required_of(page), declared_of(page));
    assert_eq!(page["additionalProperties"], false);
    assert_eq!(describe(&page["properties"]["items"]), "array<ref:GitRefEntry>");
    assert_eq!(page["properties"]["items"]["maxItems"], 500);
    assert_eq!(describe(&page["properties"]["next"]), "ref:Cursor|null");

    for code in ["200", "400", "401", "403", "default"] {
        assert!(
            op["responses"].get(code).is_some(),
            "listGitRefs documents {code}"
        );
    }
    for (code, r) in op["responses"].as_object().unwrap() {
        if code.starts_with('4') || code == "default" {
            let content = resolve(&d, r)["content"].as_object().unwrap();
            assert_eq!(
                content.keys().collect::<Vec<_>>(),
                ["application/problem+json"],
                "{code}"
            );
        }
    }
    // A read: no write headers, and no audit event.
    for header in ["X-CSRF-Token", "Idempotency-Key"] {
        assert!(
            !op["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| resolve(&d, p)["name"] == header),
            "a read does not take {header}"
        );
    }

    // The words of the description: names only, an order a cursor can follow, what `kind` and `q` do.
    let said = op["description"].as_str().unwrap_or("").to_lowercase();
    for (needle, fact) in [
        (
            "never file contents",
            "that only names and commit ids are returned",
        ),
        ("sorted by `name`", "the order the cursor follows"),
        (
            "branches before tags",
            "which comes first when a branch and a tag share a name",
        ),
        ("both", "that an absent `kind` lists branches and tags"),
        ("not audited", "that a read is not audited"),
    ] {
        assert!(
            said.contains(needle),
            "the description of listGitRefs must state {fact} (`{needle}`)"
        );
    }
}

/// One entry says what a ref is, which commit it points at and, for a branch, how it is used; `BranchKind` is the domain's.
#[test]
fn git_ref_entries_carry_the_domains_branch_kind() {
    let d = doc();
    let schemas = &d["components"]["schemas"];

    // One entry: a name, what it is, the commit it points at, and the use of a branch.
    let entry = &schemas["GitRefEntry"];
    assert_eq!(entry["additionalProperties"], false);
    let fields = [
        ("name", "ref:GitRef"),
        ("kind", "ref:GitRefKind"),
        ("commit", "ref:CommitId"),
        ("branch_kind", "ref:BranchKind|null"),
    ];
    assert_eq!(
        declared_of(entry),
        fields.iter().map(|(k, _)| (*k).to_owned()).collect()
    );
    assert_eq!(
        required_of(entry),
        declared_of(entry),
        "`branch_kind` is null for a tag, never absent"
    );
    for (key, want) in fields {
        assert_eq!(describe(&entry["properties"][key]), want, "GitRefEntry.{key}");
    }
    let tag_null = entry["properties"]["branch_kind"]["description"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        tag_null.contains("null for a tag"),
        "branch_kind says it is null for a tag"
    );

    // `GitRefKind` is what a ref is; `BranchKind` is the domain's, value for value.
    assert_eq!(strings(&schemas["GitRefKind"]["enum"]), set(&["branch", "tag"]));
    assert_eq!(
        strings(&schemas["BranchKind"]["enum"]),
        BRANCH_KINDS
            .iter()
            .map(|k| branch_kind_name(*k).to_owned())
            .collect(),
        "BranchKind is domain::BranchKind"
    );
    for k in BRANCH_KINDS {
        assert_eq!(
            serde_json::to_value(k).unwrap(),
            branch_kind_name(k),
            "the wire name of domain::BranchKind::{k:?}"
        );
    }
    let said = schemas["BranchKind"]["description"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        said.contains("tenant repository"),
        "BranchKind says that it is about the tenant repository's branches"
    );
}

/// Every string and array of the schemas this commit adds has a bound (S11), and each is closed.
#[test]
fn new_schemas_are_bounded() {
    fn walk(at: &str, s: &Value) {
        let Some(obj) = s.as_object() else { return };
        match obj.get("type").and_then(Value::as_str) {
            Some("string") => assert!(
                ["maxLength", "pattern", "enum", "const"]
                    .iter()
                    .any(|k| obj.contains_key(*k)),
                "{at} is an unbounded string"
            ),
            Some("array") => assert!(obj.contains_key("maxItems"), "{at} is an array without maxItems"),
            _ => {}
        }
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (k, v) in props {
                walk(&format!("{at}.{k}"), v);
            }
        }
        for key in ["items", "oneOf"] {
            match obj.get(key) {
                Some(Value::Array(arms)) => arms
                    .iter()
                    .enumerate()
                    .for_each(|(i, a)| walk(&format!("{at}.{key}[{i}]"), a)),
                Some(other) => walk(&format!("{at}.{key}"), other),
                None => {}
            }
        }
    }
    let d = doc();
    for name in ["BranchKind", "GitRefKind", "GitRefEntry", "GitRefPage"] {
        let schema = &d["components"]["schemas"][name];
        assert!(schema.is_object(), "{name} is not in the document");
        walk(name, schema);
        if schema["type"] == "object" {
            assert_eq!(schema["additionalProperties"], false, "{name} is closed");
        }
    }
    // The `q` parameter is bounded where it is declared.
    let (_, _, op) = operation(&d, "listGitRefs");
    assert!(query_param(&d, op, "q").unwrap()["schema"]["maxLength"].is_u64());
}
