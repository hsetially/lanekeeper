//! S11: the arguments of every tool are typed, bounded and strict, and they agree with the schemas of the contract.
//!
//! The property that matters (`typed_args_never_accept_what_the_schema_rejects_prop`): nothing the typed parser accepts is rejected
//! by the document's schema, checked with a validator that is not ours (`jsonschema`). The parser may be stricter than the schema.

use hub_mcp::schemas::Contract;
use hub_mcp::tools::args::{ArgError, ToolArgs, ToolName, parse_args};
use jsonschema::Validator;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// One valid call for each tool, with every optional argument it has. The same table drives the matrix tests.
pub fn full_examples() -> Vec<(ToolName, Value)> {
    vec![
        (ToolName::ListSwimlanes, json!({"cursor": "abc", "limit": 20})),
        (ToolName::GetSwimlane, json!({"swimlane": "sit1"})),
        (
            ToolName::ListFiles,
            json!({"swimlane": "sit1", "prefix": "tx-infinity-api", "drift": "nfs_ahead", "cursor": "abc", "limit": 200}),
        ),
        (
            ToolName::ReadFile,
            json!({"swimlane": "sit1", "path": "a/b-sit1.yml", "source": "git", "repo": "tenant", "ref": "v1.0.0", "lines": {"start": 1, "end": 10}}),
        ),
        (
            ToolName::GetEffectiveConfig,
            json!({"swimlane": "sit1", "file": "a/b.yml", "tenant": "sit1", "channel": "remote-itm", "setting_prefix": "a.b"}),
        ),
        (
            ToolName::GetServedConfig,
            json!({"swimlane": "sit1", "application": "tx-infinity-api", "tenant": "sit1", "channel": "remote-itm", "file": "tx-infinity-core.yml"}),
        ),
        (
            ToolName::Compare,
            json!({"left": "nfs:sit1", "right": "git:base@v1.0.0", "file": "a/b.yml", "channel": "x", "detail": "settings"}),
        ),
        (
            ToolName::CompareTree,
            json!({"left": "baseline:sit1", "right": "effective:sit2", "prefix": "a", "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::CompareGrid,
            json!({"file": "a/b.yml", "swimlanes": ["sit1", "sit2"], "channel": "x", "only_differing": false, "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::SearchSettings,
            json!({"q": "timeout", "swimlanes": ["sit1"], "file": "a/b.yml", "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::SearchText,
            json!({"pattern": "foo", "regex": true, "case_sensitive": true, "swimlanes": ["sit1"], "path_glob": "*.yml", "max_total": 500, "max_per_file": 100}),
        ),
        (
            ToolName::FindInconsistencies,
            json!({"swimlane": "sit1", "kinds": ["drift", "orphan_file"], "min_severity": "high", "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::GetDrift,
            json!({"swimlane": "sit1", "state": "conflict", "prefix": "a", "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::GetPendingRestarts,
            json!({"swimlane": "sit1", "cursor": "abc", "limit": 5}),
        ),
        (
            ToolName::GetHistory,
            json!({"swimlane": "sit1", "path": "a/b.yml", "cursor": "abc", "limit": 5}),
        ),
        (ToolName::SearchDocs, json!({"q": "feature flags", "limit": 50})),
        (
            ToolName::ReadDoc,
            json!({"path": "/git/main/readme.md", "lines": {"start": 3, "end": 9}}),
        ),
        (
            ToolName::GrepDocs,
            json!({"pattern": "foo", "regex": false, "context": 5, "path_prefix": "/uploads/x", "limit": 100}),
        ),
        (
            ToolName::GetDocsForFile,
            json!({"file": "a/b.yml", "setting": "a.b", "limit": 50}),
        ),
        (
            ToolName::GetFeatureStatus,
            json!({"flag": "atm", "file": "a/b.yml", "channel": "x", "swimlanes": ["sit1"]}),
        ),
        (
            ToolName::ProposeChange,
            json!({"swimlane": "sit1", "path": "a/b.yml", "expected": {"state": "hash", "hash": HASH}, "content": "a: 1\n", "change_eol": "crlf"}),
        ),
        (ToolName::ApplyChange, json!({"draft_id": "d-1"})),
        (
            ToolName::UploadFile,
            json!({"swimlane": "sit1", "path": "a/b.yml", "expected": {"state": "absent"}, "content": "x"}),
        ),
        (
            ToolName::DeleteFile,
            json!({"swimlane": "sit1", "path": "a/b.yml", "expected_hash": HASH}),
        ),
        (
            ToolName::RaisePr,
            json!({"swimlane": "sit1", "paths": ["a/b.yml"], "title": "Fix", "body": null, "target": {"kind": "tenant", "branch": "sit1"}}),
        ),
        (
            ToolName::RestartService,
            json!({"swimlane": "sit1", "service": {"namespace": "apps", "name": "tx-core"}}),
        ),
        (
            ToolName::NotifyServices,
            json!({"swimlane": "sit1", "paths": ["a/b.yml", "c.yml"]}),
        ),
    ]
}

/// Only the required arguments of each tool.
pub fn minimal_examples() -> Vec<(ToolName, Value)> {
    let contract = Contract::embedded().unwrap();
    full_examples()
        .into_iter()
        .map(|(tool, full)| {
            let spec = contract.tool(tool.as_str()).unwrap();
            let required: Vec<&str> = spec
                .input()
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let kept: Map<String, Value> = full
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| required.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (tool, Value::Object(kept))
        })
        .collect()
}

fn validator(tool: ToolName) -> Validator {
    let contract = Contract::embedded().unwrap();
    let schema = Value::Object((**contract.tool(tool.as_str()).unwrap().input()).clone());
    jsonschema::validator_for(&schema).expect("the tool's input schema compiles")
}

fn map(value: &Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}

#[test]
fn the_examples_cover_every_tool_exactly_once() {
    let names: Vec<ToolName> = full_examples().into_iter().map(|(t, _)| t).collect();
    assert_eq!(
        names,
        ToolName::ALL,
        "one example per tool, in the document's order"
    );
}

#[test]
fn valid_examples_deserialize_into_typed_args() {
    for (tool, example) in full_examples().into_iter().chain(minimal_examples()) {
        let schema = validator(tool);
        assert!(
            schema.is_valid(&example),
            "{tool}: the example is not valid for the schema: {example}"
        );
        let parsed =
            parse_args(tool, map(&example)).unwrap_or_else(|e| panic!("{tool}: {e:?} for {example}"));
        assert_eq!(parsed.tool(), tool);
    }
}

#[test]
fn every_tool_name_round_trips_and_the_table_is_the_documents() {
    let contract = Contract::embedded().unwrap();
    let from_table: Vec<&str> = ToolName::ALL.iter().map(|t| t.as_str()).collect();
    let from_doc: Vec<&str> = contract.tools().iter().map(|t| t.name()).collect();
    assert_eq!(from_table, from_doc);
    for tool in ToolName::ALL {
        assert_eq!(ToolName::from_name(tool.as_str()), Some(*tool));
    }
    assert_eq!(ToolName::from_name("rm_rf"), None);
    assert_eq!(ToolName::from_name(""), None);
    assert_eq!(ToolName::from_name("LIST_SWIMLANES"), None);
}

#[test]
fn the_declared_arguments_are_the_schemas_properties() {
    // `parse_args` refuses an unknown argument from its own table; the table must be the schema's `properties` and `required`.
    let contract = Contract::embedded().unwrap();
    for (tool, full) in full_examples() {
        let spec = contract.tool(tool.as_str()).unwrap();
        let properties: Vec<&str> = spec.input()["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut used: Vec<&str> = full.as_object().unwrap().keys().map(String::as_str).collect();
        if tool == ToolName::ReadFile {
            // `lines` and `setting_path` exclude each other, so the full example has `lines` and `setting_path` is checked below.
            used.push("setting_path");
        }
        used.sort_unstable();
        let mut expected = properties.clone();
        expected.sort_unstable();
        assert_eq!(
            used, expected,
            "{tool}: the full example uses every property, and only them"
        );
        // Dropping a required argument is reported by name, and the name is the tool's own.
        for required in spec.input()["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
        {
            let mut args = map(&full).unwrap();
            args.remove(required);
            match parse_args(tool, Some(args)) {
                Err(ArgError::Missing(name)) => assert_eq!(name, required, "{tool}"),
                other => panic!("{tool} without {required}: {other:?}"),
            }
        }
    }
}

#[test]
fn unknown_field_is_invalid_argument() {
    for (tool, minimal) in minimal_examples() {
        let mut args = map(&minimal).unwrap();
        args.insert("surprise".to_owned(), json!(1));
        assert_eq!(parse_args(tool, Some(args)), Err(ArgError::Invalid), "{tool}");
    }
}

#[test]
fn a_null_for_an_optional_argument_is_refused() {
    // The schemas give none of these fields a `null`, so the parser must not either (`raise_pr.body` is the one that says so).
    for (tool, full) in full_examples() {
        let required: Vec<String> =
            Contract::embedded().unwrap().tool(tool.as_str()).unwrap().input()["required"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
        for key in full.as_object().unwrap().keys() {
            if required.contains(key) || (tool == ToolName::RaisePr && key == "body") {
                continue;
            }
            let mut args = map(&full).unwrap();
            args.insert(key.clone(), Value::Null);
            assert_eq!(
                parse_args(tool, Some(args)),
                Err(ArgError::Invalid),
                "{tool}.{key} = null"
            );
        }
    }
    let mut args = map(&full_examples()
        .into_iter()
        .find(|(t, _)| *t == ToolName::RaisePr)
        .unwrap()
        .1)
    .unwrap();
    args.insert("body".to_owned(), Value::Null);
    assert!(
        parse_args(ToolName::RaisePr, Some(args)).is_ok(),
        "a PR body may be null"
    );
}

#[test]
fn content_over_2_mib_of_bytes_is_payload_too_large() {
    let at_limit = "a".repeat(2 * 1024 * 1024);
    let over = format!("{at_limit}a");
    for tool in [ToolName::ProposeChange, ToolName::UploadFile] {
        let mut base = full_examples().into_iter().find(|(t, _)| *t == tool).unwrap().1;
        base["content"] = json!(at_limit);
        assert!(
            parse_args(tool, map(&base)).is_ok(),
            "{tool}: exactly 2 MiB is allowed"
        );
        base["content"] = json!(over);
        assert_eq!(parse_args(tool, map(&base)), Err(ArgError::TooLarge), "{tool}");
    }
    // Counted in bytes, not characters: 700,000 characters of three bytes each are over the limit though the schema allows them.
    let wide = "\u{20ac}".repeat(700_000);
    let mut base = full_examples()
        .into_iter()
        .find(|(t, _)| *t == ToolName::UploadFile)
        .unwrap()
        .1;
    base["content"] = json!(wide);
    assert_eq!(
        parse_args(ToolName::UploadFile, map(&base)),
        Err(ArgError::TooLarge)
    );
}

#[test]
fn pattern_over_512_bytes_is_invalid_argument() {
    for tool in [ToolName::SearchText, ToolName::GrepDocs] {
        let mut base = full_examples().into_iter().find(|(t, _)| *t == tool).unwrap().1;
        base["pattern"] = json!("a".repeat(512));
        assert!(parse_args(tool, map(&base)).is_ok(), "{tool}");
        base["pattern"] = json!("a".repeat(513));
        assert_eq!(parse_args(tool, map(&base)), Err(ArgError::Invalid), "{tool}");
        base["pattern"] = json!("");
        assert_eq!(
            parse_args(tool, map(&base)),
            Err(ArgError::Invalid),
            "{tool}: empty"
        );
    }
    let mut base = json!({"q": "a".repeat(512)});
    assert!(parse_args(ToolName::SearchDocs, map(&base)).is_ok());
    base["q"] = json!("a".repeat(513));
    assert_eq!(
        parse_args(ToolName::SearchDocs, map(&base)),
        Err(ArgError::Invalid)
    );
}

#[test]
fn swimlane_lists_over_64_are_invalid_argument() {
    let lanes = |n: usize| (0..n).map(|i| format!("sit{i}")).collect::<Vec<_>>();
    for (tool, key, base) in [
        (ToolName::CompareGrid, "swimlanes", json!({"file": "a.yml"})),
        (ToolName::SearchSettings, "swimlanes", json!({"q": "x"})),
        (ToolName::SearchText, "swimlanes", json!({"pattern": "x"})),
        (ToolName::GetFeatureStatus, "swimlanes", json!({})),
    ] {
        let mut args = map(&base).unwrap();
        args.insert(key.to_owned(), json!(lanes(64)));
        assert!(
            parse_args(tool, Some(args.clone())).is_ok(),
            "{tool}: 64 are allowed"
        );
        args.insert(key.to_owned(), json!(lanes(65)));
        assert_eq!(
            parse_args(tool, Some(args.clone())),
            Err(ArgError::Invalid),
            "{tool}: 65"
        );
        args.insert(key.to_owned(), json!(["sit1", "sit1"]));
        assert_eq!(
            parse_args(tool, Some(args)),
            Err(ArgError::Invalid),
            "{tool}: duplicates"
        );
    }
    // A grid needs at least one.
    assert_eq!(
        parse_args(
            ToolName::CompareGrid,
            map(&json!({"file": "a.yml", "swimlanes": []}))
        ),
        Err(ArgError::Invalid)
    );
    // A PR or a notification lists 1 to 100 paths, unique after normalisation.
    let paths = |n: usize| (0..n).map(|i| format!("f{i}.yml")).collect::<Vec<_>>();
    for tool in [ToolName::RaisePr, ToolName::NotifyServices] {
        let base = |p: Vec<String>| {
            let mut m = json!({"swimlane": "sit1", "paths": p});
            if tool == ToolName::RaisePr {
                m["title"] = json!("t");
            }
            map(&m)
        };
        assert!(parse_args(tool, base(paths(100))).is_ok(), "{tool}");
        assert_eq!(
            parse_args(tool, base(paths(101))),
            Err(ArgError::Invalid),
            "{tool}"
        );
        assert_eq!(parse_args(tool, base(vec![])), Err(ArgError::Invalid), "{tool}");
        assert_eq!(
            parse_args(tool, base(vec!["a/b.yml".into(), "a//b.yml".into()])),
            Err(ArgError::Invalid),
            "{tool}: the same file written twice"
        );
    }
}

#[test]
fn every_path_argument_uses_a_domain_parser() {
    // `..`, an absolute path, a backslash and a control character are refused for every argument that names a file.
    let hostile = [
        "../etc/passwd",
        "a/../../b",
        "/etc/passwd",
        "a\\b",
        "a\u{0}b",
        "a\nb",
    ];
    for (tool, key) in [
        (ToolName::ListFiles, "prefix"),
        (ToolName::ReadFile, "path"),
        (ToolName::GetServedConfig, "file"),
        (ToolName::CompareTree, "prefix"),
        (ToolName::GetDrift, "prefix"),
        (ToolName::GetHistory, "path"),
        (ToolName::ProposeChange, "path"),
        (ToolName::UploadFile, "path"),
        (ToolName::DeleteFile, "path"),
        (ToolName::GetEffectiveConfig, "file"),
        (ToolName::Compare, "file"),
        (ToolName::CompareGrid, "file"),
        (ToolName::GetDocsForFile, "file"),
    ] {
        let base = full_examples().into_iter().find(|(t, _)| *t == tool).unwrap().1;
        for bad in hostile {
            let mut args = map(&base).unwrap();
            args.insert(key.to_owned(), json!(bad));
            assert_eq!(
                parse_args(tool, Some(args)),
                Err(ArgError::Invalid),
                "{tool}.{key} accepted {bad:?}"
            );
        }
    }
    // The paths of a PR and a notification are checked the same way.
    for tool in [ToolName::RaisePr, ToolName::NotifyServices] {
        let mut base = map(&full_examples().into_iter().find(|(t, _)| *t == tool).unwrap().1).unwrap();
        for bad in hostile {
            base.insert("paths".to_owned(), json!([bad]));
            assert_eq!(
                parse_args(tool, Some(base.clone())),
                Err(ArgError::Invalid),
                "{tool}: {bad:?}"
            );
        }
    }
    // A doc path is a path of the virtual tree only.
    for bad in [
        "/etc/passwd",
        "git/main/x",
        "/git/../x",
        "/git/main/.git/config",
        "/uploads/",
        "/git/a\\b",
    ] {
        assert_eq!(
            parse_args(ToolName::ReadDoc, map(&json!({"path": bad}))),
            Err(ArgError::Invalid),
            "{bad}"
        );
    }
}

#[test]
fn ids_and_hashes_are_held_to_the_schemas_patterns() {
    let upper = HASH.to_uppercase();
    assert_eq!(
        parse_args(
            ToolName::DeleteFile,
            map(&json!({"swimlane": "sit1", "path": "a", "expected_hash": upper}))
        ),
        Err(ArgError::Invalid),
        "the schema's hash is lower case only"
    );
    for bad in ["-x", ".x", "x y", "x/y", ""] {
        assert_eq!(
            parse_args(
                ToolName::GetEffectiveConfig,
                map(&json!({"swimlane": "sit1", "file": "a", "tenant": "sit1", "channel": bad}))
            ),
            Err(ArgError::Invalid),
            "channel {bad:?}"
        );
    }
    assert!(parse_args(ToolName::GetSwimlane, map(&json!({"swimlane": "Sit1"}))).is_err());
    assert!(parse_args(ToolName::GetSwimlane, map(&json!({"swimlane": "a".repeat(64)}))).is_err());
    assert!(parse_args(ToolName::GetSwimlane, map(&json!({"swimlane": 5}))).is_err());
    assert!(parse_args(ToolName::ListSwimlanes, map(&json!({"limit": 0}))).is_err());
    assert!(parse_args(ToolName::ListSwimlanes, map(&json!({"limit": 201}))).is_err());
    assert!(parse_args(ToolName::ListSwimlanes, map(&json!({"limit": 1.5}))).is_err());
    assert!(parse_args(ToolName::ListSwimlanes, map(&json!({"cursor": "has space"}))).is_err());
    assert!(
        parse_args(
            ToolName::ReadFile,
            map(&json!({"swimlane": "s", "path": "a", "lines": {"start": 1, "end": 2, "x": 3}}))
        )
        .is_err()
    );
    assert!(
        parse_args(
            ToolName::ReadFile,
            map(&json!({"swimlane": "s", "path": "a", "lines": {"start": 5, "end": 2}}))
        )
        .is_err()
    );
    assert!(
        parse_args(
            ToolName::ReadFile,
            map(&json!({"swimlane": "s", "path": "a", "lines": {"start": 1, "end": 10_000_001}}))
        )
        .is_err()
    );
    assert!(
        parse_args(
            ToolName::UploadFile,
            map(&json!({"swimlane": "s", "path": "a", "expected": {"state": "absent", "hash": HASH}, "content": ""}))
        )
        .is_err(),
        "an absent file has no hash"
    );
    for bad in [
        json!({"kind": "base", "branch": "sit1"}),
        json!({"kind": "tenant"}),
        json!({"kind": "tenant", "branch": "Sit1"}),
        json!({"kind": "other"}),
    ] {
        assert!(
            parse_args(
                ToolName::RaisePr,
                map(&json!({"swimlane": "s", "paths": ["a"], "title": "t", "target": bad}))
            )
            .is_err(),
            "{bad}"
        );
    }
    assert!(
        parse_args(
            ToolName::ReadFile,
            map(&json!({"swimlane": "s", "path": "a", "setting_path": "a.b"}))
        )
        .is_ok(),
        "a setting path alone"
    );
}

#[test]
fn read_file_with_lines_and_setting_path_together_is_refused_at_the_schema_and_left_to_the_handler() {
    // The schema says `not: {required: [lines, setting_path]}`. The parser accepts the pair (the handler refuses it with a message
    // of its own), which is the one place where the parser is looser than the schema, so the property test excludes it by name.
    let schema = validator(ToolName::ReadFile);
    let both = json!({"swimlane": "s", "path": "a", "lines": {"start": 1, "end": 2}, "setting_path": "a.b"});
    assert!(!schema.is_valid(&both));
    assert!(parse_args(ToolName::ReadFile, map(&both)).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------------------------------------------------

/// Values worth putting in any field: valid, nearly valid and hostile, of every JSON type.
fn pool() -> Vec<Value> {
    let mut values: Vec<Value> = [
        "",
        "a",
        "sit1",
        "prod-1",
        "Prod",
        "-x",
        "_x",
        ".x",
        "x.y",
        "a b",
        "x/..",
        "a//b",
        "a/./b",
        "a/b/",
        "/abs",
        "a\\b",
        "ctl\u{1}",
        "line\nbreak",
        "\u{2028}",
        "\u{20ac}\u{20ac}",
        "nfs:sit1",
        "nfs:Sit",
        "baseline:sit1",
        "effective:a-b",
        "git:base@main",
        "git:tenant@v1.0.0",
        "git:base@",
        "git:other@x",
        "git:base@a b",
        "git:base@a\u{2028}b",
        "in_sync",
        "conflict",
        "nope",
        "yaml",
        "lf",
        "crlf",
        "mixed",
        "git",
        "nfs",
        "base",
        "tenant",
        "settings",
        "auto",
        "lines",
        "high",
        "drift",
        "orphan_file",
        "some text",
        "tx-infinity-api/x.yml",
        "a.b[0]",
        "#2/a.b",
        "/git/main/x.md",
        "/uploads/a.md",
        "/git/",
        "m1.abc",
        "abc",
        "d-1",
        "apps",
        "tx-core",
        "d.1",
        "d 1",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
        "0123456789abcdef",
    ]
    .iter()
    .map(|s| json!(s))
    .collect();
    for n in [
        63, 64, 128, 129, 255, 256, 257, 300, 301, 512, 513, 1024, 1025, 1100,
    ] {
        values.push(json!("a".repeat(n)));
    }
    values.extend([
        json!(0), json!(1), json!(2), json!(5), json!(50), json!(100), json!(101), json!(200), json!(201), json!(500),
        json!(501), json!(-1), json!(1.5), json!(1e2), json!(4_294_967_296_u64), json!(10_000_000), json!(10_000_001),
        json!(true), json!(false), Value::Null, json!([]), json!(["sit1"]), json!(["sit1", "sit1"]), json!(["Sit"]),
        json!(["a/b.yml", "a//b.yml"]), json!(["drift", "drift"]), json!(["drift"]), json!(["../x"]), json!([1]),
        json!({}), json!({"start": 1, "end": 2}), json!({"start": 3, "end": 2}), json!({"start": 0, "end": 2}),
        json!({"start": 1, "end": 2, "x": 1}), json!({"start": 1}), json!({"state": "absent"}),
        json!({"state": "hash", "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}),
        json!({"state": "hash"}), json!({"state": "other"}),
        json!({"state": "absent", "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}),
        json!({"namespace": "apps", "name": "tx-core"}), json!({"namespace": "Apps", "name": "tx-core"}),
        json!({"namespace": "apps", "name": "tx-core", "x": 1}), json!({"namespace": "apps"}),
        json!({"kind": "tenant", "branch": "sit1"}), json!({"kind": "base"}), json!({"kind": "base", "branch": "sit1"}),
        json!({"kind": "tenant"}), json!({"kind": "tenant", "branch": "Sit"}),
    ]);
    values
}

fn interesting_value() -> impl Strategy<Value = Value> {
    proptest::sample::select(pool())
}

#[test]
fn every_pool_value_in_every_field_of_every_tool_agrees_with_the_schema() {
    // The exhaustive form of the property below: each field of each tool's full example, replaced by each value of the pool.
    let mut checked = 0_usize;
    for (tool, base) in full_examples() {
        let schema = validator(tool);
        let base = base.as_object().unwrap();
        for key in base.keys() {
            for value in pool() {
                let mut args = base.clone();
                args.insert(key.clone(), value);
                let instance = Value::Object(args.clone());
                if parse_args(tool, Some(args)).is_ok() {
                    checked += 1;
                    assert!(
                        schema.is_valid(&instance),
                        "{tool}.{key}: the typed parser accepted what the schema rejects: {instance}"
                    );
                }
            }
        }
    }
    assert!(checked > 1_500, "the pool reached only {checked} accepted calls");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn typed_args_never_accept_what_the_schema_rejects_prop(
        index in 0..27_usize,
        mutations in proptest::collection::vec((0..8_usize, interesting_value()), 0..4),
        drops in proptest::collection::vec(0..8_usize, 0..3),
        extra_key in proptest::option::of(prop_oneof![Just("surprise"), Just("lines"), Just("limit"), Just("cursor")]),
    ) {
        let (tool, base) = full_examples().swap_remove(index);
        let mut args = base.as_object().unwrap().clone();
        let keys: Vec<String> = args.keys().cloned().collect();
        for (slot, value) in mutations {
            if let Some(key) = keys.get(slot % keys.len()) {
                args.insert(key.clone(), value);
            }
        }
        for slot in drops {
            if let Some(key) = keys.get(slot % keys.len()) {
                args.remove(key);
            }
        }
        if let Some(key) = extra_key {
            args.entry(key.to_owned()).or_insert(json!(1));
        }
        let instance = Value::Object(args.clone());
        if parse_args(tool, Some(args)).is_ok() {
            // The one place the parser is looser: the schema's `not` for `lines` with `setting_path` (the handler refuses it).
            let both = tool == ToolName::ReadFile
                && instance.get("lines").is_some()
                && instance.get("setting_path").is_some();
            prop_assert!(
                both || validator(tool).is_valid(&instance),
                "{tool}: the typed parser accepted what the schema rejects: {instance}"
            );
        }
    }

    #[test]
    fn arbitrary_json_never_panics_the_parser(
        index in 0..27_usize,
        keys in proptest::collection::vec(prop_oneof![Just("swimlane"), Just("path"), Just("limit"), Just("content"), Just("x")], 0..5),
        values in proptest::collection::vec(interesting_value(), 5),
    ) {
        let tool = ToolName::ALL[index];
        let args: Map<String, Value> = keys.iter().zip(values).map(|(k, v)| ((*k).to_owned(), v)).collect();
        let _ = parse_args(tool, Some(args));
        let _ = parse_args(tool, None);
    }
}

#[test]
fn parsed_arguments_hold_domain_types_not_strings() {
    // A parsed path is the normalised path, which is what the ports get (S11).
    let parsed = parse_args(
        ToolName::ReadFile,
        map(&json!({"swimlane": "sit1", "path": "a//b/./c.yml"})),
    )
    .unwrap();
    let ToolArgs::ReadFile(args) = parsed else {
        panic!()
    };
    assert_eq!(args.path.0.as_str(), "a/b/c.yml");
}
