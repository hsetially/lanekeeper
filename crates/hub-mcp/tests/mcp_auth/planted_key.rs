//! Plan 07, T6 (S14b, S15): a planted fake key never appears in any tool output.
//!
//! The matrix runs every tool that can carry file or doc text, in both ways the text can reach the model (`ContentMode::Summary`,
//! where `content` is a summary and the text is in `structuredContent`, and `ContentMode::DataBlock`, where `content` carries the
//! data a second time). The key is built at run time from parts (A18), planted in the fixture the tool reads from, and the whole
//! response (`structuredContent` and `content`) is checked for it: for the key, for its body without the prefix (a partial reveal is
//! a reveal), and for the marker that must stand in its place. A result with no marker would prove nothing, because the key might
//! not have reached the tool at all.
//!
//! The matrix is complete by construction: `every_tool_is_in_the_matrix_or_says_why_it_carries_no_text` fails when a tool of the
//! contract is in neither list, so a new tool has to decide.
//!
//! The final sweep has its own tests here too: it replaces what a field that skipped the typed path leaves in a result, counts it,
//! leaves everything else byte for byte as it was, and fails closed when it cannot scan.

use domain::{
    ChangeKind, FindingKind, Grid, GridCell, GridRow, Paged, Role, SettingHit, Severity, TextHit, TextResults,
};
use hub_mcp::config::{AuthMode, ContentMode};
use hub_mcp::output::Redactor;
use hub_mcp::output::envelope::{Parts, Rendered, Shape, render};
use hub_mcp::output::sweep::sweep;
use hub_mcp::tools::ToolName;
use serde_json::{Value, json};

use crate::fix::*;
use crate::read_tools::{comparison, effective_fixture};
use crate::rig::Rig;
use crate::served::served_fixture;
use crate::tools_rig::{call, signed_in};

const MODES: [ContentMode; 2] = [ContentMode::Summary, ContentMode::DataBlock];

fn rig_in(mode: ContentMode) -> Rig {
    Rig::build(
        AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.config.content_mode = mode;
            s.limits.write_burst = 100;
            s.limits.write_per_minute = 6_000;
            s
        },
    )
}

fn seed(rig: &Rig, swimlane: &str, files: &[(&str, &str)]) {
    let files: Vec<_> = files.iter().map(|(p, t)| (np(p), bytes(t))).collect();
    assert!(rig.registry.inner.seed(&sid(swimlane), &files));
}

fn dp(s: &str) -> domain::DocPath {
    domain::DocPath::parse(s).unwrap()
}

/// One way a key can reach a tool: where it is planted, who asks, and what is asked.
struct Scenario {
    name: &'static str,
    tool: &'static str,
    role: Role,
    /// Plants `secret` where the tool will read it from.
    plant: fn(&Rig, &str),
    args: fn() -> Value,
}

