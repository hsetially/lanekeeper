//! D58, P9: a result is at most 16 KB (a read tool) or 4 KB (a write tool), counted over `structuredContent` and `content`; a list
//! that is cut says so and says how to get the rest, and paging through a cut list loses and repeats nothing.

use domain::{
    AttributionSource, ChangeKind, Confidence, DiffHunk, DiffLine, DiffLineKind, DriftState, FindingKind,
    GridCell, GridRow, Paged, Role, SettingHit, Severity, TextHit, TextResults, TreeCompareEntry,
    TreeComparison,
};
use proptest::prelude::*;
use serde_json::{Value, json};

use crate::fix::*;
use crate::rig::Rig;
use crate::tools_rig::{call, signed_in};

const CAP: usize = 16 * 1024;

fn long(n: usize) -> String {
    "x".repeat(n)
}

fn long_path(n: usize) -> String {
    // Four components of at most 255 bytes.
    let part = "d".repeat(n.min(250));
    format!("{part}/{part}/{part}/{}.yml", &part[..part.len().min(200)])
}

fn seed(rig: &Rig, swimlane: &str, files: &[(&str, &str)]) {
    let files: Vec<_> = files.iter().map(|(p, t)| (np(p), bytes(t))).collect();
    assert!(rig.registry.inner.seed(&sid(swimlane), &files));
}

