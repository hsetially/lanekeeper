//! The hub's deny globs on a live connection (T11, D79): they arrive with `AgentConfig`, are added to the built-in ones
//! and never replace them, and change what the next delta carries, in virtual time with a scripted file system.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::tree::TreeSource;
use proto::convert::FromAgent;
use proto::pb;
use support::oob::Harness;
use support::rig::Rig;
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

const SECRET: &[u8] = b"MARKER-0d4c-session-secret";

fn config(deny_globs: &[&str]) -> pb::AgentConfig {
    pb::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: deny_globs.iter().map(|g| (*g).to_owned()).collect(),
        env_allowlist: Vec::new(),
        tenants: vec!["sit1".to_owned()],
    }
}

fn entry_for(conn: &support::fake_hub::ConnHandle, path: &str) -> Option<domain::ScanEntry> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Delta(d) => Some(d),
            _ => None,
        })
        .flat_map(|d| d.entries)
        .filter(|e| e.path.as_str() == path)
        .next_back()
}

async fn entry_with(conn: &support::fake_hub::ConnHandle, path: &str, content: &[u8]) -> domain::ScanEntry {
    for _ in 0..600 {
        if let Some(e) = entry_for(conn, path) {
            if e.size == content.len() as u64 {
                return e;
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("the hub never got {path}");
}

#[tokio::test(start_paused = true)]
async fn hub_deny_globs_are_added_not_replaced() {
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/app.yml", b"a: 1\n");
    source.write("svc/x.secret", b"before");
    source.write("svc/t.pem", b"before");
    let rig = Rig::new();
    rig.server.set_config(config(&["*.secret"]));
    let harness = Harness::start_with(source.clone(), rig).await;
    assert_eq!(source.deny().hub_globs(), ["*.secret"]);

    // The hub's glob and a built-in one both deny: the new versions go out as name, size and hash.
    source.write("svc/x.secret", SECRET);
    source.write("svc/t.pem", SECRET);
    source.write("svc/app.yml", b"a: 2, an ordinary change\n");
    let secret = entry_with(&harness.conn, "svc/x.secret", SECRET).await;
    assert!(secret.denied && secret.bytes.is_none());
    let pem = entry_with(&harness.conn, "svc/t.pem", SECRET).await;
    assert!(pem.denied && pem.bytes.is_none());
    let app = entry_with(&harness.conn, "svc/app.yml", b"a: 2, an ordinary change\n").await;
    assert!(!app.denied && app.bytes.is_some());
}

#[tokio::test(start_paused = true)]
async fn a_later_configuration_without_the_glob_releases_the_hub_s_files_but_never_a_built_in_one() {
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/x.secret", b"before");
    source.write("svc/t.pem", b"before");
    let rig = Rig::new();
    rig.server.set_config(config(&["*.secret"]));
    let harness = Harness::start_with(source.clone(), rig).await;
    assert_eq!(source.deny().hub_globs(), ["*.secret"]);

    // The hub's next configuration has no globs at all.
    harness.rig.server.set_config(config(&[]));
    harness.rig.server.net.kill_connections();
    let conn = harness.rig.server.wait_for_connection(2).await;
    conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
    assert!(source.deny().hub_globs().is_empty(), "the hub took its glob back");
    assert!(source.deny().is_denied("svc/t.pem"), "the built-in globs stay");

    source.write("svc/x.secret", SECRET);
    source.write("svc/t.pem", SECRET);
    let released = entry_with(&conn, "svc/x.secret", SECRET).await;
    assert!(!released.denied && released.bytes.as_deref() == Some(SECRET));
    let still = entry_with(&conn, "svc/t.pem", SECRET).await;
    assert!(still.denied && still.bytes.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_glob_that_is_not_valid_is_dropped_and_the_others_are_used() {
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/ok.thing", b"before");
    let rig = Rig::new();
    rig.server.set_config(config(&["[unclosed", "*.thing", "{a,b"]));
    let harness = Harness::start_with(source.clone(), rig).await;
    assert_eq!(source.deny().hub_globs(), ["*.thing"]);
    assert_eq!(source.deny().rejected(), 2);

    source.write("svc/ok.thing", SECRET);
    let entry = entry_with(&harness.conn, "svc/ok.thing", SECRET).await;
    assert!(entry.denied && entry.bytes.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_glob_the_hub_adds_re_marks_the_tree_without_waiting_for_the_next_walk() {
    // The hub's glob arrives with a configuration that denies a file that is already in the tree; the scanner is asked to
    // walk at once, so the tree's marks follow the list (the next change to the file then goes out as a denied entry).
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/x.secret", b"before");
    let harness = Harness::start(source.clone()).await;
    let walks = source.scans();
    let mut first_tree = source.tree();
    assert!(!first_tree.files()[0].1.denied);

    harness.rig.server.set_config(config(&["*.secret"]));
    harness.rig.server.net.kill_connections();
    let conn = harness.rig.server.wait_for_connection(2).await;
    conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
    sleep(Duration::from_secs(2)).await;
    assert!(
        source.scans() > walks,
        "a walk was asked for when the list changed"
    );
    first_tree = source.tree();
    assert!(
        first_tree.files()[0].1.denied,
        "the source marks it by the new list"
    );
}