const PATH: &str = "tx-core/app.yml";

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "read_file",
            tool: "read_file",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[(PATH, &format!("a: 1\ntoken: {secret}\nb: 2\n"))]);
            },
            args: || json!({"swimlane": "sit1", "path": PATH}),
        },
        Scenario {
            name: "get_effective_config",
            tool: "get_effective_config",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[(PATH, "a: 1\n")]);
                rig.registry.inner.set_effective(effective_fixture(secret));
            },
            args: || json!({"swimlane": "sit1", "file": PATH, "tenant": "sit1"}),
        },
        Scenario {
            name: "get_served_config",
            tool: "get_served_config",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                rig.registry.inner.set_served(served_fixture(
                    Some(&format!("a: 1\ntoken: {secret}\nb: 2\n")),
                    false,
                ));
            },
            args: || json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": "app.yml"}),
        },
        Scenario {
            name: "compare, setting changes",
            tool: "compare",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                let mut c = comparison(domain::FileClass::Structured);
                c.setting_changes = vec![
                    change("token", ChangeKind::Changed, Some(secret), Some("rotated")),
                    change("other", ChangeKind::Added, None, Some(secret)),
                ];
                rig.registry.inner.set_comparison(c);
            },
            args: || json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": "a.yml", "detail": "settings"}),
        },
        Scenario {
            name: "compare, line diff",
            tool: "compare",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                let mut c = comparison(domain::FileClass::Text);
                c.hunks[0].lines[1].text = format!("token: {secret}");
                rig.registry.inner.set_comparison(c);
            },
            args: || json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": "a.yml", "detail": "lines"}),
        },
        Scenario {
            name: "compare_grid",
            tool: "compare_grid",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                seed(rig, "sit2", &[("a.yml", "a: 1\n")]);
                let cell = |text: &str| GridCell {
                    value: Some(value(text)),
                    location: Some(location("token", 2, 2)),
                };
                rig.registry.inner.set_grid(Grid {
                    swimlanes: vec![sid("sit1"), sid("sit2")],
                    rows: Paged::last(vec![GridRow {
                        path: sp("token"),
                        cells: vec![cell(secret), cell("other")],
                    }]),
                });
            },
            args: || json!({"file": "a.yml", "swimlanes": ["sit1", "sit2"], "only_differing": false}),
        },
        Scenario {
            name: "search_settings",
            tool: "search_settings",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                rig.registry.inner.set_setting_hits(vec![SettingHit {
                    swimlane: sid("sit1"),
                    file: np("a.yml"),
                    path: sp("token"),
                    value: value(secret),
                    location: location("token", 2, 2),
                    logical_file: None,
                    tenant: None,
                }]);
            },
            args: || json!({"q": "token", "swimlanes": ["sit1"]}),
        },
        Scenario {
            name: "search_text",
            tool: "search_text",
            role: Role::Viewer,
            plant: |rig, secret| {
                seed(rig, "sit1", &[("a.yml", "a: 1\n")]);
                rig.registry.inner.set_text_results(TextResults {
                    hits: vec![TextHit {
                        swimlane: sid("sit1"),
                        path: np("a.yml"),
                        line: 2,
                        text: format!("token: {secret} # timeout"),
                        ranges: vec![(0, 5)],
                        logical_file: None,
                        tenant: None,
                    }],
                    truncated: false,
                });
            },
            args: || json!({"pattern": "token", "swimlanes": ["sit1"]}),
        },
        Scenario {
            name: "search_docs, snippet",
            tool: "search_docs",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.inner.seed(
                    dp("/git/main/guide.md"),
                    "Guide",
                    &format!("# Guide\nthe timeout token is {secret} here\n"),
                );
            },
            args: || json!({"q": "timeout"}),
        },
        Scenario {
            // A doc's title and its headings are doc text too, though they are short names and not a body.
            name: "search_docs, title and headings",
            tool: "search_docs",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.script_search(vec![domain::DocHit {
                    doc: domain::DocId::from(1),
                    title: st(&format!("Guide {secret}")),
                    path: dp("/git/main/guide.md"),
                    heading_path: vec![st("Operations"), st(&format!("Heading {secret}"))],
                    snippet: "the timeout is 30".to_owned(),
                    score: 0.5,
                    related_files: Vec::new(),
                }]);
            },
            args: || json!({"q": "timeout"}),
        },
        Scenario {
            name: "read_doc",
            tool: "read_doc",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.inner.seed(
                    dp("/git/main/guide.md"),
                    "Guide",
                    &format!("# Guide\nexample token {secret}\nlast line\n"),
                );
            },
            args: || json!({"path": "/git/main/guide.md"}),
        },
        Scenario {
            name: "grep_docs, hit and context",
            tool: "grep_docs",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.inner.seed(
                    dp("/git/main/guide.md"),
                    "Guide",
                    &format!("before {secret}\nthe timeout line\nafter {secret}\n"),
                );
            },
            args: || json!({"pattern": "timeout", "context": 1}),
        },
        Scenario {
            name: "get_docs_for_file",
            tool: "get_docs_for_file",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.inner.seed(
                    dp("/git/main/guide.md"),
                    "Guide",
                    &format!("# Guide\nsee `tx-core/app.yml` token {secret} for the timeout\n"),
                );
            },
            args: || json!({"file": PATH}),
        },
        Scenario {
            name: "get_feature_status",
            tool: "get_feature_status",
            role: Role::Viewer,
            plant: |rig, secret| {
                rig.docs.inner.set_feature_status(domain::FeatureStatus {
                    flags: vec![domain::FlagStatus {
                        flag: st("atm"),
                        file: lf(PATH),
                        channel: None,
                        setting: sp("features.atm"),
                        template_default: Some(value(secret)),
                        values: vec![domain::FlagValue {
                            swimlane: sid("sit1"),
                            value: Some(value(secret)),
                        }],
                    }],
                });
            },
            args: || json!({}),
        },
        Scenario {
            name: "propose_change",
            tool: "propose_change",
            role: Role::Editor,
            plant: |rig, secret| {
                rig.writes
                    .inner
                    .seed(&sid("sit1"), &[(np(PATH), bytes("name: one\nlevel: 1\n"))]);
                rig.writes.script_draft_facts(
                    vec![change("token", ChangeKind::Changed, Some(secret), Some(secret))],
                    Vec::new(),
                );
            },
            args: || {
                json!({"swimlane": "sit1", "path": PATH,
                       "expected": {"state": "hash", "hash": hash_of(b"name: one\nlevel: 1\n").to_string()},
                       "content": "name: one\nlevel: 2\n"})
            },
        },
    ]
}

