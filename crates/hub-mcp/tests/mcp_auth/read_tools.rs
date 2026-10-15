//! Plan 07, T2: the read tools, one by one, over the fakes of the ports. Every successful result is checked against its schema by
//! `Called::ok`, so each test is also a test that the result is what `docs/mcp-tools.md` says.

use domain::{
    AttributionSource, ChangeKind, Confidence, DiffHunk, DiffLine, DiffLineKind, DriftState, EffectiveConfig,
    EffectiveSetting, EffectiveSource, FileClass, FileContent, FindingKind, Grid, GridCell, GridRow, Paged,
    PickupState, Role, ServedFile, SettingHit, Severity, TextHit, TextResults, TreeCompareEntry,
    TreeComparison,
};
use serde_json::{Value, json};

use crate::fix::*;
use crate::rig::Rig;
use crate::tools_rig::{call, signed_in};

fn rig() -> Rig {
    Rig::oauth()
}

fn seed(rig: &Rig, swimlane: &str, files: &[(&str, &str)]) {
    let files: Vec<_> = files.iter().map(|(p, t)| (np(p), bytes(t))).collect();
    assert!(rig.registry.inner.seed(&sid(swimlane), &files));
}

async fn viewer(rig: &Rig) -> crate::support::Client {
    signed_in(rig, "alice", Role::Viewer).await
}

fn ids(data: &Value, key: &str, field: &str) -> Vec<String> {
    data[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v[field].as_str().unwrap().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------
// Swimlanes
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn list_swimlanes_pages_by_keyset() {
    let rig = rig();
    for n in 1..=5 {
        seed(&rig, &format!("sit{n}"), &[("a.yml", "x: 1\n")]);
    }
    let mut c = viewer(&rig).await;
    let first = call(&mut c, "list_swimlanes", &json!({"limit": 2})).await;
    assert_eq!(ids(first.ok(), "swimlanes", "id"), ["sit1", "sit2"]);
    let envelope = first.envelope();
    assert_eq!(envelope["truncated"], true);
    let next = envelope["next"].as_str().unwrap().to_owned();
    assert!(next.starts_with("m1."), "{next}");

    let second = call(&mut c, "list_swimlanes", &json!({"limit": 2, "cursor": next})).await;
    assert_eq!(ids(second.ok(), "swimlanes", "id"), ["sit3", "sit4"]);
    let next = second.envelope()["next"].as_str().unwrap().to_owned();

    let third = call(&mut c, "list_swimlanes", &json!({"limit": 2, "cursor": next})).await;
    assert_eq!(ids(third.ok(), "swimlanes", "id"), ["sit5"]);
    assert_eq!(third.envelope()["truncated"], false);
    assert_eq!(third.envelope()["next"], Value::Null);

    // A cursor belongs to the tool that made it; one that is made up is refused.
    let other_tool = call(
        &mut c,
        "get_drift",
        &json!({"swimlane": "sit1", "cursor": first.envelope()["next"]}),
    )
    .await;
    assert_eq!(other_tool.err(), "invalid_argument");
    for bad in ["abc", "m1.AAAA", "m1.", "m2.bGlzdF9zd2ltbGFuZXMKMAo"] {
        let called = call(&mut c, "list_swimlanes", &json!({"cursor": bad})).await;
        assert_eq!(called.err(), "invalid_argument", "{bad}");
    }
    // The summary states counts and no more.
    assert!(first.envelope()["summary"].as_str().unwrap().contains("2 of 5"));
}

#[tokio::test]
async fn get_swimlane_lists_services_and_auto_notify() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "x: 1\n")]);
    rig.registry.inner.set_pending_restarts(
        &sid("sit1"),
        vec![
            service("apps", "core", PickupState::NeedsNotifyOrRestart),
            service("apps", "api", PickupState::Live),
        ],
    );
    rig.registry.inner.set_auto_notify(&sid("sit1"), true);
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_swimlane", &json!({"swimlane": "sit1"})).await;
    let data = called.ok();
    assert_eq!(data["swimlane"]["id"], "sit1");
    assert_eq!(data["auto_notify"], true);
    assert_eq!(
        data["services"][0]["service"],
        json!({"namespace": "apps", "name": "api"})
    );
    assert_eq!(data["services"][0]["pickup"], "live");
    assert_eq!(data["services"][1]["pickup"], "needs_notify_or_restart");
    assert_eq!(called.envelope()["truncated"], false);

    assert_eq!(
        call(&mut c, "get_swimlane", &json!({"swimlane": "nope"}))
            .await
            .err(),
        "not_found"
    );

    // A port that cannot say whether auto-notify is on is an `unavailable` tool, never a `false`.
    rig.registry.unavailable.lock().unwrap().push("auto_notify");
    let called = call(&mut c, "get_swimlane", &json!({"swimlane": "sit1"})).await;
    assert_eq!(called.err(), "unavailable");
}

