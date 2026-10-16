//! Plan 07, T2: the schemas the server advertises are the schemas of `docs/mcp-tools.md` (S4, S11, S14).
//!
//! `src/schemas/tools.json` is generated from the document by the test below and never edited by hand:
//!
//! ```text
//! UPDATE_SCHEMAS=1 cargo test -p hub-mcp --test mcp_auth contract_schemas::embedded_schemas_equal_the_contract_document
//! ```
//!
//! (`just mcp-schemas`). Run normally, the same test fails when the file and the document differ, so a contract change to the
//! document (a new tool, a changed field, a reworded paragraph) cannot reach the server without the file following it.
//!
//! The extraction here is deliberately separate from the library's loader: the loader reads the generated file, and this reads
//! the Markdown, so each checks the other.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use domain::Role;
use hub_mcp::schemas::{Confirmation, Contract, Pagination};
use ports::Users as _;
use serde_json::{Map, Value, json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tools_json_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/schemas/tools.json")
}

/// The first fenced JSON block among `lines`.
fn first_json_block(lines: &[&str], what: &str) -> Value {
    let mut inside = false;
    let mut buffer = String::new();
    for line in lines {
        if !inside && line.trim_start().starts_with("```json") {
            inside = true;
        } else if inside && line.trim_start().starts_with("```") {
            return serde_json::from_str(&buffer)
                .unwrap_or_else(|e| panic!("{what}: JSON does not parse: {e}"));
        } else if inside {
            buffer.push_str(line);
            buffer.push('\n');
        }
    }
    panic!("{what}: no ```json block");
}

/// What the document says about one tool.
pub struct DocTool {
    pub name: String,
    pub description: String,
    pub fields: BTreeMap<String, String>,
    pub input: Value,
    pub output: Value,
}

pub struct Doc {
    pub envelope: Value,
    pub error_result: Value,
    pub defs: Map<String, Value>,
    pub tools: Vec<DocTool>,
}

const FIELD_KEYS: [&str; 5] = ["role", "confirmation", "output cap", "pagination", "rest"];

pub fn parse_doc() -> Doc {
    let path = repo_root().join("docs/mcp-tools.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let lines: Vec<&str> = text.lines().collect();

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
            .unwrap_or_else(|| panic!("no `## {title}` section"))
            .1
    };
    let envelope = first_json_block(section("Result envelope"), "Result envelope");
    let error_result = first_json_block(section("Errors"), "Errors");
    let defs = first_json_block(section("Shared definitions"), "Shared definitions")
        .get("$defs")
        .and_then(Value::as_object)
        .expect("`$defs` in Shared definitions")
        .clone();

    let mut bodies: Vec<(String, Vec<&str>)> = Vec::new();
    for line in section("Tools") {
        if let Some(rest) = line.strip_prefix("### ") {
            bodies.push((rest.trim().trim_matches('`').to_owned(), Vec::new()));
        } else if let Some((_, body)) = bodies.last_mut() {
            body.push(line);
        }
    }
    let mut tools = Vec::new();
    for (name, body) in bodies {
        let mut fields = BTreeMap::new();
        let mut prose: Vec<&str> = Vec::new();
        let mut seen_schema = false;
        for line in &body {
            if line.trim() == "**Input schema**" {
                seen_schema = true;
            }
            if seen_schema {
                break;
            }
            let field = line
                .strip_prefix("- ")
                .and_then(|rest| rest.split_once(':'))
                .filter(|(k, _)| FIELD_KEYS.contains(&k.trim()));
            if let Some((k, v)) = field {
                fields.insert(k.trim().to_owned(), v.trim().trim_matches('`').to_owned());
            } else {
                prose.push(line);
            }
        }
        // Paragraphs, separated by one blank line; the bullet list between them leaves no gap.
        let mut paragraphs: Vec<String> = Vec::new();
        let mut current: Vec<&str> = Vec::new();
        for line in prose {
            if line.trim().is_empty() {
                if !current.is_empty() {
                    paragraphs.push(current.join(" "));
                    current.clear();
                }
            } else {
                current.push(line.trim());
            }
        }
        if !current.is_empty() {
            paragraphs.push(current.join(" "));
        }
        let block_after = |label: &str| -> Value {
            let at = body
                .iter()
                .position(|l| l.trim() == label)
                .unwrap_or_else(|| panic!("tool `{name}`: no `{label}`"));
            first_json_block(&body[at + 1..], &format!("tool `{name}` {label}"))
        };
        tools.push(DocTool {
            description: paragraphs.join("\n\n"),
            fields,
            input: block_after("**Input schema**"),
            output: block_after("**Output schema**"),
            name,
        });
    }
    Doc {
        envelope,
        error_result,
        defs,
        tools,
    }
}