/// The tools whose results hold no text from a file or a doc, and why. The contract's output schemas have only typed values for
/// them: names, ids, counts, states, hashes, times, paths and the sentences the server writes itself. A path, a setting path or a
/// service name is a typed value from the engine and the registry, not scanned (see the threat model); a message of a finding is
/// made by a check of the engine and holds paths, lines and hashes, never a value, and the final sweep is the net under it
/// (`late_sweep_catches_a_field_the_typed_path_missed`).
const NO_TEXT: [(&str, &str); 12] = [
    ("list_swimlanes", "names and counts"),
    ("get_swimlane", "services and states"),
    ("get_pending_restarts", "services and pickup states"),
    ("list_files", "paths, classes, hashes, attribution"),
    ("get_drift", "paths, states, hashes, attribution"),
    ("get_history", "hashes, times, attribution"),
    ("compare_tree", "paths and states"),
    (
        "find_inconsistencies",
        "findings: rules, paths, a sentence of the engine",
    ),
    ("apply_change", "an outcome, a hash, ids"),
    ("upload_file", "an outcome, a hash, ids"),
    ("delete_file", "an outcome, a hash, ids"),
    ("raise_pr", "an outcome and ids"),
];
/// Not in either list above: `restart_service` and `notify_services` are outcomes only and are listed here for the completeness
/// check.
const OUTCOME_ONLY: [&str; 2] = ["restart_service", "notify_services"];

#[test]
fn every_tool_is_in_the_matrix_or_says_why_it_carries_no_text() {
    let mut accounted: Vec<&str> = scenarios().iter().map(|s| s.tool).collect();
    accounted.extend(NO_TEXT.iter().map(|(t, _)| *t));
    accounted.extend(OUTCOME_ONLY);
    accounted.sort_unstable();
    accounted.dedup();
    let mut all: Vec<&str> = ToolName::ALL.iter().map(|t| t.as_str()).collect();
    all.sort_unstable();
    assert_eq!(
        accounted, all,
        "a tool of the contract is neither in the planted-key matrix nor listed as carrying no text"
    );
}

/// The response as a whole, as a client receives it: `structuredContent` and `content`.
fn everything(called: &crate::tools_rig::Called) -> String {
    called.response.to_string()
}