#[tokio::test]
async fn get_pending_restarts_use_pickup_states() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "x: 1\n")]);
    rig.registry.inner.set_pending_restarts(
        &sid("sit1"),
        vec![
            service("app", "zz", PickupState::LiveWithinTtl),
            service("app-x", "aa", PickupState::NeedsConfigServerRestart),
            service("app", "aa", PickupState::NeedsNotifyOrRestart),
            service("zeta", "one", PickupState::Live),
        ],
    );
    let mut c = viewer(&rig).await;
    // Paged by the key `namespace/name`, in the order of that text (`app-x/aa` before `app/aa`).
    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut args = json!({"swimlane": "sit1", "limit": 1});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let called = call(&mut c, "get_pending_restarts", &args).await;
        for s in called.ok()["services"].as_array().unwrap() {
            seen.push((
                format!(
                    "{}/{}",
                    s["service"]["namespace"].as_str().unwrap(),
                    s["service"]["name"].as_str().unwrap()
                ),
                s["pickup"].as_str().unwrap().to_owned(),
            ));
        }
        cursor = called.envelope()["next"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(
        seen,
        [
            ("app-x/aa".to_owned(), "needs_config_server_restart".to_owned()),
            ("app/aa".to_owned(), "needs_notify_or_restart".to_owned()),
            ("app/zz".to_owned(), "live_within_ttl".to_owned()),
            ("zeta/one".to_owned(), "live".to_owned()),
        ]
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn list_files_filters_by_drift() {
    let rig = rig();
    seed(
        &rig,
        "sit1",
        &[("a/x.yml", "x: 1\n"), ("a/y.yml", "y: 1\n"), ("b.yml", "b: 1\n")],
    );
    let mut c = viewer(&rig).await;

    // A folder listing asks the tree for the prefix as a typed path.
    let listed = call(&mut c, "list_files", &json!({"swimlane": "sit1", "prefix": "a"})).await;
    assert_eq!(ids(listed.ok(), "files", "path"), ["a/x.yml", "a/y.yml"]);
    assert!(
        rig.registry
            .log
            .of("tree")
            .iter()
            .any(|l| l.contains("prefix=a ")),
        "{:?}",
        rig.registry.log.all()
    );

    // With a drift filter the question goes to `drift`, with the state and the prefix as typed values.
    rig.registry.inner.set_drift(
        &sid("sit1"),
        vec![
            entry("a/x.yml", DriftState::NfsAhead),
            entry("a/y.yml", DriftState::InSync),
            entry("b.yml", DriftState::NfsAhead),
        ],
    );
    rig.registry.log.clear();
    let drifted = call(
        &mut c,
        "list_files",
        &json!({"swimlane": "sit1", "drift": "nfs_ahead"}),
    )
    .await;
    assert_eq!(ids(drifted.ok(), "files", "path"), ["a/x.yml", "b.yml"]);
    let logged = rig.registry.log.of("drift");
    assert_eq!(logged.len(), 1);
    assert!(
        logged[0].contains("state=Some(NfsAhead)") && logged[0].contains("prefix=None"),
        "{logged:?}"
    );
    let drifted = call(
        &mut c,
        "list_files",
        &json!({"swimlane": "sit1", "drift": "nfs_ahead", "prefix": "a"}),
    )
    .await;
    assert_eq!(ids(drifted.ok(), "files", "path"), ["a/x.yml"]);
    assert!(rig.registry.log.of("drift")[1].contains("prefix=Some(\"a\")"));
    assert!(
        rig.registry.log.of("tree").is_empty(),
        "the drift filter does not also list the tree"
    );
}

#[tokio::test]
async fn get_drift_carries_attribution_and_severity() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "x: 1\n")]);
    let mut e = entry("a.yml", DriftState::NfsAhead);
    e.attribution = Some(attribution(
        AttributionSource::SentinelLogin,
        Confidence::High,
        Some("jdoe"),
    ));
    e.severity = Some(Severity::High);
    e.pickup_state = Some(PickupState::NeedsNotifyOrRestart);
    rig.registry.inner.set_drift(
        &sid("sit1"),
        vec![
            e,
            entry("b.yml", DriftState::InSync),
            entry("c.yml", DriftState::Conflict),
        ],
    );
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_drift", &json!({"swimlane": "sit1"})).await;
    let data = called.ok();
    // The files that are not in sync, by default: attribution and severity ride with each.
    assert_eq!(ids(data, "files", "path"), ["a.yml", "c.yml"]);
    let first = &data["files"][0];
    assert_eq!(first["drift"], "nfs_ahead");
    assert_eq!(first["severity"], "high");
    assert_eq!(first["pickup_state"], "needs_notify_or_restart");
    assert_eq!(
        first["attribution"],
        json!({"source": "sentinel_login", "confidence": "high", "actor": {"kind": "os_login", "id": "jdoe"}, "executable": "vim"})
    );
    assert_eq!(data["files"][1]["attribution"], Value::Null);
    // A state filter reaches the port as typed.
    let only = call(
        &mut c,
        "get_drift",
        &json!({"swimlane": "sit1", "state": "conflict"}),
    )
    .await;
    assert_eq!(ids(only.ok(), "files", "path"), ["c.yml"]);
}