fn collect_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                out.push(r.clone());
            }
            map.values().for_each(|v| collect_refs(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

/// `schema` with the transitive closure of the definitions it refers to, under `$defs`, so it stands alone.
pub fn with_closure(defs: &Map<String, Value>, schema: &Value) -> Value {
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    let mut queue = Vec::new();
    collect_refs(schema, &mut queue);
    while let Some(reference) = queue.pop() {
        let name = reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("a reference that is not into `#/$defs`: {reference}"))
            .to_owned();
        if wanted.insert(name.clone()) {
            let def = defs
                .get(&name)
                .unwrap_or_else(|| panic!("no definition `{name}`"));
            collect_refs(def, &mut queue);
        }
    }
    let mut out = schema.as_object().expect("a schema object").clone();
    if !wanted.is_empty() {
        let closure: Map<String, Value> = wanted
            .into_iter()
            .map(|n| (n.clone(), defs[&n].clone()))
            .collect();
        out.insert("$defs".to_owned(), Value::Object(closure));
    }
    Value::Object(out)
}

fn cap_bytes(text: &str) -> u64 {
    let kb: u64 = text
        .strip_suffix(" KB")
        .unwrap_or_else(|| panic!("output cap `{text}`"))
        .parse()
        .expect("a number of KB");
    kb * 1024
}

/// The generated file, from the document.
pub fn generate() -> String {
    let doc = parse_doc();
    let tools: Vec<Value> = doc
        .tools
        .iter()
        .map(|t| {
            let field = |k: &str| {
                t.fields
                    .get(k)
                    .unwrap_or_else(|| panic!("tool `{}` lacks `{k}`", t.name))
                    .clone()
            };
            json!({
                "name": t.name,
                "description": t.description,
                "role": field("role"),
                "confirmation": field("confirmation"),
                "output_cap_bytes": cap_bytes(&field("output cap")),
                "pagination": field("pagination"),
                "rest": field("rest"),
                "input": with_closure(&doc.defs, &t.input),
                "output": with_closure(&doc.defs, &t.output),
            })
        })
        .collect();
    let file = json!({
        "schema": 1,
        "source": "docs/mcp-tools.md",
        "envelope": with_closure(&doc.defs, &doc.envelope),
        "error_result": with_closure(&doc.defs, &doc.error_result),
        "tools": tools,
    });
    let mut text = serde_json::to_string_pretty(&file).unwrap();
    text.push('\n');
    text
}

#[test]
fn embedded_schemas_equal_the_contract_document() {
    let generated = generate();
    let path = tools_json_path();
    if std::env::var_os("UPDATE_SCHEMAS").is_some_and(|v| v == "1") {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == generated,
        "src/schemas/tools.json is not what docs/mcp-tools.md says. Regenerate it with `just mcp-schemas` \
         (UPDATE_SCHEMAS=1 cargo test -p hub-mcp --test mcp_auth contract_schemas::embedded_schemas_equal_the_contract_document) \
         and commit the result; never edit the file by hand."
    );
    // The library's copy is the file's, byte for byte.
    assert_eq!(hub_mcp::schemas::TOOLS_JSON, generated);
}

#[test]
fn the_embedded_contract_loads_and_every_schema_stands_alone() {
    let contract = Contract::embedded().expect("the embedded contract loads");
    assert_eq!(contract.tools().len(), 27);
    for tool in contract.tools() {
        for (what, schema) in [
            ("input", tool.input()),
            ("output", tool.output()),
            ("result", tool.result()),
        ] {
            let mut refs = Vec::new();
            collect_refs(&Value::Object((**schema).clone()), &mut refs);
            let defs = schema.get("$defs").and_then(Value::as_object);
            for reference in refs {
                let name = reference.strip_prefix("#/$defs/").unwrap();
                assert!(
                    defs.is_some_and(|d| d.contains_key(name)),
                    "{} {what}: `{reference}` is not embedded",
                    tool.name()
                );
            }
        }
    }
}

#[test]
fn advertised_roles_confirmations_caps_match_the_document() {
    let doc = parse_doc();
    let contract = Contract::embedded().unwrap();
    assert_eq!(
        contract.tools().iter().map(|t| t.name()).collect::<Vec<_>>(),
        doc.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        "the same tools, in the same order"
    );
    for (spec, documented) in contract.tools().iter().zip(&doc.tools) {
        let field = |k: &str| documented.fields[k].as_str();
        assert_eq!(spec.role().as_str(), field("role"), "{}", spec.name());
        assert_eq!(
            match spec.confirmation() {
                Confirmation::None => "none",
                Confirmation::Required => "required",
            },
            field("confirmation"),
            "{}",
            spec.name()
        );
        assert_eq!(
            spec.output_cap_bytes() as u64,
            cap_bytes(field("output cap")),
            "{}",
            spec.name()
        );
        assert_eq!(
            match spec.pagination() {
                Pagination::None => "none",
                Pagination::Cursor => "cursor",
            },
            field("pagination"),
            "{}",
            spec.name()
        );
        assert_eq!(spec.rest(), field("rest"), "{}", spec.name());
        // Only a tool with a cursor input pages, and every tool that pages has one.
        let has_cursor = spec
            .input()
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key("cursor"));
        assert_eq!(
            has_cursor,
            spec.pagination() == Pagination::Cursor,
            "{}",
            spec.name()
        );
    }
    // The six tools that act on the world, and only they, need confirmation (D52).
    let confirmed: BTreeSet<&str> = contract
        .tools()
        .iter()
        .filter(|t| t.confirmation() == Confirmation::Required)
        .map(|t| t.name())
        .collect();
    assert_eq!(
        confirmed,
        BTreeSet::from([
            "apply_change",
            "upload_file",
            "delete_file",
            "raise_pr",
            "restart_service",
            "notify_services"
        ])
    );
}