#[tokio::test]
async fn no_read_tool_exceeds_16kb_at_worst_case_items() {
    let rig = Rig::oauth();
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    let secret_free_long = long(2000);

    // list_swimlanes: 200 swimlanes with the longest ids.
    for n in 0..150 {
        seed(&rig, &format!("{}{n:03}", "s".repeat(60)), &[("a.yml", "a: 1\n")]);
    }
    // The rest of the fixtures use one swimlane.
    seed(
        &rig,
        "sit1",
        &[
            ("big.yml", &"k: v\n".repeat(6000)),
            ("wide.txt", &format!("{}\n", long(5000)).repeat(300)),
        ],
    );
    rig.registry.inner.set_pending_restarts(
        &sid("sit1"),
        (0..200)
            .map(|n| {
                let mut s = service(
                    "apps",
                    &format!("svc-{n:03}"),
                    domain::PickupState::NeedsNotifyOrRestart,
                );
                s.pending_files = (0..50).map(|_| np(&long_path(250))).collect();
                s
            })
            .collect(),
    );
    let drifted: Vec<_> = (0..400)
        .map(|n| {
            let mut e = entry(
                &format!("{}/{n:03}.yml", long_path(200).trim_end_matches(".yml")),
                DriftState::NfsAhead,
            );
            e.attribution = Some(attribution(
                AttributionSource::SentinelLogin,
                Confidence::High,
                Some(&"u".repeat(200)),
            ));
            e.severity = Some(Severity::Critical);
            e
        })
        .collect();
    rig.registry.inner.set_drift(&sid("sit1"), drifted);
    for n in 0..200 {
        rig.registry
            .inner
            .add_finding(finding(n, FindingKind::Drift, "sit1", Severity::High));
    }
    for n in 0..100 {
        rig.registry
            .inner
            .observe(&sid("sit1"), &np("big.yml"), bytes(&format!("k: {n}\n")));
    }
    rig.registry.inner.set_setting_hits(
        (0..200)
            .map(|n| SettingHit {
                swimlane: sid("sit1"),
                file: np(&long_path(250)),
                path: sp(&format!("{}{n}", long(900))),
                value: value(&secret_free_long),
                location: location("a.b", 1, 2),
                logical_file: None,
                tenant: None,
            })
            .collect(),
    );
    rig.registry.inner.set_text_results(TextResults {
        hits: (0..500)
            .map(|n| TextHit {
                swimlane: sid("sit1"),
                path: np(&long_path(250)),
                line: n + 1,
                text: secret_free_long.clone(),
                ranges: vec![(0, 5); 20],
                logical_file: None,
                tenant: None,
            })
            .collect(),
        truncated: true,
    });
    let hunk = DiffHunk {
        old_start: 1,
        old_lines: 400,
        new_start: 1,
        new_lines: 400,
        lines: (0..400)
            .map(|_| DiffLine {
                kind: DiffLineKind::Added,
                text: long(1000),
                inline_changes: Vec::new(),
            })
            .collect(),
    };
    rig.registry.inner.set_comparison(domain::Comparison {
        left: "nfs:sit1".parse().unwrap(),
        right: "nfs:sit2".parse().unwrap(),
        file: Some(lf("a.yml")),
        class: domain::FileClass::Structured,
        identical: false,
        eol_only: false,
        hunks: vec![hunk; 20],
        setting_changes: (0..100)
            .map(|n| {
                let mut ch = change(&format!("p{n}"), ChangeKind::Changed, Some("a"), Some("b"));
                ch.left = Some(value(&secret_free_long));
                ch.right = Some(value(&secret_free_long));
                ch
            })
            .collect(),
        binary: None,
    });
    rig.registry.inner.set_tree_comparison(TreeComparison {
        entries: Paged::last(
            (0..400)
                .map(|n| TreeCompareEntry {
                    file: lf(&format!("{}/{n}.yml", long_path(200).trim_end_matches(".yml"))),
                    state: domain::CompareState::Different,
                })
                .collect(),
        ),
        skipped_identical_subtrees: 1,
    });
    let lanes: Vec<String> = (1..=64).map(|n| format!("sit{n}")).collect();
    for lane in &lanes[1..] {
        seed(&rig, lane, &[("a.yml", "a: 1\n")]);
    }
    rig.registry.inner.set_grid(domain::Grid {
        swimlanes: lanes.iter().map(|l| sid(l)).collect(),
        rows: Paged::last(
            (0..200)
                .map(|n| GridRow {
                    path: sp(&format!("{}{n}", long(300))),
                    cells: (0..64)
                        .map(|_| GridCell {
                            value: Some(value(&secret_free_long)),
                            location: Some(location(&long(300), 1, 1)),
                        })
                        .collect(),
                })
                .collect(),
        ),
    });
    rig.registry.inner.set_effective(domain::EffectiveConfig {
        swimlane: sid("sit1"),
        file: lf("a.yml"),
        tenant: tid("sit1"),
        channel: None,
        role: domain::FileRole::PropertySource,
        hash: hash_of(b"x"),
        sources: (1..=16)
            .map(|r| domain::EffectiveSource {
                path: np(&long_path(250)),
                rank: r,
            })
            .collect(),
        settings: (0..1000)
            .map(|n| domain::EffectiveSetting {
                path: sp(&format!("{}{n}", long(500))),
                value: secret_free_long.clone(),
                source: np(&long_path(250)),
                location: Some(location("a.b", 1, 2)),
            })
            .collect(),
        unresolved_placeholders: (0..200).map(|n| st(&format!("${{{}{n}}}", long(200)))).collect(),
    });
    rig.registry.inner.set_served(domain::ServedFile {
        swimlane: sid("sit1"),
        application: domain::AppName::parse("tx-core").unwrap(),
        tenant: tid("sit1"),
        channel: None,
        file: np("a.xsl"),
        role: domain::FileRole::Resource,
        hash: hash_of(b"served"),
        content_withheld: false,
        text: Some(format!("{}\n", long(1000)).repeat(2000)),
        sources: Vec::new(),
        unresolved_placeholders: (0..200).map(|n| st(&format!("${{{}{n}}}", long(200)))).collect(),
    });
    for n in 0..50 {
        rig.docs.inner.seed(
            domain::DocPath::parse(&format!("/git/main/{}{n}.md", long(100))).unwrap(),
            &long(200),
            &format!("timeout {}\n", long(1500)).repeat(30),
        );
    }
    rig.docs.inner.set_feature_status(domain::FeatureStatus {
        flags: (0..50)
            .map(|n| domain::FlagStatus {
                flag: st(&format!("{}{n}", long(200))),
                file: lf(&long_path(250)),
                channel: None,
                setting: sp(&long(900)),
                template_default: Some(value(&secret_free_long)),
                values: (0..64)
                    .map(|i| domain::FlagValue {
                        swimlane: sid(&format!("sit{}", i + 1)),
                        value: Some(value(&secret_free_long)),
                    })
                    .collect(),
            })
            .collect(),
    });

    let calls: Vec<(&str, Value)> = vec![
        ("list_swimlanes", json!({"limit": 200})),
        ("get_swimlane", json!({"swimlane": "sit1"})),
        (
            "list_files",
            json!({"swimlane": "sit1", "drift": "nfs_ahead", "limit": 200}),
        ),
        ("read_file", json!({"swimlane": "sit1", "path": "big.yml"})),
        (
            "read_file",
            json!({"swimlane": "sit1", "path": "wide.txt", "lines": {"start": 1, "end": 200}}),
        ),
        (
            "get_effective_config",
            json!({"swimlane": "sit1", "file": "a.yml", "tenant": "sit1"}),
        ),
        (
            "get_served_config",
            json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": "a.xsl"}),
        ),
        (
            "compare",
            json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": "a.yml", "detail": "settings"}),
        ),
        (
            "compare",
            json!({"left": "nfs:sit1", "right": "nfs:sit2", "file": "a.yml", "detail": "lines"}),
        ),
        (
            "compare_tree",
            json!({"left": "nfs:sit1", "right": "nfs:sit2", "limit": 200}),
        ),
        (
            "compare_grid",
            json!({"file": "a.yml", "swimlanes": lanes, "limit": 200, "only_differing": false}),
        ),
        ("search_settings", json!({"q": "x", "limit": 200})),
        ("search_text", json!({"pattern": "x", "max_total": 500})),
        ("find_inconsistencies", json!({"limit": 200})),
        ("get_drift", json!({"swimlane": "sit1", "limit": 200})),
        ("get_pending_restarts", json!({"swimlane": "sit1", "limit": 200})),
        (
            "get_history",
            json!({"swimlane": "sit1", "path": "big.yml", "limit": 200}),
        ),
        ("search_docs", json!({"q": "timeout", "limit": 50})),
        (
            "read_doc",
            json!({"path": format!("/git/main/{}0.md", long(100))}),
        ),
        (
            "grep_docs",
            json!({"pattern": "timeout", "context": 5, "limit": 100}),
        ),
        ("get_docs_for_file", json!({"file": "a.yml", "limit": 50})),
        ("get_feature_status", json!({})),
    ];
    for (tool, args) in calls {
        let called = call(&mut c, tool, &args).await;
        if called.is_error() {
            // A worst case may be too big for one item: an honest `unavailable`-free answer is still within the cap. The only
            // error allowed here is one the fixture explains (a tool whose fake has no data), and it is small too.
            assert!(
                called.bytes() <= CAP,
                "{tool}: an error of {} bytes",
                called.bytes()
            );
            continue;
        }
        let envelope = called.envelope();
        assert!(
            called.bytes() <= CAP,
            "{tool}: {} bytes (cap {CAP}): {args}",
            called.bytes()
        );
        // The summary is bounded as the contract says.
        assert!(
            envelope["summary"].as_str().unwrap().chars().count() <= 512,
            "{tool}"
        );
        eprintln!(
            "{tool}: {} bytes, truncated={}",
            called.bytes(),
            envelope["truncated"]
        );
    }
}