#[tokio::test]
async fn get_history_pages_newest_first() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "v: 1\n")]);
    for n in 2..=4 {
        assert!(
            rig.registry
                .inner
                .observe(&sid("sit1"), &np("a.yml"), bytes(&format!("v: {n}\n")))
        );
    }
    let mut c = viewer(&rig).await;
    let first = call(
        &mut c,
        "get_history",
        &json!({"swimlane": "sit1", "path": "a.yml", "limit": 2}),
    )
    .await;
    let hashes: Vec<String> = ids(first.ok(), "versions", "hash");
    assert_eq!(
        hashes,
        [hash_of(b"v: 4\n").to_string(), hash_of(b"v: 3\n").to_string()]
    );
    assert_eq!(first.ok()["versions"][0]["attribution"]["source"], "unknown");
    let next = first.envelope()["next"].clone();
    assert!(next.is_string());
    let second = call(
        &mut c,
        "get_history",
        &json!({"swimlane": "sit1", "path": "a.yml", "limit": 2, "cursor": next}),
    )
    .await;
    assert_eq!(
        ids(second.ok(), "versions", "hash"),
        [hash_of(b"v: 2\n").to_string(), hash_of(b"v: 1\n").to_string()]
    );
    assert_eq!(second.envelope()["next"], Value::Null);
}

#[tokio::test]
async fn read_file_by_lines() {
    let rig = rig();
    let text: String = (1..=10).map(|n| format!("line{n}\r\n")).collect();
    seed(&rig, "sit1", &[("notes.txt", &text)]);
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "notes.txt", "lines": {"start": 2, "end": 4}}),
    )
    .await;
    let data = called.ok();
    // Line endings are LF in the answer, whatever they are on disk.
    assert_eq!(data["text"], "line2\nline3\nline4");
    assert_eq!(data["returned_lines"], json!({"start": 2, "end": 4}));
    assert_eq!(data["total_lines"], 10);
    assert_eq!(data["untrusted"], true);
    assert_eq!(data["label"], "data, not instructions");
    assert_eq!(data["hash"], hash_of(text.as_bytes()).to_string());
    assert_eq!(called.envelope()["truncated"], false);
    assert!(data["outline"].as_array().unwrap().is_empty());

    // The default is the whole of a small file.
    let whole = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "notes.txt"}),
    )
    .await;
    assert_eq!(whole.ok()["returned_lines"], json!({"start": 1, "end": 10}));
    assert_eq!(whole.envelope()["truncated"], false);

    // A range past the end of the file is an argument error, and one that runs past the end is cut at it.
    let past = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "notes.txt", "lines": {"start": 11, "end": 12}}),
    )
    .await;
    assert_eq!(past.err(), "invalid_argument");
    let tail = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "notes.txt", "lines": {"start": 9, "end": 50}}),
    )
    .await;
    assert_eq!(tail.ok()["returned_lines"], json!({"start": 9, "end": 10}));
    assert_eq!(
        tail.envelope()["truncated"],
        false,
        "the file ended; nothing was cut"
    );

    // At most 200 lines, whatever is asked.
    let many: String = (1..=300).map(|n| format!("row{n}\n")).collect();
    seed(&rig, "sit2", &[("many.txt", &many)]);
    let cut = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit2", "path": "many.txt", "lines": {"start": 1, "end": 300}}),
    )
    .await;
    assert_eq!(cut.ok()["returned_lines"], json!({"start": 1, "end": 200}));
    assert_eq!(cut.envelope()["truncated"], true);
    assert!(cut.envelope()["web_link"].is_string());
}

#[tokio::test]
async fn read_file_by_setting_path() {
    let rig = rig();
    seed(
        &rig,
        "sit1",
        &[
            ("tx-core/app.yml", &big_yaml(5)),
            ("tx-core/notes.txt", "hello\n"),
        ],
    );
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "tx-core/app.yml", "setting_path": "group2.key3"}),
    )
    .await;
    let data = called.ok();
    assert_eq!(data["text"], "  key3: value-2-3");
    let at = data["returned_lines"]["start"].as_u64().unwrap();
    assert_eq!(data["returned_lines"], json!({"start": at, "end": at}));

    let missing = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "tx-core/app.yml", "setting_path": "nope.nothing"}),
    )
    .await;
    assert_eq!(missing.err(), "not_found");
    // A text file has no settings.
    let text_file = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "tx-core/notes.txt", "setting_path": "a"}),
    )
    .await;
    assert_eq!(text_file.err(), "invalid_argument");
}

#[tokio::test]
async fn read_file_lines_and_setting_path_together_is_invalid() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", &big_yaml(2))]);
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "a.yml", "lines": {"start": 1, "end": 2}, "setting_path": "group0.key0"}),
    )
    .await;
    assert_eq!(called.err(), "invalid_argument");
    assert!(
        called.error()["message"]
            .as_str()
            .unwrap()
            .contains("cannot be used together")
    );
    // The port was not asked: the arguments are refused first.
    assert!(rig.registry.log.of("content").is_empty());
}

