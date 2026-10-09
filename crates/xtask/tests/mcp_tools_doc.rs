//! Lint for `docs/mcp-tools.md` (prompt 01, T7; S4, S14, S14b, S15, D52, D58, P9).
//!
//! The document is a contract that prompt 07 implements and a contract test there compares against. This lint keeps
//! it machine-readable and complete: the exact tool set, one section per tool with every field, JSON Schema blocks
//! that parse and resolve their `$ref`s, bounded inputs, the role floor, the confirmation rule and the output caps.
//! It also ties each tool to the REST operation it wraps, so a role cannot drift away from `api/openapi.yaml`.
//!
//! Q22: there is deliberately no MCP tool that notifies the config-server. Its `/update-resources` endpoint is
//! unauthenticated (Q37), so it is not exposed to AI clients. `no_notify_tool_is_exposed` pins that.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{Map, Value};

/// The 19 read tools of prompt 01 T7.
const READ_TOOLS: &[&str] = &[
    "list_swimlanes",
    "get_swimlane",
    "list_files",
    "read_file",
    "get_effective_config",
    "compare",
    "compare_tree",
    "compare_grid",
    "search_settings",
    "search_text",
    "find_inconsistencies",
    "get_drift",
    "get_pending_restarts",
    "get_history",
    "search_docs",
    "read_doc",
    "grep_docs",
    "get_docs_for_file",
    "get_feature_status",
];

/// The six write tools, with the lowest role that may call them (Q22).
const WRITE_TOOLS: &[(&str, &str)] = &[
    ("propose_change", "editor"),
    ("apply_change", "editor"),
    ("upload_file", "editor"),
    ("delete_file", "editor"),
    ("raise_pr", "editor"),
    ("restart_service", "operator"),
];

/// Write tools that act on the world and so must ask the user first (D52). `propose_change` only creates a draft.
const SIDE_EFFECT_TOOLS: &[&str] = &[
    "apply_change",
    "upload_file",
    "delete_file",
    "raise_pr",
    "restart_service",
];

/// The REST operation each tool wraps (`operationId` in `api/openapi.yaml`).
const REST_OPERATION: &[(&str, &str)] = &[
    ("list_swimlanes", "listSwimlanes"),
    ("get_swimlane", "getSwimlane"),
    ("list_files", "listSwimlaneTree"),
    ("read_file", "getFileContent"),
    ("get_effective_config", "getEffectiveConfig"),
    ("compare", "compareFile"),
    ("compare_tree", "compareTree"),
    ("compare_grid", "getGrid"),
    ("search_settings", "searchSettings"),
    ("search_text", "searchText"),
    ("find_inconsistencies", "listFindings"),
    ("get_drift", "listDrift"),
    ("get_pending_restarts", "listPendingRestarts"),
    ("get_history", "listFileHistory"),
    ("search_docs", "searchDocs"),
    ("read_doc", "readDoc"),
    ("grep_docs", "grepDocs"),
    ("get_docs_for_file", "docsForFile"),
    ("get_feature_status", "getFeatureStatus"),
    ("propose_change", "createDraft"),
    ("apply_change", "applyDraft"),
    ("upload_file", "uploadFile"),
    ("delete_file", "deleteFile"),
    ("raise_pr", "createPr"),
    ("restart_service", "restartService"),
];

/// Tools that page with a keyset cursor. Every other tool has no `cursor` input.
const CURSOR_TOOLS: &[&str] = &[
    "list_swimlanes",
    "list_files",
    "compare_tree",
    "compare_grid",
    "search_settings",
    "find_inconsistencies",
    "get_drift",
    "get_pending_restarts",
    "get_history",
];

/// P9 and D58: the default output of a tool is at most 16 KB.
const MAX_OUTPUT_KB: u64 = 16;

struct Tool {
    name: String,
    fields: BTreeMap<String, String>,
    input: Value,
    output: Value,
}