#[test]
fn the_result_schema_is_the_envelope_with_the_tools_data_inside() {
    let contract = Contract::embedded().unwrap();
    for tool in contract.tools() {
        let result = tool.result();
        let props = result["properties"].as_object().expect("properties");
        assert_eq!(
            props.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["summary", "data", "truncated", "next", "web_link"])
        );
        // `data` is this tool's output schema, with its definitions merged into the result's.
        let data = props["data"].as_object().unwrap();
        for (key, value) in tool.output().iter().filter(|(k, _)| *k != "$defs") {
            assert_eq!(data.get(key), Some(value), "{}: data.{key}", tool.name());
        }
        assert_eq!(result["additionalProperties"], json!(false));
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The server against the contract
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn every_documented_tool_has_a_handler_and_every_handler_a_spec() {
    use hub_mcp::tools::{HostError, ToolHost, ToolName};
    let contract = Contract::embedded().unwrap();
    // The two sets are the same.
    for spec in contract.tools() {
        assert!(
            ToolName::from_name(spec.name()).is_some(),
            "{} has no handler",
            spec.name()
        );
    }
    for tool in ToolName::ALL {
        assert!(contract.tool(tool.as_str()).is_some(), "{tool} has no spec");
    }
    assert!(ToolHost::listing(&contract, true).is_ok());
    // A contract that lacks a tool the handlers have, or has one they lack, does not start the server.
    let mut file: Value = serde_json::from_str(hub_mcp::schemas::TOOLS_JSON).unwrap();
    let tools = file["tools"].as_array_mut().unwrap();
    let mut without = tools.clone();
    without.retain(|t| t["name"] != "read_file");
    let mut with_extra = tools.clone();
    let mut extra = tools[0].clone();
    extra["name"] = json!("rm_rf");
    with_extra.push(extra);
    for variant in [without, with_extra] {
        file["tools"] = Value::Array(variant);
        let changed = Contract::parse(&file.to_string()).unwrap();
        assert_eq!(
            ToolHost::listing(&changed, true).unwrap_err(),
            HostError::Mismatch
        );
    }
    // A tool named twice is refused outright.
    let mut twice = file["tools"].as_array().unwrap().clone();
    twice.push(twice[0].clone());
    file["tools"] = Value::Array(twice);
    assert!(Contract::parse(&file.to_string()).is_err());
}

#[tokio::test]
async fn outputs_validate_against_output_schemas() {
    use crate::args::full_examples;
    use crate::rig::Rig;
    use crate::tools_rig::{call, signed_in};
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.read_burst = 1_000;
            s.limits.write_burst = 1_000;
            s.limits.read_per_minute = 60_000;
            s.limits.write_per_minute = 6_000;
            s.tools.notify_services_enabled = true;
            s
        },
    );
    rig.registry.inner.seed(
        &crate::fix::sid("sit1"),
        &[(crate::fix::np("a/b.yml"), crate::fix::bytes("a: 1\n"))],
    );
    let mut admin = signed_in(&rig, "root", Role::Admin).await;
    for (tool, args) in full_examples() {
        let called = call(&mut admin, tool.as_str(), &args).await;
        // Whatever a tool answers, it answers in the shape of the contract: a result of its schema, or an error of the error schema.
        if called.is_error() {
            called.err();
        } else {
            called.ok();
        }
    }
}