#[tokio::test]
async fn read_file_large_returns_outline_and_first_lines() {
    let rig = rig();
    let text = big_yaml(100);
    seed(&rig, "sit1", &[("big.yml", &text)]);
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "big.yml"}),
    )
    .await;
    let data = called.ok();
    let returned = &data["returned_lines"];
    assert_eq!(returned["start"], 1);
    let last = returned["end"].as_u64().unwrap();
    assert!((1..=200).contains(&last), "{returned}");
    assert_eq!(data["total_lines"], 600);
    assert_eq!(called.envelope()["truncated"], true);
    assert!(called.envelope()["web_link"].is_string());
    // The first lines are the first lines of the file.
    let expected: String = text
        .lines()
        .take(usize::try_from(last).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(data["text"], expected);
    // The outline is the top-level settings with the lines each spans.
    let outline = data["outline"].as_array().unwrap();
    assert!(!outline.is_empty() && outline.len() <= 200);
    assert_eq!(
        outline[0],
        json!({"path": "group0", "lines": {"start": 2, "end": 6}})
    );
    assert!(called.bytes() <= 16 * 1024, "{}", called.bytes());

    // An outline of hundreds of entries shares the 16 KB with the text; it never takes all of it.
    seed(&rig, "sit3", &[("wide.yml", &big_yaml(400))]);
    let wide = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit3", "path": "wide.yml"}),
    )
    .await;
    let data = wide.ok();
    let listed = data["outline"].as_array().unwrap().len();
    assert!((1..=200).contains(&listed), "{listed}");
    assert!(
        data["returned_lines"]["end"].as_u64().unwrap() >= 20,
        "the text keeps room: {}",
        data["returned_lines"]
    );
    assert!(wide.bytes() <= 16 * 1024, "{}", wide.bytes());
    assert_eq!(wide.envelope()["truncated"], true);

    // A file that is not structured gets its first lines and no outline.
    let many: String = (1..=500).map(|n| format!("row{n}\n")).collect();
    seed(&rig, "sit2", &[("many.txt", &many)]);
    let plain = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit2", "path": "many.txt"}),
    )
    .await;
    assert!(plain.ok()["outline"].as_array().unwrap().is_empty());
    assert_eq!(plain.envelope()["truncated"], true);
}

#[tokio::test]
async fn read_file_withholds_a_denied_file_and_says_so() {
    let rig = rig();
    seed(&rig, "sit1", &[("keys/server.p12", "not really")]);
    let text = "should never be read";
    rig.registry.set_content(
        &sid("sit1"),
        &np("keys/server.p12"),
        FileContent {
            path: np("keys/server.p12"),
            hash: hash_of(text.as_bytes()),
            class: FileClass::Denied,
            format: file_format(),
            size: 1234,
            bytes: Some(bytes(text)),
            withheld: false,
        },
    );
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "keys/server.p12"}),
    )
    .await;
    let data = called.ok();
    assert_eq!(data["content_withheld"], true);
    assert_eq!(data["text"], "content withheld");
    assert_eq!(data["class"], "denied");
    assert_eq!(data["hash"], hash_of(text.as_bytes()).to_string());
    assert!(
        !called.response.to_string().contains(text),
        "the bytes the port held are nowhere in the answer"
    );
    // The flag of the port alone is enough, whatever the class.
    rig.registry.set_content(
        &sid("sit1"),
        &np("keys/server.p12"),
        FileContent {
            path: np("keys/server.p12"),
            hash: hash_of(text.as_bytes()),
            class: FileClass::Text,
            format: file_format(),
            size: 5,
            bytes: None,
            withheld: true,
        },
    );
    let again = call(
        &mut c,
        "read_file",
        &json!({"swimlane": "sit1", "path": "keys/server.p12"}),
    )
    .await;
    assert_eq!(again.ok()["content_withheld"], true);
}

// ---------------------------------------------------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------------------------------------------------

pub(crate) fn effective_fixture(secret: &str) -> EffectiveConfig {
    EffectiveConfig {
        swimlane: sid("sit1"),
        file: lf("tx-core/app.yml"),
        tenant: tid("sit1"),
        channel: None,
        role: domain::FileRole::PropertySource,
        hash: hash_of(b"rendered"),
        sources: vec![
            EffectiveSource {
                path: np("tx-core/app-sit1.yml"),
                rank: 1,
            },
            EffectiveSource {
                path: np("tx-core/app.yml"),
                rank: 3,
            },
        ],
        settings: vec![
            EffectiveSetting {
                path: sp("a.b"),
                value: "1".into(),
                source: np("tx-core/app-sit1.yml"),
                location: Some(location("a.b", 3, 3)),
            },
            EffectiveSetting {
                path: sp("a.c"),
                value: secret.into(),
                source: np("tx-core/app.yml"),
                location: None,
            },
            EffectiveSetting {
                path: sp("x.y"),
                value: "2".into(),
                source: np("tx-core/app.yml"),
                location: Some(location("x.y", 9, 9)),
            },
        ],
        unresolved_placeholders: vec![st("${HOST}")],
    }
}