struct Doc {
    text: String,
    envelope: Value,
    error_result: Value,
    defs: Map<String, Value>,
    tools: Vec<Tool>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn first_json_block(section: &[&str], what: &str) -> Value {
    let mut inside = false;
    let mut buf = String::new();
    for line in section {
        if !inside && line.trim_start().starts_with("```json") {
            inside = true;
        } else if inside && line.trim_start().starts_with("```") {
            return serde_json::from_str(&buf)
                .unwrap_or_else(|e| panic!("{what}: JSON block does not parse: {e}"));
        } else if inside {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    panic!("{what}: no ```json block found");
}

fn parse_doc() -> Doc {
    let path = repo_root().join("docs/mcp-tools.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let lines: Vec<&str> = text.lines().collect();

    // Split into `## ` sections.
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in &lines {
        if let Some(title) = line.strip_prefix("## ") {
            sections.push((title.trim().to_owned(), Vec::new()));
        } else if let Some(last) = sections.last_mut() {
            last.1.push(line);
        }
    }
    let section = |title: &str| -> &Vec<&str> {
        &sections
            .iter()
            .find(|(t, _)| t == title)
            .unwrap_or_else(|| panic!("missing `## {title}` section"))
            .1
    };

    let envelope = first_json_block(section("Result envelope"), "Result envelope");
    let error_result = first_json_block(section("Errors"), "Errors");
    let defs_doc = first_json_block(section("Shared definitions"), "Shared definitions");
    let defs = defs_doc
        .get("$defs")
        .and_then(Value::as_object)
        .expect("Shared definitions must be an object with a `$defs` key")
        .clone();

    // Tool sections: `### `name``, until the next heading.
    let mut tools: Vec<Tool> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    let mut finished: Vec<(String, Vec<&str>)> = Vec::new();
    for line in section("Tools") {
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some(done) = current.take() {
                finished.push(done);
            }
            let name = rest.trim().trim_matches('`').to_owned();
            current = Some((name, Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some(done) = current.take() {
        finished.push(done);
    }

    for (name, body) in finished {
        let mut fields = BTreeMap::new();
        for line in &body {
            if let Some(rest) = line.strip_prefix("- ")
                && let Some((k, v)) = rest.split_once(':')
            {
                let key = k.trim().to_owned();
                if ["role", "confirmation", "output cap", "pagination", "rest"].contains(&key.as_str()) {
                    fields.insert(key, v.trim().trim_matches('`').to_owned());
                }
            }
        }
        let block_after = |label: &str| -> Value {
            let at = body
                .iter()
                .position(|l| l.trim() == label)
                .unwrap_or_else(|| panic!("tool `{name}`: missing the `{label}` label"));
            first_json_block(&body[at + 1..], &format!("tool `{name}` {label}"))
        };
        let input = block_after("**Input schema**");
        let output = block_after("**Output schema**");
        tools.push(Tool {
            name,
            fields,
            input,
            output,
        });
    }

    Doc {
        text,
        envelope,
        error_result,
        defs,
        tools,
    }
}

/// Every JSON object in the tree that is a schema node: its `type` is a string or an array of strings.
fn schema_nodes<'a>(v: &'a Value, out: &mut Vec<&'a Map<String, Value>>) {
    match v {
        Value::Object(m) => {
            if matches!(m.get("type"), Some(Value::String(_) | Value::Array(_))) {
                out.push(m);
            }
            for child in m.values() {
                schema_nodes(child, out);
            }
        }
        Value::Array(a) => {
            for child in a {
                schema_nodes(child, out);
            }
        }
        _ => {}
    }
}

fn types_of(m: &Map<String, Value>) -> Vec<&str> {
    match m.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                out.push(r.clone());
            }
            for child in m.values() {
                collect_refs(child, out);
            }
        }
        Value::Array(a) => {
            for child in a {
                collect_refs(child, out);
            }
        }
        _ => {}
    }
}

fn contains_key(v: &Value, key: &str) -> bool {
    match v {
        Value::Object(m) => m.contains_key(key) || m.values().any(|c| contains_key(c, key)),
        Value::Array(a) => a.iter().any(|c| contains_key(c, key)),
        _ => false,
    }
}

/// The schema with every `$ref` replaced by its shared definition, so key searches see through references.
fn expanded(doc: &Doc, v: &Value, depth: u8) -> Value {
    assert!(depth < 12, "shared definitions refer to each other in a cycle");
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                let name = r.strip_prefix("#/$defs/").expect("refs point into #/$defs");
                return expanded(doc, &doc.defs[name], depth + 1);
            }
            Value::Object(
                m.iter()
                    .map(|(k, c)| (k.clone(), expanded(doc, c, depth)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(|c| expanded(doc, c, depth)).collect()),
        other => other.clone(),
    }
}

fn tool<'a>(doc: &'a Doc, name: &str) -> &'a Tool {
    doc.tools
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no section for tool `{name}`"))
}

fn expected_names() -> BTreeSet<&'static str> {
    READ_TOOLS
        .iter()
        .copied()
        .chain(WRITE_TOOLS.iter().map(|(n, _)| *n))
        .collect()
}

