//! Plan 07, T6 (S14b, D79): a denied file (`*.jks`, `*.p12`, `*.pfx`, `*.pem`, `*.key`, `*.keystore`, `*private*`) is known by name,
//! size and hash only. The agent never sends its content; the hub must not show it even if a port did. These tests give every file
//! tool a denied file whose "content" is a marker that is not shaped like a secret (so the secret scan could not remove it) and check
//! that the marker is nowhere in the answer.
//!
//! A file is denied when the port says so (its class, or the flag that withholds it), or when its name matches the built-in globs of
//! the engine. The second is the floor: a port that forgot, or an agent with a shorter list, does not make a key readable.
//!
//! The matrix is complete by construction: `every_tool_is_in_the_matrix_or_says_why_a_denied_file_cannot_reach_it` fails when a tool
//! is in neither list.

use bytes::Bytes;
use domain::{
    ChangeKind, DriftState, FileClass, FileContent, Grid, GridCell, GridRow, Paged, Role, SettingHit,
    TextHit, TextResults, TreeCompareEntry, TreeComparison,
};
use hub_mcp::tools::ToolName;
use serde_json::{Value, json};

use crate::fix::*;
use crate::read_tools::{comparison, effective_fixture, served_fixture};
use crate::rig::Rig;
use crate::support::elicitation_capabilities;
use crate::tools_rig::{Called, call, signed_in, signed_in_with};

/// What a denied file would hold. Not secret-shaped, so only the denial can keep it out.
const MARKER: &str = "DENIED-CONTENT-MARKER-5c1f";

/// Denied by name alone, in the shapes the globs cover: an extension, a part of a name at any depth, and upper case.
const DENIED_NAMES: [&str; 5] = [
    "keys/server.pem",
    "keys/store.JKS",
    "tx-core/private/app.yml",
    "tx-core/app-private.yml",
    "KEYS/SERVER.KEY",
];

fn rig() -> Rig {
    Rig::oauth()
}

fn seed(rig: &Rig, swimlane: &str, files: &[(&str, &str)]) {
    let files: Vec<_> = files.iter().map(|(p, t)| (np(p), bytes(t))).collect();
    assert!(rig.registry.inner.seed(&sid(swimlane), &files));
}

fn nothing_of_it(called: &Called) {
    let seen = called.response.to_string();
    assert!(
        !seen.contains(MARKER),
        "{}: the content of a denied file is in the answer: {seen}",
        called.tool
    );
}