#[tokio::test]
async fn get_effective_config_passes_tenant_channel_and_prefix() {
    let rig = rig();
    seed(&rig, "sit1", &[("tx-core/app.yml", "a: 1\n")]);
    let secret = fake_github_token();
    rig.registry.inner.set_effective(effective_fixture(&secret));
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "get_effective_config",
        &json!({"swimlane": "sit1", "file": "tx-core/app.yml", "tenant": "sit2", "channel": "remote-itm", "setting_prefix": "a"}),
    )
    .await;
    let data = called.ok();
    // The question reached the port as typed values, and the answer is for that question.
    let logged = rig.registry.log.of("effective_for");
    assert!(
        logged[0].contains("tenant=sit2")
            && logged[0].contains("Some(\"remote-itm\")")
            && logged[0].contains("Some(\"a\")"),
        "{logged:?}"
    );
    assert_eq!(data["tenant"], "sit2");
    assert_eq!(data["channel"], "remote-itm");
    assert_eq!(ids(data, "settings", "path"), ["a.b", "a.c"]);
    assert_eq!(data["settings_total"], 2);
    assert_eq!(data["role"], "property_source");
    assert_eq!(data["sources"][0]["rank"], 1);
    assert_eq!(data["unresolved_placeholders"][0], "${HOST}");
    assert_eq!(data["untrusted"], true);
    // A value the scan flags is replaced, in the structured result and in the text.
    let flagged = &data["settings"][1]["value"];
    assert_eq!(flagged["redacted"], true);
    assert!(
        flagged["text"]
            .as_str()
            .unwrap()
            .starts_with("[REDACTED: 40 chars, github_token]"),
        "{flagged}"
    );
    assert!(!called.response.to_string().contains(&secret));
    // The port does not give the type of a value, so none is claimed.
    assert_eq!(data["settings"][0]["value"]["type"], "other");
    assert_eq!(data["settings"][0]["value"]["flag"], Value::Null);
    // No tenant is a missing argument, named.
    let missing = call(
        &mut c,
        "get_effective_config",
        &json!({"swimlane": "sit1", "file": "a.yml"}),
    )
    .await;
    assert_eq!(missing.err(), "invalid_argument");
    assert!(missing.error()["message"].as_str().unwrap().contains("`tenant`"));
}

pub(crate) fn served_fixture(text: Option<&str>, withheld: bool) -> ServedFile {
    ServedFile {
        swimlane: sid("sit1"),
        application: domain::AppName::parse("tx-core").unwrap(),
        tenant: tid("sit1"),
        channel: None,
        file: np("app.yml"),
        role: domain::FileRole::PropertySource,
        hash: hash_of(b"served"),
        content_withheld: withheld,
        text: text.map(str::to_owned),
        sources: vec![EffectiveSource {
            path: np("tx-core/app.yml"),
            rank: 1,
        }],
        unresolved_placeholders: Vec::new(),
    }
}

#[tokio::test]
async fn get_served_config_is_redacted() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let secret = fake_github_token();
    rig.registry.inner.set_served(served_fixture(
        Some(&format!("a: 1\ntoken: {secret}\nb: 2\n")),
        false,
    ));
    let mut c = viewer(&rig).await;
    let args = json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit2", "channel": "x", "file": "app.yml"});
    let called = call(&mut c, "get_served_config", &args).await;
    let data = called.ok();
    assert!(
        data["text"]
            .as_str()
            .unwrap()
            .contains("token: [REDACTED: 40 chars, github_token]"),
        "{data}"
    );
    assert_eq!(data["redacted_values"], 1);
    assert!(!called.response.to_string().contains(&secret));
    assert_eq!(data["tenant"], "sit2", "the answer is for the question asked");
    assert_eq!(data["untrusted"], true);
    let logged = rig.registry.log.of("served");
    assert!(
        logged[0].contains("app=tx-core")
            && logged[0].contains("tenant=sit2")
            && logged[0].contains("Some(\"x\")"),
        "{logged:?}"
    );

    // A withheld file has no text, and says so.
    rig.registry.inner.set_served(served_fixture(None, true));
    let withheld = call(&mut c, "get_served_config", &args).await;
    assert_eq!(withheld.ok()["content_withheld"], true);
    assert_eq!(withheld.ok()["text"], Value::Null);
}

#[tokio::test]
async fn served_unavailable_is_unavailable() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let mut c = viewer(&rig).await;
    // The fake has no scripted file: as when the agent cannot be reached.
    let called = call(
        &mut c,
        "get_served_config",
        &json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": "app.yml"}),
    )
    .await;
    assert_eq!(called.err(), "unavailable");
    assert_eq!(called.error()["retryable"], true);
}

// ---------------------------------------------------------------------------------------------------------------------
// Compare
// ---------------------------------------------------------------------------------------------------------------------

pub(crate) fn comparison(class: FileClass) -> domain::Comparison {
    domain::Comparison {
        left: "nfs:sit1".parse().unwrap(),
        right: "nfs:sit2".parse().unwrap(),
        file: Some(lf("a.yml")),
        class,
        identical: false,
        eol_only: false,
        hunks: vec![DiffHunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: "a: 1".into(),
                    inline_changes: Vec::new(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "b: 2".into(),
                    inline_changes: Vec::new(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "b: 3".into(),
                    inline_changes: Vec::new(),
                },
            ],
        }],
        setting_changes: vec![
            change("b", ChangeKind::Changed, Some("2"), Some("3")),
            change("c", ChangeKind::Added, None, Some("9")),
        ],
        binary: None,
    }
}