#[tokio::test]
async fn write_tools_stay_under_4kb() {
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.limits.write_burst = 100;
            s
        },
    );
    let mut c = signed_in(&rig, "root", Role::Admin).await;
    let big = long(900);
    for (tool, args) in [
        ("apply_change", json!({"draft_id": "d-1"})),
        (
            "upload_file",
            json!({"swimlane": "sit1", "path": long_path(250), "expected": {"state": "absent"}, "content": big}),
        ),
        (
            "delete_file",
            json!({"swimlane": "sit1", "path": long_path(250), "expected_hash": hash_of(b"x").to_string()}),
        ),
        (
            "raise_pr",
            json!({"swimlane": "sit1", "paths": (0..100).map(|_| long_path(250)).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>(), "title": long(250), "body": long(60000)}),
        ),
        (
            "restart_service",
            json!({"swimlane": "sit1", "service": {"namespace": "apps", "name": "core"}}),
        ),
    ] {
        let called = call(&mut c, tool, &args).await;
        called.ok();
        assert!(called.bytes() <= 4 * 1024, "{tool}: {}", called.bytes());
    }
}

#[tokio::test]
async fn truncated_sets_next_or_web_link() {
    let rig = Rig::oauth();
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    rig.registry.inner.set_text_results(TextResults {
        hits: (0..500)
            .map(|n| TextHit {
                swimlane: sid("sit1"),
                path: np("a.yml"),
                line: n + 1,
                text: long(400),
                ranges: Vec::new(),
                logical_file: None,
                tenant: None,
            })
            .collect(),
        truncated: false,
    });
    for n in 0..200 {
        rig.registry
            .inner
            .add_finding(finding(n, FindingKind::Drift, "sit1", Severity::High));
    }
    // A tool that pages: cut by the cap, it says `next`.
    let paged = call(&mut c, "find_inconsistencies", &json!({"limit": 200})).await;
    let envelope = paged.envelope();
    assert_eq!(envelope["truncated"], true);
    assert!(envelope["next"].is_string(), "{envelope}");
    assert!(paged.ok()["findings"].as_array().unwrap().len() < 200);
    // A tool that does not page: cut by the cap, it says `web_link`, and there is no cursor.
    let unpaged = call(&mut c, "search_text", &json!({"pattern": "x", "max_total": 500})).await;
    let envelope = unpaged.envelope();
    assert_eq!(envelope["truncated"], true);
    assert_eq!(envelope["next"], Value::Null);
    assert!(
        envelope["web_link"]
            .as_str()
            .unwrap()
            .starts_with("https://hub.example.test/"),
        "{envelope}"
    );
    // Not cut: neither.
    rig.registry.inner.set_text_results(TextResults {
        hits: vec![TextHit {
            swimlane: sid("sit1"),
            path: np("a.yml"),
            line: 1,
            text: "x".into(),
            ranges: Vec::new(),
            logical_file: None,
            tenant: None,
        }],
        truncated: false,
    });
    let whole = call(&mut c, "search_text", &json!({"pattern": "x", "max_total": 1})).await;
    assert_eq!(whole.envelope()["truncated"], false);
    assert_eq!(whole.envelope()["web_link"], Value::Null);
}