fn content_of(path: &str, class: FileClass, withheld: bool) -> FileContent {
    FileContent {
        path: np(path),
        hash: hash_of(MARKER.as_bytes()),
        class,
        format: file_format(),
        size: 4321,
        bytes: Some(Bytes::from(format!("password: {MARKER}\n"))),
        withheld,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn denied_file_returns_content_withheld_for_every_source() {
    let sources = [
        json!({}),
        json!({"source": "nfs"}),
        json!({"source": "baseline"}),
        json!({"source": "git", "repo": "base", "ref": "v1.0.0"}),
    ];
    // How the file comes to be denied: the class the port gives, the flag the port gives, or its name alone with a port that says
    // it is an ordinary text file and sends the bytes.
    let mut cases: Vec<(String, FileContent)> = vec![
        (
            "keys/class.p12".to_owned(),
            content_of("keys/class.p12", FileClass::Denied, false),
        ),
        (
            "tx-core/flag.yml".to_owned(),
            content_of("tx-core/flag.yml", FileClass::Structured, true),
        ),
    ];
    for name in DENIED_NAMES {
        cases.push((name.to_owned(), content_of(name, FileClass::Structured, false)));
    }
    for (path, content) in cases {
        for source in &sources {
            let rig = rig();
            seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
            rig.registry
                .set_content(&sid("sit1"), &np(&path), content.clone());
            let mut c = signed_in(&rig, "alice", Role::Viewer).await;
            let mut args = json!({"swimlane": "sit1", "path": path});
            args.as_object_mut()
                .unwrap()
                .extend(source.as_object().unwrap().clone());
            let called = call(&mut c, "read_file", &args).await;
            let data = called.ok();
            let label = format!("{path} {source}");
            assert_eq!(data["content_withheld"], true, "{label}");
            assert_eq!(data["text"], "content withheld", "{label}");
            assert_eq!(data["class"], "denied", "{label}: the class says why");
            assert_eq!(data["size"], 4321, "{label}: the size stays");
            assert_eq!(
                data["hash"],
                hash_of(MARKER.as_bytes()).to_string(),
                "{label}: the hash stays"
            );
            assert_eq!(data["total_lines"], Value::Null, "{label}");
            assert!(data["outline"].as_array().unwrap().is_empty(), "{label}");
            nothing_of_it(&called);
        }
    }
}

#[tokio::test]
async fn a_file_that_is_not_denied_is_read_as_before() {
    // The floor must not catch what the globs do not: a name that merely contains a word, an extension that is not listed.
    let rig = rig();
    seed(
        &rig,
        "sit1",
        &[
            ("tx-core/privacy.yml", "a: 1\n"),
            ("tx-core/app.key.yml", "b: 2\n"),
            ("tx-core/keystore.yml", "c: 3\n"),
        ],
    );
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    for (path, text) in [
        ("tx-core/privacy.yml", "a: 1"),
        ("tx-core/app.key.yml", "b: 2"),
        ("tx-core/keystore.yml", "c: 3"),
    ] {
        let called = call(&mut c, "read_file", &json!({"swimlane": "sit1", "path": path})).await;
        assert_eq!(called.ok()["content_withheld"], false, "{path}");
        assert_eq!(called.ok()["text"].as_str().unwrap().trim_end(), text, "{path}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// get_served_config, get_effective_config
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_served_config_withholds_a_denied_file_and_lists_nothing_else_of_it() {
    // By the flag of the port, with the text sent anyway; and by name alone.
    for (file, withheld) in [
        ("app.yml", true),
        ("server.pem", false),
        ("private/app.yml", false),
        ("S.KEY", false),
    ] {
        let rig = rig();
        seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
        let mut served = served_fixture(Some(&format!("password: {MARKER}\n")), withheld);
        served.file = np(file);
        served.unresolved_placeholders = vec![st("${SOMETHING}")];
        rig.registry.inner.set_served(served);
        let mut c = signed_in(&rig, "alice", Role::Viewer).await;
        let called = call(
            &mut c,
            "get_served_config",
            &json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": file}),
        )
        .await;
        let data = called.ok();
        assert_eq!(data["content_withheld"], true, "{file}");
        assert_eq!(data["text"], Value::Null, "{file}");
        // The contract: a denied file reveals at most its name, size and hash, so no sources and no placeholders.
        assert!(data["sources"].as_array().unwrap().is_empty(), "{file}: {data}");
        assert!(
            data["unresolved_placeholders"].as_array().unwrap().is_empty(),
            "{file}: {data}"
        );
        nothing_of_it(&called);
    }
}

#[tokio::test]
async fn get_effective_config_refuses_a_denied_file_before_asking_the_port() {
    for name in DENIED_NAMES {
        let rig = rig();
        seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
        let mut effective = effective_fixture(MARKER);
        effective.file = lf(name);
        rig.registry.inner.set_effective(effective);
        let mut c = signed_in(&rig, "alice", Role::Viewer).await;
        let called = call(
            &mut c,
            "get_effective_config",
            &json!({"swimlane": "sit1", "file": name, "tenant": "sit1"}),
        )
        .await;
        assert_eq!(called.err(), "unprocessable", "{name}");
        assert_eq!(called.error()["reason"], "denied", "{name}");
        assert!(
            rig.registry.log.of("effective_for").is_empty(),
            "{name}: the port was asked"
        );
        nothing_of_it(&called);
    }
}

#[tokio::test]
async fn get_effective_config_refuses_a_value_that_comes_from_a_denied_file() {
    // The file asked for is fine, but the port says one of its values comes from a denied source: not shown, and not half shown.
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let mut effective = effective_fixture("plain");
    effective.settings[0].value = MARKER.to_owned();
    effective.settings[0].source = np("tx-core/app-private.yml");
    rig.registry.inner.set_effective(effective);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let called = call(
        &mut c,
        "get_effective_config",
        &json!({"swimlane": "sit1", "file": "tx-core/app.yml", "tenant": "sit1"}),
    )
    .await;
    assert_eq!(called.err(), "unprocessable");
    assert_eq!(called.error()["reason"], "denied");
    nothing_of_it(&called);
}

// ---------------------------------------------------------------------------------------------------------------------
// compare, compare_tree, compare_grid
// ---------------------------------------------------------------------------------------------------------------------

fn comparison_with_marker(class: FileClass, file: &str) -> domain::Comparison {
    let mut c = comparison(class);
    c.file = Some(lf(file));
    c.setting_changes = vec![change(
        "password",
        ChangeKind::Changed,
        Some(MARKER),
        Some("rotated"),
    )];
    c.hunks[0].lines[1].text = format!("password: {MARKER}");
    c.binary = Some(domain::BinaryCompare {
        left: Some(hash_of(b"left")),
        right: Some(hash_of(b"right")),
        left_size: Some(10),
        right_size: Some(11),
    });
    c
}

#[tokio::test]
async fn compare_shows_a_denied_file_by_name_size_and_hash_only() {
    // By the class of the port, and by name alone with a port that sends a diff of an ordinary structured file.
    let mut cases = vec![("keys/class.p12".to_owned(), FileClass::Denied)];
    cases.extend(
        DENIED_NAMES
            .iter()
            .map(|n| ((*n).to_owned(), FileClass::Structured)),
    );
    for (file, class) in cases {
        for detail in ["settings", "lines", "auto"] {
            let rig = rig();
            seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
            rig.registry
                .inner
                .set_comparison(comparison_with_marker(class, &file));
            let mut c = signed_in(&rig, "alice", Role::Viewer).await;
            let called = call(
                &mut c,
                "compare",
                &json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": file, "detail": detail}),
            )
            .await;
            let data = called.ok();
            let label = format!("{file} {detail}");
            assert_eq!(data["class"], "denied", "{label}");
            assert!(data["setting_changes"].as_array().unwrap().is_empty(), "{label}");
            assert!(data["hunks"].as_array().unwrap().is_empty(), "{label}");
            // Nothing is claimed about content that is not shown.
            assert_eq!(data["setting_change_count"], 0, "{label}");
            assert_eq!(data["hunk_count"], 0, "{label}");
            // The hashes and sizes stay: that is what a denied file is known by.
            assert_eq!(data["binary"]["left"], hash_of(b"left").to_string(), "{label}");
            nothing_of_it(&called);
        }
    }
}

#[tokio::test]
async fn compare_tree_lists_a_denied_file_by_name_and_state_only() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    rig.registry.inner.set_tree_comparison(TreeComparison {
        entries: Paged::last(vec![
            TreeCompareEntry {
                file: lf("keys/server.pem"),
                state: domain::CompareState::Different,
            },
            TreeCompareEntry {
                file: lf("a.yml"),
                state: domain::CompareState::LeftOnly,
            },
        ]),
        skipped_identical_subtrees: 0,
    });
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let called = call(
        &mut c,
        "compare_tree",
        &json!({"left": "nfs:sit1", "right": "nfs:sit2"}),
    )
    .await;
    let data = called.ok();
    // The entry carries a path and a state, and nothing else a file could hold.
    assert_eq!(
        data["entries"][0],
        json!({"file": "keys/server.pem", "state": "different"})
    );
    assert_eq!(data["entries"].as_array().unwrap().len(), 2);
    nothing_of_it(&called);
}

#[tokio::test]
async fn compare_grid_refuses_a_denied_file_before_asking_the_port() {
    for name in DENIED_NAMES {
        let rig = rig();
        seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
        let cell = GridCell {
            value: Some(value(MARKER)),
            location: Some(location("password", 1, 1)),
        };
        rig.registry.inner.set_grid(Grid {
            swimlanes: vec![sid("sit1")],
            rows: Paged::last(vec![GridRow {
                path: sp("password"),
                cells: vec![cell],
            }]),
        });
        let mut c = signed_in(&rig, "alice", Role::Viewer).await;
        let called = call(
            &mut c,
            "compare_grid",
            &json!({"file": name, "swimlanes": ["sit1"], "only_differing": false}),
        )
        .await;
        assert_eq!(called.err(), "unprocessable", "{name}");
        assert_eq!(called.error()["reason"], "denied", "{name}");
        assert!(
            rig.registry.log.of("grid").is_empty(),
            "{name}: the port was asked"
        );
        nothing_of_it(&called);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// search_settings, search_text
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn search_settings_drops_hits_in_denied_files_and_pages_exactly() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let hit = |file: &str, n: u32, text: &str| SettingHit {
        swimlane: sid("sit1"),
        file: np(file),
        path: sp(&format!("timeout.t{n}")),
        value: value(text),
        location: location(&format!("timeout.t{n}"), n, n),
    };
    rig.registry.inner.set_setting_hits(vec![
        hit("a.yml", 1, "one"),
        hit("keys/server.pem", 2, MARKER),
        hit("tx-core/app-private.yml", 3, MARKER),
        hit("b.yml", 4, "four"),
        hit("KEYS/X.KEY", 5, MARKER),
        hit("c.yml", 6, "six"),
    ]);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    for limit in [1_u32, 2, 3, 6] {
        let mut files = Vec::new();
        let mut cursor = Value::Null;
        for _ in 0..20 {
            let mut args = json!({"q": "timeout", "limit": limit, "swimlanes": ["sit1"]});
            if !cursor.is_null() {
                args["cursor"] = cursor.clone();
            }
            let called = call(&mut c, "search_settings", &args).await;
            nothing_of_it(&called);
            for hit in called.ok()["hits"].as_array().unwrap() {
                files.push(hit["file"].as_str().unwrap().to_owned());
            }
            cursor = called.envelope()["next"].clone();
            if cursor.is_null() {
                break;
            }
        }
        assert!(cursor.is_null(), "limit {limit}: the paging did not end");
        assert_eq!(
            files,
            ["a.yml", "b.yml", "c.yml"],
            "limit {limit}: every other hit once, in order"
        );
    }
}

#[tokio::test]
async fn search_text_drops_hits_in_denied_files() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let hit = |file: &str, line: u32, text: &str| TextHit {
        swimlane: sid("sit1"),
        path: np(file),
        line,
        text: text.to_owned(),
        ranges: vec![(0, 4)],
    };
    rig.registry.inner.set_text_results(TextResults {
        hits: vec![
            hit("a.yml", 1, "plain a"),
            hit("keys/server.key", 2, &format!("password {MARKER}")),
            hit("tx-core/private/app.yml", 3, &format!("password {MARKER}")),
            hit("b.yml", 4, "plain b"),
        ],
        truncated: false,
    });
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let called = call(&mut c, "search_text", &json!({"pattern": "password"})).await;
    let files: Vec<&str> = called.ok()["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["a.yml", "b.yml"]);
    nothing_of_it(&called);
}

// ---------------------------------------------------------------------------------------------------------------------
// list_files, get_drift, get_history
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn list_files_and_get_drift_show_a_denied_file_by_name_size_and_hash() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let mut by_class = entry("keys/class.p12", DriftState::NfsAhead);
    by_class.class = FileClass::Denied;
    // A port that forgot: the class says structured and nothing is withheld, but the name is a key's.
    let mut by_name = entry("keys/forgotten.pem", DriftState::NfsAhead);
    by_name.class = FileClass::Structured;
    by_name.content_withheld = false;
    rig.registry.inner.set_drift(
        &sid("sit1"),
        vec![by_class, by_name, entry("plain/app.yml", DriftState::NfsAhead)],
    );
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let drift = call(&mut c, "get_drift", &json!({"swimlane": "sit1"})).await;
    let listed = call(
        &mut c,
        "list_files",
        &json!({"swimlane": "sit1", "drift": "nfs_ahead"}),
    )
    .await;
    for called in [drift, listed] {
        let files = called.ok()["files"].as_array().unwrap().clone();
        assert_eq!(files.len(), 3, "{}", called.tool);
        for f in &files[..2] {
            assert_eq!(f["class"], "denied", "{}: {f}", called.tool);
            assert_eq!(f["content_withheld"], true, "{}: {f}", called.tool);
            assert_eq!(f["size"], 1234, "{}: the size stays", called.tool);
            assert!(f["hash"].is_string(), "{}: the hash stays", called.tool);
        }
        assert_eq!(files[2]["class"], "structured", "{}", called.tool);
        assert_eq!(files[2]["content_withheld"], false, "{}", called.tool);
    }
}

#[tokio::test]
async fn get_history_of_a_denied_file_lists_hashes_and_nothing_else() {
    let rig = rig();
    seed(&rig, "sit1", &[("keys/server.pem", MARKER)]);
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let called = call(
        &mut c,
        "get_history",
        &json!({"swimlane": "sit1", "path": "keys/server.pem"}),
    )
    .await;
    let versions = called.ok()["versions"].as_array().unwrap().clone();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["hash"], hash_of(MARKER.as_bytes()).to_string());
    // The fake keeps the bytes; the answer never carries them, denied or not.
    nothing_of_it(&called);
}

// ---------------------------------------------------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn writes_to_a_denied_file_are_refused_before_anything_is_asked() {
    for name in DENIED_NAMES {
        // A client that can be asked, and one that cannot (which would be given a link to the web UI).
        for capabilities in [elicitation_capabilities(), json!({})] {
            let rig = rig();
            rig.writes
                .inner
                .seed(&sid("sit1"), &[(np("a.yml"), bytes("a: 1\n"))]);
            let mut c = signed_in_with(&rig, "ed", Role::Editor, &capabilities).await;
            let hash = hash_of(b"a: 1\n").to_string();
            for (tool, args) in [
                (
                    "propose_change",
                    json!({"swimlane": "sit1", "path": name, "expected": {"state": "absent"}, "content": MARKER}),
                ),
                (
                    "upload_file",
                    json!({"swimlane": "sit1", "path": name, "expected": {"state": "absent"}, "content": MARKER}),
                ),
                (
                    "delete_file",
                    json!({"swimlane": "sit1", "path": name, "expected_hash": hash}),
                ),
                (
                    "raise_pr",
                    json!({"swimlane": "sit1", "paths": ["a.yml", name], "title": "Rotate"}),
                ),
            ] {
                let called = call(&mut c, tool, &args).await;
                assert_eq!(called.err(), "unprocessable", "{tool} {name}");
                assert_eq!(called.error()["reason"], "denied", "{tool} {name}");
                nothing_of_it(&called);
            }
            assert!(
                rig.writes.log.all().is_empty(),
                "{name}: the write service was called: {:?}",
                rig.writes.log.all()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Completeness
// ---------------------------------------------------------------------------------------------------------------------

/// The tools that take or return a file of the swimlane, each with its test above.
const FILE_TOOLS: [&str; 15] = [
    "read_file",
    "get_served_config",
    "get_effective_config",
    "compare",
    "compare_tree",
    "compare_grid",
    "search_settings",
    "search_text",
    "list_files",
    "get_drift",
    "get_history",
    "propose_change",
    "upload_file",
    "delete_file",
    "raise_pr",
];

/// The rest, and why a denied file cannot reach them: they answer with swimlanes, services, findings (a sentence of the engine, never
/// a value) and drafts by id, or they read the docs (D79 is about the files of the swimlanes, and the docs are not on NFS), or they
/// are outcomes only. `apply_change` takes a draft that the write service made, and it refuses to make one for a denied file
/// (`propose_change` refuses first, above).
const OTHER_TOOLS: [&str; 12] = [
    "list_swimlanes",
    "get_swimlane",
    "get_pending_restarts",
    "find_inconsistencies",
    "search_docs",
    "read_doc",
    "grep_docs",
    "get_docs_for_file",
    "get_feature_status",
    "apply_change",
    "restart_service",
    "notify_services",
];

#[test]
fn every_tool_is_in_the_matrix_or_says_why_a_denied_file_cannot_reach_it() {
    let mut accounted: Vec<&str> = FILE_TOOLS.iter().chain(OTHER_TOOLS.iter()).copied().collect();
    accounted.sort_unstable();
    let before = accounted.len();
    accounted.dedup();
    assert_eq!(before, accounted.len(), "a tool is listed twice");
    let mut all: Vec<&str> = ToolName::ALL.iter().map(|t| t.as_str()).collect();
    all.sort_unstable();
    assert_eq!(accounted, all, "a tool of the contract is in neither list");
}