#[tokio::test]
async fn compare_settings_and_lines_detail() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    rig.registry
        .inner
        .set_comparison(comparison(FileClass::Structured));
    let mut c = viewer(&rig).await;
    let base =
        json!({"left": "nfs:sit1", "right": "git:base@v1.0.0", "file": "a.yml", "channel": "remote-itm"});
    let with = |detail: &str| {
        let mut args = base.clone();
        args["detail"] = json!(detail);
        args
    };

    let settings = call(&mut c, "compare", &with("settings")).await;
    let data = settings.ok();
    assert_eq!(data["setting_change_count"], 2);
    assert_eq!(data["setting_changes"][0]["path"], "b");
    assert_eq!(data["setting_changes"][0]["kind"], "changed");
    assert_eq!(data["setting_changes"][0]["left"]["text"], "2");
    assert_eq!(data["hunk_count"], 1);
    assert!(
        data["hunks"].as_array().unwrap().is_empty(),
        "settings detail leaves the hunks out"
    );
    // The channel and the refs reached the port as typed values.
    let logged = rig.registry.log.of("compare_in");
    assert!(
        logged[0].contains("channel=Some(\"remote-itm\")") && logged[0].contains("git:base@v1.0.0"),
        "{logged:?}"
    );

    let lines = call(&mut c, "compare", &with("lines")).await;
    let data = lines.ok();
    assert!(data["setting_changes"].as_array().unwrap().is_empty());
    assert_eq!(
        data["hunks"][0]["lines"][1],
        json!({"kind": "removed", "text": "b: 2"})
    );

    // `auto`: setting changes for a structured file, hunks for a text file.
    let auto = call(&mut c, "compare", &with("auto")).await;
    assert_eq!(auto.ok()["setting_changes"].as_array().unwrap().len(), 2);
    rig.registry.inner.set_comparison(comparison(FileClass::Text));
    let auto_text = call(&mut c, "compare", &with("auto")).await;
    assert_eq!(auto_text.ok()["hunks"].as_array().unwrap().len(), 1);
    assert!(auto_text.ok()["setting_changes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn compare_tree_pages_and_reports_skipped_subtrees() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let entries: Vec<TreeCompareEntry> = (1..=5)
        .map(|n| TreeCompareEntry {
            file: lf(&format!("a/f{n}.yml")),
            state: if n % 2 == 0 {
                domain::CompareState::LeftOnly
            } else {
                domain::CompareState::Different
            },
        })
        .collect();
    rig.registry.inner.set_tree_comparison(TreeComparison {
        entries: Paged::last(entries),
        skipped_identical_subtrees: 7,
    });
    let mut c = viewer(&rig).await;
    let args = |cursor: Option<&Value>| {
        let mut a = json!({"left": "nfs:sit1", "right": "nfs:sit2", "prefix": "a", "limit": 2});
        if let Some(cursor) = cursor {
            a["cursor"] = cursor.clone();
        }
        a
    };
    let mut files = Vec::new();
    let mut cursor: Option<Value> = None;
    loop {
        let called = call(&mut c, "compare_tree", &args(cursor.as_ref())).await;
        let data = called.ok();
        assert_eq!(data["skipped_identical_subtrees"], 7);
        files.extend(ids(data, "entries", "file"));
        let next = called.envelope()["next"].clone();
        if next.is_null() {
            break;
        }
        cursor = Some(next);
    }
    assert_eq!(
        files,
        ["a/f1.yml", "a/f2.yml", "a/f3.yml", "a/f4.yml", "a/f5.yml"]
    );
    assert!(
        rig.registry.log.of("compare_tree_under").len() >= 3,
        "the prefix reached the port"
    );
}

#[tokio::test]
async fn compare_grid_up_to_64_swimlanes() {
    let rig = rig();
    let lanes: Vec<String> = (1..=64).map(|n| format!("sit{n}")).collect();
    for lane in &lanes {
        seed(&rig, lane, &[("a.yml", "a: 1\n")]);
    }
    let cell = |text: &str| GridCell {
        value: Some(value(text)),
        location: Some(location("a.b", 1, 1)),
    };
    rig.registry.inner.set_grid(Grid {
        swimlanes: lanes.iter().map(|l| sid(l)).collect(),
        rows: Paged::last(vec![GridRow {
            path: sp("a.b"),
            cells: (0..64)
                .map(|n| {
                    if n == 5 {
                        GridCell {
                            value: None,
                            location: None,
                        }
                    } else {
                        cell(&format!("v{n}"))
                    }
                })
                .collect(),
        }]),
    });
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "compare_grid",
        &json!({"file": "a.yml", "swimlanes": lanes, "channel": "x", "only_differing": false}),
    )
    .await;
    let data = called.ok();
    assert_eq!(data["swimlanes"].as_array().unwrap().len(), 64);
    let cells = data["rows"][0]["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 64);
    assert_eq!(
        cells[5],
        json!({"value": null, "location": null}),
        "a null value means the setting is absent"
    );
    assert_eq!(cells[0]["value"]["text"], "v0");
    let logged = rig.registry.log.of("grid");
    assert!(
        logged[0].contains("swimlanes=64")
            && logged[0].contains("only_differing=false")
            && logged[0].contains("Some(\"x\")"),
        "{logged:?}"
    );
    // 65 is over the contract's limit.
    let mut too_many = lanes.clone();
    too_many.push("sit65".to_owned());
    assert_eq!(
        call(
            &mut c,
            "compare_grid",
            &json!({"file": "a.yml", "swimlanes": too_many})
        )
        .await
        .err(),
        "invalid_argument"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Search and findings
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn search_settings_pages() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let hits: Vec<SettingHit> = (1..=5)
        .map(|n| SettingHit {
            swimlane: sid("sit1"),
            file: np("a.yml"),
            path: sp(&format!("timeout.t{n}")),
            value: value(&format!("{n}")),
            location: location(&format!("timeout.t{n}"), n, n),
        })
        .collect();
    rig.registry.inner.set_setting_hits(hits);
    let mut c = viewer(&rig).await;
    let mut paths = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let mut args = json!({"q": "timeout", "limit": 2, "swimlanes": ["sit1"]});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let called = call(&mut c, "search_settings", &args).await;
        paths.extend(ids(called.ok(), "hits", "path"));
        cursor = called.envelope()["next"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(
        paths,
        [
            "timeout.t1",
            "timeout.t2",
            "timeout.t3",
            "timeout.t4",
            "timeout.t5"
        ]
    );
    assert!(rig.registry.log.of("search_settings")[0].contains("q=timeout swimlanes=1"));
}

#[tokio::test]
async fn search_text_truncation_is_reported() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let secret = fake_github_token();
    rig.registry.inner.set_text_results(TextResults {
        hits: vec![
            TextHit {
                swimlane: sid("sit1"),
                path: np("a.yml"),
                line: 3,
                text: "timeout: 30".into(),
                ranges: vec![(0, 7)],
            },
            TextHit {
                swimlane: sid("sit1"),
                path: np("a.yml"),
                line: 9,
                text: format!("token: {secret} # timeout"),
                ranges: vec![(25, 32)],
            },
            TextHit {
                swimlane: sid("sit1"),
                path: np("b.yml"),
                line: 1,
                text: "x".repeat(2000),
                ranges: vec![(0, 1), (1900, 1901)],
            },
        ],
        truncated: true,
    });
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "search_text",
        &json!({"pattern": "timeout", "swimlanes": ["sit1"], "path_glob": "*.yml"}),
    )
    .await;
    let data = called.ok();
    let envelope = called.envelope();
    assert_eq!(envelope["truncated"], true);
    assert!(envelope["web_link"].is_string(), "{envelope}");
    assert_eq!(envelope["next"], Value::Null, "search_text does not page");
    assert_eq!(data["hits"][0]["ranges"], json!([[0, 7]]));
    // A hit with a secret in it is redacted and has no ranges, since they no longer point at the text.
    assert!(
        data["hits"][1]["text"]
            .as_str()
            .unwrap()
            .contains("[REDACTED: 40 chars, github_token]")
    );
    assert_eq!(data["hits"][1]["ranges"], json!([]));
    assert!(!called.response.to_string().contains(&secret));
    // A long line is cut to the contract's 512 characters, and the ranges beyond the cut go.
    assert!(data["hits"][2]["text"].as_str().unwrap().chars().count() <= 512);
    assert_eq!(data["hits"][2]["ranges"], json!([[0, 1]]));
    // The defaults reached the port.
    let logged = rig.registry.log.of("search_text");
    assert!(
        logged[0].contains("max_total=50")
            && logged[0].contains("max_per_file=5")
            && logged[0].contains("regex=false"),
        "{logged:?}"
    );
}

#[tokio::test]
async fn find_inconsistencies_filters_kind_and_severity() {
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    seed(&rig, "sit2", &[("a.yml", "a: 1\n")]);
    for (id, kind, lane, severity) in [
        (1, FindingKind::Drift, "sit1", Severity::Low),
        (2, FindingKind::OrphanFile, "sit1", Severity::High),
        (3, FindingKind::Drift, "sit2", Severity::Critical),
        (4, FindingKind::DuplicateKeys, "sit2", Severity::Medium),
    ] {
        rig.registry.inner.add_finding(finding(id, kind, lane, severity));
    }
    let mut c = viewer(&rig).await;
    let all = call(&mut c, "find_inconsistencies", &json!({})).await;
    assert_eq!(all.ok()["findings"].as_array().unwrap().len(), 4);
    let drift = call(&mut c, "find_inconsistencies", &json!({"kinds": ["drift"]})).await;
    assert_eq!(ids(drift.ok(), "findings", "swimlane"), ["sit1", "sit2"]);
    let high = call(&mut c, "find_inconsistencies", &json!({"min_severity": "high"})).await;
    assert_eq!(ids(high.ok(), "findings", "kind"), ["orphan_file", "drift"]);
    let lane = call(
        &mut c,
        "find_inconsistencies",
        &json!({"swimlane": "sit2", "kinds": ["drift", "duplicate_keys"], "min_severity": "medium"}),
    )
    .await;
    assert_eq!(lane.ok()["findings"].as_array().unwrap().len(), 2);
    let first = &all.ok()["findings"][0];
    assert_eq!(first["rule_ids"], json!(["R1"]));
    assert_eq!(
        first["locations"][0],
        json!({"path": "a.b", "lines": {"start": 3, "end": 4}})
    );
    assert_eq!(first["message"], "A plain sentence from the engine.");
}

// ---------------------------------------------------------------------------------------------------------------------
// Docs
// ---------------------------------------------------------------------------------------------------------------------

fn dp(s: &str) -> domain::DocPath {
    domain::DocPath::parse(s).unwrap()
}

#[tokio::test]
async fn search_docs_read_doc_grep_docs_get_docs_for_file() {
    let rig = rig();
    let secret = fake_github_token();
    rig.docs.inner.seed(
        dp("/git/main/guide.md"),
        "Guide",
        &format!("# Guide\nSet timeout in `tx-core/app.yml`.\nsetting a.b controls retries.\nexample token {secret}\nlast line\n"),
    );
    rig.docs.inner.seed(
        dp("/uploads/notes.md"),
        "Notes",
        "timeout is 30 seconds\nsecond\n",
    );
    let mut c = viewer(&rig).await;

    let search = call(&mut c, "search_docs", &json!({"q": "timeout", "limit": 5})).await;
    let hits = search.ok()["hits"].as_array().unwrap().clone();
    assert_eq!(hits.len(), 2);
    assert_eq!(search.ok()["untrusted"], true);
    assert!(
        hits.iter()
            .any(|h| h["path"] == "/git/main/guide.md" && h["title"] == "Guide")
    );
    assert!(rig.docs.log.of("search")[0].contains("limit=5"));

    let read = call(
        &mut c,
        "read_doc",
        &json!({"path": "/git/main/guide.md", "lines": {"start": 2, "end": 4}}),
    )
    .await;
    let data = read.ok();
    assert_eq!(data["returned_lines"], json!({"start": 2, "end": 4}));
    assert_eq!(data["total_lines"], 5);
    assert!(
        data["text"]
            .as_str()
            .unwrap()
            .contains("[REDACTED: 40 chars, github_token]"),
        "{data}"
    );
    assert!(!read.response.to_string().contains(&secret));
    assert_eq!(data["untrusted"], true);
    assert_eq!(
        call(&mut c, "read_doc", &json!({"path": "/git/main/missing.md"}))
            .await
            .err(),
        "not_found"
    );

    let grep = call(
        &mut c,
        "grep_docs",
        &json!({"pattern": "timeout", "context": 1, "path_prefix": "/git/main", "limit": 10}),
    )
    .await;
    let hits = &grep.ok()["hits"];
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["path"], "/git/main/guide.md");
    assert_eq!(hits[0]["line"], 2);
    assert_eq!(hits[0]["before"], json!(["# Guide"]));
    let logged = rig.docs.log.of("grep_in");
    assert!(
        logged[0].contains("prefix=Some(\"/git/main\")")
            && logged[0].contains("limit=10")
            && logged[0].contains("context=1"),
        "{logged:?}"
    );
    // The linear-time engine is the port's: a regex the port refuses is an argument error.
    let regex = call(&mut c, "grep_docs", &json!({"pattern": "t.meout", "regex": true})).await;
    assert_eq!(regex.err(), "invalid_argument");

    let related = call(
        &mut c,
        "get_docs_for_file",
        &json!({"file": "tx-core/app.yml", "setting": "a.b", "limit": 3}),
    )
    .await;
    assert_eq!(related.ok()["hits"].as_array().unwrap().len(), 1);
    let logged = rig.docs.log.of("related_to");
    assert!(
        logged[0].contains("setting=Some(\"a.b\")") && logged[0].contains("limit=3"),
        "{logged:?}"
    );
    let _ = &secret;
}