#[tokio::test]
async fn planted_key_never_appears_in_any_tool_output() {
    for mode in MODES {
        for scenario in scenarios() {
            let rig = rig_in(mode);
            let secret = fake_github_token();
            (scenario.plant)(&rig, &secret);
            let mut client = signed_in(&rig, "alice", scenario.role).await;
            let called = call(&mut client, scenario.tool, &(scenario.args)()).await;
            // A schema-valid success: the tool answered, and answered what the contract says.
            called.ok();
            let seen = everything(&called);
            let label = format!("{mode:?}, {}: {seen}", scenario.name);
            assert!(!seen.contains(&secret), "the key is in the result ({label})");
            assert!(
                !seen.contains(&secret["ghp_".len()..]),
                "the body of the key is in the result ({label})"
            );
            assert!(
                seen.contains("[REDACTED"),
                "nothing was redacted, so the key may not have reached the tool ({label})"
            );
            // The typed path did the work. The sweep had nothing left to replace.
            assert_eq!(
                rig.state.metrics().late_redactions(),
                0,
                "a field skipped the typed path and only the sweep caught it ({label})"
            );
        }
    }
}

#[tokio::test]
async fn a_key_cut_by_the_line_limit_of_a_window_is_hidden_in_both_modes() {
    // A private key whose header is above the window and whose end is below it.
    for mode in MODES {
        let rig = rig_in(mode);
        let begin = format!("-----BEGIN {} KEY-----", "RSA PRIVATE");
        let end = format!("-----END {} KEY-----", "RSA PRIVATE");
        let body: Vec<String> = (0..8)
            .map(|n| {
                format!(
                    "{}{n}{}",
                    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC", "x7Yz"
                )
            })
            .collect();
        let mut text = String::from("a: 1\nkey: |\n");
        text.push_str(&format!("  {begin}\n"));
        for line in &body {
            text.push_str(&format!("  {line}\n"));
        }
        text.push_str(&format!("  {end}\nb: 2\n"));
        seed(&rig, "sit1", &[("keys.yml", &text)]);
        let mut c = signed_in(&rig, "alice", Role::Viewer).await;
        // Lines 5 to 8 are the middle of the block.
        let called = call(
            &mut c,
            "read_file",
            &json!({"swimlane": "sit1", "path": "keys.yml", "lines": {"start": 5, "end": 8}}),
        )
        .await;
        called.ok();
        let seen = everything(&called);
        for line in &body[2..6] {
            assert!(
                !seen.contains(line.as_str()),
                "{mode:?}: a line of the key is shown"
            );
        }
        assert!(seen.contains("[REDACTED"), "{mode:?}: {seen}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The final sweep
// ---------------------------------------------------------------------------------------------------------------------

fn shape(mode: ContentMode) -> Shape {
    Shape {
        mode,
        cap: 16 * 1024,
        text_bearing: true,
    }
}

/// An envelope as a tool that did not use `RedactedText` would build it: the key sits in `data` as a plain string.
fn leaky(mode: ContentMode, secret: &str) -> Rendered {
    render(
        &shape(mode),
        Parts {
            summary: "Two hits.".to_owned(),
            data: json!({
                "hits": [
                    {"path": "a/b.yml", "line": 3, "text": format!("token: {secret}")},
                    {"path": "c.yml", "line": 4, "text": "plain"},
                ],
                "note": {"deep": [["x", format!("deep {secret}")]]},
            }),
            truncated: false,
            next: None,
            web_link: None,
        },
    )
}

fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

#[test]
fn the_sweep_replaces_what_a_field_that_skipped_the_typed_path_left() {
    for mode in MODES {
        let secret = fake_github_token();
        let before = leaky(mode, &secret);
        // The setup is right: the key is in the data, and in the content too in data-block mode.
        assert!(before.structured.to_string().contains(&secret));
        assert_eq!(before.content.contains(&secret), mode == ContentMode::DataBlock);
        let swept = sweep(&Redactor::new(), mode, before.clone()).unwrap();
        assert_eq!(swept.late, 2, "{mode:?}: two values were left in the data");
        let after = &swept.rendered;
        assert!(!after.structured.to_string().contains(&secret), "{mode:?}");
        assert!(!after.content.contains(&secret), "{mode:?}");
        assert!(!after.content.contains(&secret["ghp_".len()..]), "{mode:?}");
        assert!(
            after
                .structured
                .to_string()
                .contains("[REDACTED: 40 chars, github_token]")
        );
        // The shape is the same, and what was fine is untouched.
        assert_eq!(
            after.structured["data"]["hits"][1],
            before.structured["data"]["hits"][1]
        );
        assert_eq!(after.structured["data"]["hits"][0]["path"], "a/b.yml");
        assert_eq!(after.structured["summary"], before.structured["summary"]);
        // The size is the size of what is sent.
        let again = hub_mcp::output::cap::json_len(&after.structured)
            + hub_mcp::output::cap::json_len(&[rmcp::model::ContentBlock::text(after.content.as_str())]);
        assert_eq!(after.bytes, again, "{mode:?}");
    }
}

#[test]
fn the_sweep_leaves_a_result_with_nothing_to_replace_exactly_as_it_was() {
    for mode in MODES {
        let honest = render(
            &shape(mode),
            Parts {
                summary: "Swimlane sit1: 3 files shown.".to_owned(),
                data: json!({
                    "files": [{
                        "path": "tx-infinity-api/tx-infinity-core-sit1.yml",
                        "hash": hash_of(b"x").to_string(),
                        "size": 1234,
                        "drift": "in_sync",
                        "when": "2026-10-15T16:00:00Z",
                        "id": "11111111-2222-4333-8444-555555555555:aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                        "setting": "spring.datasource.hikari[0].maximum-pool-size",
                        "text": "name: one\nlevel: 2\n",
                    }],
                }),
                truncated: true,
                // A cursor and a link look like random strings; they are the server's and are not swept.
                next: Some(
                    "m1.eyJ0IjoibGlzdF9maWxlcyIsImkiOiJRMjNwV3RGOHZMa1R6eFlSN2JOY1U1ZFhhSGpNd1BzIiwicyI6M30"
                        .to_owned(),
                ),
                web_link: Some(
                    "https://hub.example.test/swimlanes/sit1/files?path=Ab3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF"
                        .to_owned(),
                ),
            },
        );
        let swept = sweep(&Redactor::new(), mode, honest.clone()).unwrap();
        assert_eq!(swept.late, 0, "{mode:?}");
        assert_eq!(swept.rendered.structured, honest.structured, "{mode:?}");
        assert_eq!(swept.rendered.content, honest.content, "{mode:?}");
        assert_eq!(swept.rendered.bytes, honest.bytes, "{mode:?}");
    }
}

#[test]
fn the_sweep_is_idempotent_and_leaves_nothing_for_the_scan() {
    for mode in MODES {
        let secret = fake_github_token();
        let redactor = Redactor::new();
        let once = sweep(&redactor, mode, leaky(mode, &secret)).unwrap().rendered;
        let twice = sweep(&redactor, mode, once.clone()).unwrap();
        assert_eq!(twice.late, 0, "{mode:?}");
        assert_eq!(twice.rendered.structured, once.structured, "{mode:?}");
        assert_eq!(twice.rendered.content, once.content, "{mode:?}");
        let mut all = Vec::new();
        strings(&once.structured["data"], &mut all);
        all.push(once.content.clone());
        for text in all {
            assert!(redactor.scan(&text).unwrap().is_empty(), "{mode:?}: {text}");
        }
    }
}

#[test]
fn the_sweep_fails_closed_when_it_cannot_scan() {
    // A string over what the scan takes (2 MiB) is not passed through: there is no result.
    let huge = "a".repeat(2 * 1024 * 1024 + 1);
    let rendered = render(
        &shape(ContentMode::Summary),
        Parts {
            summary: "x".to_owned(),
            data: json!({"text": huge}),
            truncated: false,
            next: None,
            web_link: None,
        },
    );
    assert!(sweep(&Redactor::new(), ContentMode::Summary, rendered).is_err());
}

#[tokio::test]
async fn late_sweep_catches_a_field_the_typed_path_missed() {
    // The message of a finding is a plain `ShortText` that the registry gave: the typed path does not scan it. A buggy or hostile
    // registry that put a key in it is caught by the sweep, which counts it as the defect it is.
    for mode in MODES {
        let rig = rig_in(mode);
        let secret = fake_github_token();
        let mut f = finding(1, FindingKind::Drift, "sit1", Severity::High);
        f.message = st(&format!("the value {secret} differs"));
        rig.registry.inner.add_finding(f);
        let mut c = signed_in(&rig, "alice", Role::Viewer).await;
        let called = call(&mut c, "find_inconsistencies", &json!({})).await;
        called.ok();
        let seen = everything(&called);
        assert!(!seen.contains(&secret), "{mode:?}: the key is in the result");
        assert!(!seen.contains(&secret["ghp_".len()..]), "{mode:?}");
        assert!(
            seen.contains("[REDACTED: 40 chars, github_token]"),
            "{mode:?}: {seen}"
        );
        assert_eq!(rig.state.metrics().late_redactions(), 1, "{mode:?}");
        // The rest of the finding is as it was.
        assert_eq!(called.ok()["findings"][0]["severity"], "high");
    }
}

#[tokio::test]
async fn the_sweep_changes_nothing_in_the_results_the_other_tools_tests_check() {
    // Every tool test in this binary checks its result against the contract's schema and for the values it expects; the sweep runs
    // on all of them. This one makes the point in the open: calls with no key anywhere replace nothing and count nothing.
    let rig = rig_in(ContentMode::DataBlock);
    seed(&rig, "sit1", &[(PATH, "a: 1\nb: two\n")]);
    rig.registry
        .inner
        .set_drift(&sid("sit1"), vec![entry(PATH, domain::DriftState::NfsAhead)]);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    for (tool, args) in [
        ("list_swimlanes", json!({})),
        ("list_files", json!({"swimlane": "sit1"})),
        ("get_drift", json!({"swimlane": "sit1"})),
        ("read_file", json!({"swimlane": "sit1", "path": PATH})),
    ] {
        call(&mut c, tool, &args).await.ok();
    }
    assert_eq!(rig.state.metrics().late_redactions(), 0);
}

// ---------------------------------------------------------------------------------------------------------------------
// No input asks for a flagged value
// ---------------------------------------------------------------------------------------------------------------------

/// Every property name of a schema, at any depth (`properties`, `$defs`, `oneOf`, `items`), with its description.
fn property_names(schema: &Value, out: &mut Vec<(String, String)>) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Object(props)) = map.get("properties") {
                for (name, def) in props {
                    let description = def["description"].as_str().unwrap_or_default().to_owned();
                    out.push((name.clone(), description));
                }
            }
            map.values().for_each(|v| property_names(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| property_names(v, out)),
        _ => {}
    }
}

#[test]
fn no_tool_input_can_reveal_a_flagged_value() {
    // Revealing a value is an audited action of the web UI (D79). No tool has an argument that asks for it: not by a name that
    // says so, and not by a description that says so. The contract is the one to change if that ever has to be so.
    const WORDS: [&str; 13] = [
        "reveal",
        "unmask",
        "unredact",
        "redact",
        "secret",
        "plain",
        "raw",
        "verbatim",
        "unfiltered",
        "password",
        "decrypt",
        "cleartext",
        "credential",
    ];
    let contract = crate::tools_rig::contract();
    assert_eq!(contract.tools().len(), ToolName::ALL.len());
    let mut checked = 0;
    for tool in contract.tools() {
        let mut names = Vec::new();
        property_names(&Value::Object((**tool.input()).clone()), &mut names);
        for (name, description) in names {
            checked += 1;
            let lower = name.to_ascii_lowercase();
            for word in WORDS {
                assert!(
                    !lower.contains(word),
                    "{}: the input `{name}` asks for `{word}`",
                    tool.name()
                );
            }
            let lower = description.to_ascii_lowercase();
            for phrase in [
                "reveal",
                "unredacted",
                "without redaction",
                "show the secret",
                "unmask",
            ] {
                assert!(
                    !lower.contains(phrase),
                    "{}: the input `{name}` is described with `{phrase}`: {description}",
                    tool.name()
                );
            }
        }
    }
    assert!(checked > 100, "only {checked} input properties were checked");
}