#[tokio::test]
async fn read_file_default_is_200_lines_or_16kb() {
    let rig = Rig::oauth();
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    // 300 short lines: 200 lines come first.
    let short: String = (1..=300).map(|n| format!("r{n}\n")).collect();
    // 100 lines of 400 characters: 16 KB comes first.
    let wide: String = (1..=100).map(|n| format!("{n:03}{}\n", long(400))).collect();
    seed(&rig, "sit1", &[("short.txt", &short), ("wide.txt", &wide)]);
    let by_lines = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "short.txt"}),
    )
    .await;
    assert_eq!(by_lines.ok()["returned_lines"], json!({"start": 1, "end": 200}));
    assert_eq!(by_lines.envelope()["truncated"], true);
    let by_bytes = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "wide.txt"}),
    )
    .await;
    let end = by_bytes.ok()["returned_lines"]["end"].as_u64().unwrap();
    assert!((20..100).contains(&end), "{end}");
    assert!(by_bytes.bytes() <= CAP);
    assert_eq!(by_bytes.envelope()["truncated"], true);
    // The cap decided: what is left of it is less than one more line of 400 characters.
    assert!(CAP - by_bytes.bytes() < 420, "{} of {CAP} used", by_bytes.bytes());
}

#[tokio::test]
async fn an_item_too_big_for_the_cap_ends_the_page_with_a_link_not_a_loop() {
    // A cap so small that one finding does not fit: the page has no items, no cursor (there is no way forward from here), and a link.
    let rig = Rig::build(
        hub_mcp::config::AuthMode::Oauth,
        false,
        hub_mcp::auth::pat::TokenSettings::default(),
        |mut s| {
            s.output.read_cap_bytes = 600;
            s
        },
    );
    let mut c = signed_in(&rig, "alice", Role::Viewer).await;
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    for id in [1, 2] {
        let mut f = finding(id, FindingKind::Drift, "sit1", Severity::High);
        f.message = st(&long(250));
        rig.registry.inner.add_finding(f);
    }
    let called = call(&mut c, "find_inconsistencies", &json!({})).await;
    assert!(called.ok()["findings"].as_array().unwrap().is_empty());
    let envelope = called.envelope();
    assert_eq!(envelope["truncated"], true);
    assert_eq!(envelope["next"], Value::Null);
    assert!(envelope["web_link"].is_string());
    assert!(called.bytes() <= 600, "{}", called.bytes());
}