#[test]
fn error_results_validate_against_the_error_schema() {
    use crate::tools_rig::error_schema_problems;
    use domain::{RejectReason, ValidationIssue};
    use hub_mcp::tools::{Code, ToolError};
    assert_eq!(Code::ALL.len(), 12, "the contract's twelve codes");
    let documented: BTreeSet<String> =
        Contract::embedded().unwrap().error_result()["$defs"]["ToolError"]["properties"]["code"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_owned())
            .collect();
    assert_eq!(
        Code::ALL
            .iter()
            .map(|c| c.as_str().to_owned())
            .collect::<BTreeSet<_>>(),
        documented
    );
    for code in Code::ALL {
        let mut bare = ToolError::new(code);
        let (value, message) = bare.render("req-1");
        assert!(
            error_schema_problems(&value).is_empty(),
            "{code:?}: {:?}",
            error_schema_problems(&value)
        );
        assert_eq!(value["error"]["code"], code.as_str());
        assert!(!message.is_empty() && message.len() <= 512);
        // With every optional field set.
        bare.current_hash = Some(crate::fix::hash_of(b"x"));
        bare.retry_after_secs = Some(30);
        bare.reason = Some(RejectReason::SecretDetected);
        bare.issues = vec![ValidationIssue {
            kind: domain::IssueKind::SecretMatch,
            line: std::num::NonZeroU32::new(4),
            rule: Some(crate::fix::st("github_token")),
        }];
        let (value, _) = bare.render("req-1");
        assert!(
            error_schema_problems(&value).is_empty(),
            "{code:?}: {:?}",
            error_schema_problems(&value)
        );
        // The fixed sentences hold no text of any request.
        assert!(!value.to_string().contains("tok-"));
    }
}

#[tokio::test]
async fn tools_list_size_is_reported() {
    use crate::rig::Rig;
    use crate::tools_rig::signed_in;
    let rig = Rig::oauth();
    let mut client = signed_in(&rig, "root", Role::Admin).await;
    let id = client.id();
    let response = client
        .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
        .await;
    let (_, message) = crate::support::Sse::new(response.into_body())
        .until_response(id)
        .await
        .unwrap();
    let wire = serde_json::to_vec(&message["result"]["tools"]).unwrap().len();
    let user = rig.users.get(&crate::rig::uid("root")).await.unwrap().unwrap();
    let reported = rig.state.tools_list_bytes(&user);
    assert_eq!(reported, wire, "the number the server reports is what it sent");
    eprintln!("tools/list for an Admin: {wire} bytes, 26 tools (notify_services is off)");
    // Measured: 112,340 bytes for 26 tools, input and result schemas included (2026-10-15, round 1 of the review). The upper bound
    // is the measurement and about a tenth more, so that a new tool or a longer schema is a visible decision: when the contract
    // (`docs/mcp-tools.md`) grows on purpose, this number grows with it, in the same PR. Every client puts this list in front of the
    // model on each session, so its size is a cost to the user. There is no tight lower bound: a contract that gets smaller is a
    // good change and must not fail this test. The floor only catches a list that has lost its tools or its schemas altogether (a
    // single tool is a few thousand bytes).
    assert!(wire <= 125_000, "tools/list is {wire} bytes");
    assert!(
        wire >= 20_000,
        "tools/list is only {wire} bytes: tools or schemas are missing"
    );
    // Without the result schemas it is smaller by a good part.
    let lean = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.advertise_output_schemas = false;
            s
        },
    );
    let mut client = signed_in(&lean, "root", Role::Admin).await;
    let id = client.id();
    let response = client
        .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
        .await;
    let (_, message) = crate::support::Sse::new(response.into_body())
        .until_response(id)
        .await
        .unwrap();
    let lean_wire = serde_json::to_vec(&message["result"]["tools"]).unwrap().len();
    eprintln!("tools/list for an Admin without result schemas: {lean_wire} bytes");
    // Measured: 31,235 bytes, less than a third of the full list. Same shape of bound: an upper one (the measurement and about a
    // tenth more) and a floor that only catches a list with nothing in it.
    assert!(
        lean_wire <= 35_000,
        "tools/list without result schemas is {lean_wire} bytes"
    );
    assert!(
        lean_wire >= 8_000,
        "tools/list without result schemas is only {lean_wire} bytes: tools are missing"
    );
    // The result schemas are what the full list adds. This holds however the schemas change, unless they are all empty.
    assert!(lean_wire < wire, "{lean_wire} against {wire}");
    assert!(message["result"]["tools"][0].get("outputSchema").is_none());
}