#[tokio::test]
async fn get_feature_status_filters() {
    let rig = rig();
    rig.docs.inner.set_feature_status(domain::FeatureStatus {
        flags: vec![domain::FlagStatus {
            flag: st("atm"),
            file: lf("tx-core/features.yml"),
            channel: Some(domain::ChannelName::parse("remote-itm").unwrap()),
            setting: sp("features.atm"),
            template_default: Some(value("true")),
            values: vec![
                domain::FlagValue {
                    swimlane: sid("sit1"),
                    value: Some(value("true")),
                },
                domain::FlagValue {
                    swimlane: sid("sit2"),
                    value: None,
                },
            ],
        }],
    });
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "get_feature_status",
        &json!({"flag": "atm", "swimlanes": ["sit1", "sit2"]}),
    )
    .await;
    let flag = &called.ok()["flags"][0];
    assert_eq!(flag["flag"], "atm");
    assert_eq!(flag["setting"], "features.atm");
    assert_eq!(flag["template_default"]["text"], "true");
    assert_eq!(
        flag["values"][1],
        json!({"swimlane": "sit2", "value": null}),
        "a null value means the setting is absent"
    );
    assert!(rig.docs.log.of("feature_status")[0].contains("flag=Some(\"atm\") swimlanes=2"));
}