#[test]
fn exactly_the_25_specified_tools_have_a_section() {
    let doc = parse_doc();
    let found: Vec<&str> = doc.tools.iter().map(|t| t.name.as_str()).collect();
    let unique: BTreeSet<&str> = found.iter().copied().collect();
    assert_eq!(found.len(), unique.len(), "a tool has two sections: {found:?}");
    assert_eq!(unique, expected_names(), "the tool set differs from prompt 01 T7");
    assert_eq!(found.len(), 25);
}

#[test]
fn no_notify_tool_is_exposed() {
    // Q22 and Q37: /update-resources is unauthenticated, so no tool may reach it, and the doc says why.
    let doc = parse_doc();
    for t in &doc.tools {
        assert!(
            !t.name.contains("notify"),
            "`{}` exposes the unauthenticated notify endpoint",
            t.name
        );
        assert_ne!(t.name, "get_served_config");
        assert!(
            !t.fields.get("rest").is_some_and(|r| r.contains("notify")),
            "`{}` wraps a notify operation",
            t.name
        );
    }
    assert!(
        doc.text.contains("/update-resources") && doc.text.contains("Q37"),
        "the document must explain the missing notify tool (Q22, Q37)"
    );
}

#[test]
fn every_tool_section_is_complete() {
    let doc = parse_doc();
    for t in &doc.tools {
        for key in ["role", "confirmation", "output cap", "pagination", "rest"] {
            assert!(
                t.fields.contains_key(key),
                "tool `{}` is missing the `{key}` field",
                t.name
            );
        }
        for (what, schema) in [("input", &t.input), ("output", &t.output)] {
            assert_eq!(
                schema.get("type").and_then(Value::as_str),
                Some("object"),
                "tool `{}` {what} schema must be an object",
                t.name
            );
            assert!(
                schema.get("properties").is_some_and(Value::is_object),
                "tool `{}` {what} schema needs `properties`",
                t.name
            );
        }
    }
}

#[test]
fn every_ref_resolves_to_a_shared_definition() {
    let doc = parse_doc();
    let mut refs = Vec::new();
    collect_refs(&doc.envelope, &mut refs);
    collect_refs(&doc.error_result, &mut refs);
    for t in &doc.tools {
        collect_refs(&t.input, &mut refs);
        collect_refs(&t.output, &mut refs);
    }
    collect_refs(&Value::Object(doc.defs.clone()), &mut refs);
    assert!(!refs.is_empty());
    for r in refs {
        let name = r
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("`{r}` must point into #/$defs"));
        assert!(
            doc.defs.contains_key(name),
            "`{r}` is not defined under Shared definitions"
        );
    }
}

#[test]
fn every_shared_definition_is_used() {
    let doc = parse_doc();
    let mut refs = Vec::new();
    collect_refs(&doc.envelope, &mut refs);
    collect_refs(&doc.error_result, &mut refs);
    for t in &doc.tools {
        collect_refs(&t.input, &mut refs);
        collect_refs(&t.output, &mut refs);
    }
    collect_refs(&Value::Object(doc.defs.clone()), &mut refs);
    let used: BTreeSet<&str> = refs.iter().filter_map(|r| r.strip_prefix("#/$defs/")).collect();
    for name in doc.defs.keys() {
        assert!(
            used.contains(name.as_str()),
            "shared definition `{name}` is never referenced"
        );
    }
}

#[test]
fn inputs_are_closed_and_their_required_keys_exist() {
    let doc = parse_doc();
    for t in &doc.tools {
        assert_eq!(
            t.input.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "tool `{}` input must set additionalProperties: false (S11)",
            t.name
        );
        let props = t.input["properties"].as_object().unwrap();
        if let Some(req) = t.input.get("required") {
            for r in req.as_array().expect("required must be an array") {
                let r = r.as_str().expect("required entries are strings");
                assert!(
                    props.contains_key(r),
                    "tool `{}` requires unknown property `{r}`",
                    t.name
                );
            }
        }
    }
}