// ---------------------------------------------------------------------------------------------------------------------
// Exact paging
// ---------------------------------------------------------------------------------------------------------------------

/// All the ids of a paged tool, page after page, with the page size (and so the cut) different on every call.
async fn walk(
    rig: &Rig,
    tool: &str,
    base: &Value,
    field: &str,
    id_field: &str,
    limits: &[u32],
    cap: usize,
) -> Vec<String> {
    let mut c = signed_in(rig, "walker", Role::Viewer).await;
    let mut out = Vec::new();
    let mut cursor = Value::Null;
    for round in 0..1000 {
        let mut args = base.clone();
        args["limit"] = json!(limits[round % limits.len()]);
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let called = call(&mut c, tool, &args).await;
        let data = called.ok();
        assert!(
            called.bytes() <= cap,
            "{} bytes over the cap {cap}",
            called.bytes()
        );
        for item in data[field].as_array().unwrap() {
            out.push(match &item[id_field] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
        cursor = called.envelope()["next"].clone();
        if cursor.is_null() {
            return out;
        }
        assert_eq!(
            called.envelope()["truncated"],
            true,
            "a page with a next cursor is truncated"
        );
    }
    panic!("the walk did not end");
}

fn exact_paging_case(count: usize, message_lengths: &[usize], limits: &[u32], cap: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let rig = Rig::build(
            hub_mcp::config::AuthMode::Oauth,
            false,
            hub_mcp::auth::pat::TokenSettings::default(),
            |mut s| {
                s.output.read_cap_bytes = cap;
                // A walk is hundreds of calls from one user.
                s.limits.read_burst = 10_000;
                s.limits.read_per_minute = 60_000;
                s
            },
        );
        // Port-paged, each item a different size: findings.
        seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
        for n in 0..count {
            let mut f = finding(
                i64::try_from(n).unwrap() + 1,
                FindingKind::Drift,
                "sit1",
                Severity::High,
            );
            f.message = st(&"m".repeat(message_lengths[n % message_lengths.len()]));
            rig.registry.inner.add_finding(f);
        }
        let got = walk(
            &rig,
            "find_inconsistencies",
            &json!({}),
            "findings",
            "id",
            limits,
            cap,
        )
        .await;
        let expected: Vec<String> = (0..count).map(|n| (n + 1).to_string()).collect();
        assert_eq!(
            got, expected,
            "findings: every item once, in order (count {count}, cap {cap}, limits {limits:?})"
        );

        // Key-paged: swimlanes.
        for n in 0..count {
            seed(&rig, &format!("lane{n:03}"), &[("a.yml", "a: 1\n")]);
        }
        let got = walk(&rig, "list_swimlanes", &json!({}), "swimlanes", "id", limits, cap).await;
        let mut expected: Vec<String> = (0..count).map(|n| format!("lane{n:03}")).collect();
        expected.push("sit1".to_owned());
        expected.sort();
        assert_eq!(got, expected, "swimlanes: every item once, in order");
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn paging_is_exact_prop(
        count in 0..45_usize,
        message_lengths in proptest::collection::vec(1..120_usize, 1..9),
        limits in proptest::collection::vec(1..12_u32, 1..5),
        cap in 1_700..6_000_usize,
    ) {
        exact_paging_case(count, &message_lengths, &limits, cap);
    }
}

#[test]
fn paging_is_exact_at_the_edges() {
    // One item per page, as many as fit, a cap just over one item, and a limit bigger than the data.
    exact_paging_case(30, &[100], &[1], 1_700);
    exact_paging_case(30, &[100], &[200], 1_700);
    exact_paging_case(30, &[1, 119], &[7, 1, 3], 2_000);
    exact_paging_case(0, &[10], &[5], 1_700);
    exact_paging_case(1, &[10], &[1], 1_700);
}