// ---------------------------------------------------------------------------------------------------------------------
// Errors from the ports
// ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn port_errors_map_to_the_contract_codes() {
    use ports::{DocError, ReadError};
    let rig = rig();
    seed(&rig, "sit1", &[("a.yml", "a: 1\n")]);
    let mut c = viewer(&rig).await;
    for (error, code) in [
        (ReadError::Forbidden, "forbidden"),
        (ReadError::NotFound, "not_found"),
        (ReadError::BadRequest, "invalid_argument"),
        (ReadError::Unavailable, "unavailable"),
    ] {
        rig.registry.fail("swimlanes", error);
        let called = call(&mut c, "list_swimlanes", &json!({})).await;
        assert_eq!(called.err(), code, "{error:?}");
        assert_eq!(called.error()["retryable"], code == "unavailable");
        assert!(!called.error()["request_id"].as_str().unwrap().is_empty());
    }
    for (error, code) in [
        (DocError::Forbidden, "forbidden"),
        (DocError::NotFound, "not_found"),
        (DocError::BadRequest, "invalid_argument"),
        (DocError::Unavailable, "unavailable"),
    ] {
        rig.docs.fail("search", error);
        assert_eq!(
            call(&mut c, "search_docs", &json!({"q": "x"})).await.err(),
            code,
            "{error:?}"
        );
    }
}