#[test]
fn inputs_and_definitions_are_bounded() {
    // AGENTS.md rule 5: everything is bounded. Strings have a maximum length (or are enums), arrays a maximum item
    // count, numbers a maximum.
    let doc = parse_doc();
    let defs = Value::Object(doc.defs.clone());
    let mut targets: Vec<(String, &Value)> = vec![("Shared definitions".to_owned(), &defs)];
    for t in &doc.tools {
        targets.push((format!("tool `{}` input", t.name), &t.input));
    }
    for (label, value) in targets {
        let mut nodes = Vec::new();
        schema_nodes(value, &mut nodes);
        for n in nodes {
            let types = types_of(n);
            if types.contains(&"string") {
                assert!(
                    n.contains_key("maxLength") || n.contains_key("enum") || n.contains_key("const"),
                    "{label}: a string has no maxLength: {n:?}"
                );
            }
            if types.contains(&"array") {
                assert!(
                    n.contains_key("maxItems"),
                    "{label}: an array has no maxItems: {n:?}"
                );
            }
            if types.contains(&"integer") || types.contains(&"number") {
                assert!(
                    n.contains_key("maximum"),
                    "{label}: a number has no maximum: {n:?}"
                );
            }
        }
    }
}

#[test]
fn output_arrays_are_bounded() {
    let doc = parse_doc();
    for t in &doc.tools {
        let mut nodes = Vec::new();
        schema_nodes(&t.output, &mut nodes);
        for n in nodes {
            if types_of(n).contains(&"array") {
                assert!(
                    n.contains_key("maxItems"),
                    "tool `{}` output has an unbounded array: {n:?}",
                    t.name
                );
            }
        }
    }
}

#[test]
fn role_floor_follows_s14_and_matches_rest() {
    let doc = parse_doc();
    let openapi = std::fs::read_to_string(repo_root().join("api/openapi.yaml")).expect("read openapi.yaml");
    let rest: BTreeMap<&str, &str> = REST_OPERATION.iter().copied().collect();
    let write_roles: BTreeMap<&str, &str> = WRITE_TOOLS.iter().copied().collect();

    for t in &doc.tools {
        let role = t.fields["role"].as_str();
        let expected = write_roles.get(t.name.as_str()).copied().unwrap_or("viewer");
        assert_eq!(role, expected, "tool `{}`: minimum role", t.name);
        assert!(
            ["viewer", "editor", "operator"].contains(&role),
            "no tool may require `{role}`"
        );

        let op = t.fields["rest"].as_str();
        assert_eq!(
            Some(op),
            rest.get(t.name.as_str()).copied(),
            "tool `{}` wraps the wrong REST operation",
            t.name
        );
        assert_eq!(
            openapi_role(&openapi, op).as_deref(),
            Some(role),
            "tool `{}` and REST operation `{op}` disagree on the minimum role (S4)",
            t.name
        );
    }
}

/// The `x-required-role` of an operation, found by scanning the text between its `operationId` and the next one.
fn openapi_role(openapi: &str, operation_id: &str) -> Option<String> {
    let marker = format!("operationId: {operation_id}");
    let mut lines = openapi.lines().skip_while(|l| l.trim() != marker);
    lines.next()?;
    for line in lines {
        if line.trim_start().starts_with("operationId:") {
            return None;
        }
        if let Some(role) = line.trim().strip_prefix("x-required-role:") {
            return Some(role.trim().to_owned());
        }
    }
    None
}

#[test]
fn confirmation_rule_follows_d52() {
    let doc = parse_doc();
    for t in &doc.tools {
        let c = t.fields["confirmation"].as_str();
        if SIDE_EFFECT_TOOLS.contains(&t.name.as_str()) {
            assert_eq!(
                c, "required",
                "write tool `{}` must say `confirmation: required` (D52, S14)",
                t.name
            );
        } else {
            assert_eq!(
                c, "none",
                "tool `{}` has no side effect and must say `confirmation: none`",
                t.name
            );
        }
    }
    // The draft step has no side effect, which is why it needs no confirmation of its own; the doc says so.
    assert!(
        doc.text.contains("propose_change"),
        "propose_change must be explained"
    );
}

#[test]
fn output_caps_follow_p9_and_d58() {
    let doc = parse_doc();
    for t in &doc.tools {
        let cap = t.fields["output cap"].as_str();
        let kb: u64 = cap
            .strip_suffix(" KB")
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("tool `{}`: output cap `{cap}` must look like `16 KB`", t.name));
        assert!(
            (1..=MAX_OUTPUT_KB).contains(&kb),
            "tool `{}`: output cap {kb} KB is over {MAX_OUTPUT_KB} KB",
            t.name
        );
    }
    for name in READ_TOOLS {
        assert_eq!(
            tool(&doc, name).fields["output cap"],
            "16 KB",
            "read tool `{name}` defaults to the P9 cap"
        );
    }
    assert!(doc.text.contains("16 KB"));
}

#[test]
fn pagination_matches_the_cursor_inputs() {
    let doc = parse_doc();
    for t in &doc.tools {
        let props = t.input["properties"].as_object().unwrap();
        let paged = CURSOR_TOOLS.contains(&t.name.as_str());
        assert_eq!(
            t.fields["pagination"],
            if paged { "cursor" } else { "none" },
            "tool `{}`: pagination",
            t.name
        );
        assert_eq!(
            props.contains_key("cursor"),
            paged,
            "tool `{}`: `cursor` input",
            t.name
        );
        if paged {
            assert!(
                props.contains_key("limit"),
                "tool `{}`: a paged tool needs `limit`",
                t.name
            );
            let max = props["limit"].get("maximum").and_then(Value::as_u64);
            assert!(
                max.is_some_and(|m| m <= 500),
                "tool `{}`: limit maximum must be <= 500",
                t.name
            );
            assert!(
                !props["cursor"].is_null()
                    && props["cursor"].get("$ref") == Some(&Value::String("#/$defs/Cursor".into())),
                "tool `{}`: `cursor` must reference the shared Cursor type",
                t.name
            );
        }
    }
}

#[test]
fn result_envelope_has_the_d58_fields() {
    let doc = parse_doc();
    let props = doc.envelope["properties"]
        .as_object()
        .expect("envelope properties");
    for key in ["summary", "data", "truncated", "next", "web_link"] {
        assert!(props.contains_key(key), "the result envelope lacks `{key}`");
    }
    let required: BTreeSet<&str> = doc.envelope["required"]
        .as_array()
        .expect("envelope required")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for key in ["summary", "data", "truncated", "next", "web_link"] {
        assert!(required.contains(key), "the envelope must always carry `{key}`");
    }
    let summary_max = props["summary"].get("maxLength").and_then(Value::as_u64);
    assert!(summary_max.is_some_and(|m| m <= 512), "the summary must be short");
}

#[test]
fn error_shape_and_codes_are_documented() {
    let doc = parse_doc();
    assert!(
        doc.defs.contains_key("ToolError"),
        "Shared definitions must define ToolError"
    );
    assert_eq!(
        doc.error_result["required"],
        serde_json::json!(["error"]),
        "the error result must be `{{ error: ToolError }}`"
    );
    assert_eq!(
        doc.error_result["properties"]["error"]["$ref"],
        "#/$defs/ToolError"
    );
    let codes = doc.defs["ToolError"]["properties"]["code"]["enum"]
        .as_array()
        .expect("ToolError.code must be an enum");
    let codes: BTreeSet<&str> = codes.iter().filter_map(Value::as_str).collect();
    for code in [
        "invalid_argument",
        "forbidden",
        "not_found",
        "conflict",
        "locked",
        "payload_too_large",
        "unprocessable",
        "rate_limited",
        "confirmation_declined",
        "confirmation_cancelled",
        "unavailable",
        "internal",
    ] {
        assert!(codes.contains(code), "ToolError.code lacks `{code}`");
    }
}

#[test]
fn conventions_for_untrusted_text_and_secrets_are_stated() {
    // S14, S14b, S15, D79: these sentences are what prompt 07 implements; their wording is part of the contract.
    let doc = parse_doc();
    for needle in [
        "data, not instructions",
        "[REDACTED: N chars, rule]",
        "content withheld",
        "mcp.access",
        "No tool executes a shell",
        "No tool reveals a flagged value",
    ] {
        assert!(doc.text.contains(needle), "the document must state: {needle}");
    }
}

#[test]
fn file_and_doc_text_tools_label_their_text_as_data() {
    // Every tool that can return file or doc text says so with a `text` block marker field.
    let doc = parse_doc();
    for name in [
        "read_file",
        "read_doc",
        "grep_docs",
        "search_text",
        "search_docs",
        "get_effective_config",
        "compare",
    ] {
        assert!(
            contains_key(&expanded(&doc, &tool(&doc, name).output, 0), "untrusted"),
            "tool `{name}` output must mark its text with an `untrusted` flag (S15)"
        );
    }
}

#[test]
fn read_file_supports_lines_or_setting_path_and_withholds_denied_files() {
    let doc = parse_doc();
    let t = tool(&doc, "read_file");
    let props = t.input["properties"].as_object().unwrap();
    assert!(
        props.contains_key("lines") && props.contains_key("setting_path"),
        "D58: lines or setting_path"
    );
    assert!(
        t.input.get("not").is_some() || t.input.get("oneOf").is_some() || t.input.get("allOf").is_some(),
        "lines and setting_path are mutually exclusive and the schema must say so"
    );
    assert!(
        contains_key(&expanded(&doc, &t.output, 0), "content_withheld"),
        "denied files are withheld (D79)"
    );
}

#[test]
fn history_and_drift_carry_attribution_and_severity() {
    // Prompt 07 T6: history and drift outputs include each change's attribution and its severity (D73, D76).
    let doc = parse_doc();
    for name in ["get_history", "get_drift"] {
        let out = &expanded(&doc, &tool(&doc, name).output, 0);
        assert!(
            contains_key(out, "attribution"),
            "`{name}` output must carry attribution"
        );
        assert!(
            contains_key(out, "severity"),
            "`{name}` output must carry severity"
        );
    }
    assert!(contains_key(
        &expanded(&doc, &tool(&doc, "find_inconsistencies").output, 0),
        "severity"
    ));
}

#[test]
fn pending_restarts_use_the_d85_pickup_states() {
    let doc = parse_doc();
    let pickup = doc.defs["PickupState"]["enum"]
        .as_array()
        .expect("PickupState enum");
    let pickup: Vec<&str> = pickup.iter().filter_map(Value::as_str).collect();
    assert_eq!(
        pickup,
        [
            "live",
            "live_within_ttl",
            "needs_notify_or_restart",
            "needs_config_server_restart"
        ],
        "D85: there is no bare pending-restart state"
    );
    assert!(contains_key(
        &expanded(&doc, &tool(&doc, "get_pending_restarts").output, 0),
        "pickup"
    ));
}

#[test]
fn write_tools_carry_expected_hashes_and_report_all_outcomes() {
    // Rule 11: no blind writes. Every file write names what it expects to find.
    let doc = parse_doc();
    for name in ["propose_change", "upload_file"] {
        let t = tool(&doc, name);
        let required = t.input["required"].as_array().expect("required");
        assert!(
            required.iter().any(|r| r == "expected"),
            "`{name}` must require `expected`"
        );
    }
    let del = tool(&doc, "delete_file");
    let required = del.input["required"].as_array().expect("required");
    assert!(
        required.iter().any(|r| r == "expected_hash"),
        "`delete_file` must require `expected_hash`"
    );

    let outcomes = doc.defs["WriteOutcome"]["properties"]["outcome"]["enum"]
        .as_array()
        .expect("WriteOutcome.outcome enum");
    let outcomes: BTreeSet<&str> = outcomes.iter().filter_map(Value::as_str).collect();
    for o in ["applied", "proposal_created", "draft_link"] {
        assert!(outcomes.contains(o), "WriteOutcome lacks `{o}`");
    }
}

#[test]
fn the_secret_flag_never_has_a_reveal_path() {
    // S14b, D79: revealing a flagged value is a web UI action. No input or tool name can ask for it.
    let doc = parse_doc();
    for t in &doc.tools {
        assert!(
            !t.name.contains("reveal"),
            "`{}` must not reveal values over MCP",
            t.name
        );
        let props = t.input["properties"].as_object().unwrap();
        assert!(
            !props.keys().any(|k| k.contains("reveal") || k == "unredacted"),
            "`{}` has a reveal input",
            t.name
        );
    }
}
