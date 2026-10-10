//! The deny globs, end to end (T11, D79, S17, S10): the whole agent runs on a real directory that holds keystores and keys,
//! against a fake hub over real TLS 1.3 and gRPC, and every place the denied bytes could go is searched for them.
//!
//! The files hold planted markers. Afterwards the stream (every message the hub received, on every connection), the
//! spool directory, the logs (from every thread, at every level) and the metrics are searched. A marker of a denied file
//! must be in none of them; a marker of an ordinary file must be on the stream (so the search is not blind) and in
//! neither the logs nor the metrics (which never hold content).
//!
//! The test runs in real time, because the walker and the spool work on real threads; it takes about half a minute.
//! There is one test that installs the process-wide log capture, so this file holds only one test that needs it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    AgentReply, ContentHash, Expected, HubCommand, NfsPath, OpError, OpResult, RequestId, ScanEntry,
};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::app_rig::{AppRig, Setup};
use support::capture::{Capture, Marker};
use support::fake_hub::ConnHandle;
use support::rig::Rig;
use support::tree::sha;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

/// How long a change may take to reach the hub: a 5 s walk, 3 s of quiet, and slack for a loaded machine.
const REACHES_THE_HUB: Duration = Duration::from_secs(90);

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn nfs(path: &str) -> NfsPath {
    NfsPath::parse(path).unwrap()
}

fn hash_of(content: &[u8]) -> ContentHash {
    ContentHash::from_bytes(sha(content))
}

fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn config(deny_globs: &[&str]) -> pb::AgentConfig {
    pb::AgentConfig {
        scan_interval_secs: 5,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: deny_globs.iter().map(|g| (*g).to_owned()).collect(),
        env_allowlist: Vec::new(),
        tenants: vec!["sit1".to_owned()],
    }
}

/// A file's content: some text around the marker, as a real file would have.
fn with_marker(marker: &Marker, head: &str) -> Vec<u8> {
    let mut content = head.as_bytes().to_vec();
    content.push(b'\n');
    content.extend(&marker.bytes);
    content.extend(b"\n-----END-----\n");
    content
}

/// Every entry the hub has received for `path`, in order, from every message of `conn`.
fn entries_for(conn: &ConnHandle, path: &str) -> Vec<ScanEntry> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Delta(d) => Some(d),
            _ => None,
        })
        .flat_map(|d| d.entries)
        .filter(|e| e.path.as_str() == path)
        .collect()
}

async fn wait_for_entry(conn: &ConnHandle, path: &str) -> ScanEntry {
    let found = timeout(
        REACHES_THE_HUB,
        conn.wait_for(
            |m| matches!(m, FromAgent::Delta(d) if d.entries.iter().any(|e| e.path.as_str() == path)),
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("the hub never got an entry for {path}"));
    let FromAgent::Delta(delta) = found else {
        unreachable!()
    };
    delta
        .entries
        .into_iter()
        .find(|e| e.path.as_str() == path)
        .unwrap()
}

async fn command(conn: &ConnHandle, id: &str, command: HubCommand) -> FromAgent {
    conn.send(ToAgent::Command(command)).await;
    let id = id.to_owned();
    timeout(
        Duration::from_secs(30),
        conn.wait_for(move |m| match m {
            FromAgent::Reply(AgentReply::Op(r)) => r.request_id.as_str() == id,
            FromAgent::Reply(AgentReply::File { request_id, .. }) => request_id.as_str() == id,
            _ => false,
        }),
    )
    .await
    .expect("the agent answers")
}

fn op(reply: FromAgent) -> OpResult {
    match reply {
        FromAgent::Reply(AgentReply::Op(result)) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

fn assert_denied_entry(entry: &ScanEntry, content: &[u8]) {
    assert!(entry.denied, "{} must be marked denied", entry.path);
    assert!(entry.bytes.is_none(), "{} must carry no bytes", entry.path);
    assert_eq!(entry.size, content.len() as u64, "{}: the size stays", entry.path);
    assert_eq!(entry.hash, hash_of(content), "{}: the hash stays", entry.path);
}

fn markers() -> Vec<Marker> {
    vec![
        Marker::text("denied_pem", "MARKER-PEM-5d1f0a77-never-leaves-the-export"),
        Marker::binary("denied_jks", 3),
        Marker::text(
            "denied_private_dir",
            "MARKER-PRIVDIR-2c9e41b8-never-leaves-the-export",
        ),
        Marker::binary("denied_keystore", 17),
        Marker::text(
            "denied_hub_glob",
            "MARKER-HUBGLOB-81e6d03f-never-leaves-the-export",
        ),
        Marker::text("denied_late", "MARKER-LATE-f4a7b2c9-denied-after-it-was-spooled"),
        Marker::text("denied_written", "MARKER-WRITTEN-0b3d95e6-a-write-to-refuse"),
        Marker::text("visible_a", "MARKER-VISIBLE-A-aa11-an-ordinary-file"),
        Marker::text("visible_b", "MARKER-VISIBLE-B-bb22-another-ordinary-file"),
    ]
}

fn marker<'a>(all: &'a [Marker], name: &str) -> &'a Marker {
    all.iter().find(|m| m.name == name).unwrap()
}

#[test]
fn planted_leaks_are_found() {
    use bytes::Bytes as B;
    use domain::{ScanDelta, Timestamp};

    let text = Marker::text("text", "MARKER-text-0001");
    let binary = Marker::binary("binary", 9);
    let capture = Capture::new(vec![text.clone(), binary.clone()]);
    let message_with = |bytes: &[u8], path: &str| {
        FromAgent::Delta(ScanDelta {
            seq: 1,
            base_root: None,
            new_root: ContentHash::from_bytes([1; 32]),
            entries: vec![ScanEntry {
                path: nfs(path),
                hash: ContentHash::from_bytes([2; 32]),
                size: bytes.len() as u64,
                mtime: Timestamp::from_unix_millis(1),
                observed_at: Timestamp::from_unix_millis(1),
                denied: false,
                bytes: Some(B::copy_from_slice(bytes)),
            }],
            removed: Vec::new(),
            skipped: Vec::new(),
            during_job: None,
            more: false,
            part: 0,
            gap: None,
        })
    };

    // In a message, whole, and as part of a longer file.
    let mut long = b"prefix ".to_vec();
    long.extend(&binary.bytes);
    long.extend(b" suffix");
    assert_eq!(capture.in_message(&message_with(&text.bytes, "a.yml")), ["text"]);
    assert_eq!(capture.in_message(&message_with(&long, "a.yml")), ["binary"]);
    // In something that is not a content field: a path.
    assert_eq!(
        capture.in_message(&message_with(b"clean", "svc/MARKER-text-0001.yml")),
        ["text"]
    );
    assert!(
        capture
            .in_message(&message_with(b"clean", "svc/a.yml"))
            .is_empty()
    );
    assert!(
        capture
            .in_message(&message_with(b"MARKER-text-000", "a.yml"))
            .is_empty()
    );

    // In a spool file.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("seg-1.lks"),
        [b"xx".as_slice(), &binary.bytes].concat(),
    )
    .unwrap();
    fs::write(dir.path().join("state"), b"nothing here").unwrap();
    assert_eq!(capture.in_spool(dir.path()), ["binary in seg-1.lks"]);

    // In the logs and in the metrics.
    {
        use std::io::Write;
        let mut writer = tracing_subscriber::fmt::MakeWriter::make_writer(capture.logs());
        writer.write_all(b"a line with MARKER-text-0001 in it\n").unwrap();
    }
    assert_eq!(capture.in_logs(), ["text in the logs"]);
    assert_eq!(
        capture.in_metrics("lk_x 1\nMARKER-text-0001 2\n"),
        ["text in the metrics"]
    );
    assert!(capture.in_metrics("lk_x 1\n").is_empty());
}

#[allow(
    clippy::too_many_lines,
    reason = "one scenario told in order: each step builds on the files and the connection of the one before"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_file_bytes_never_on_any_outbound_message_spool_or_log() {
    let all = markers();
    let capture = Capture::new(all.clone());
    capture.logs().install_global();

    // The export: ordinary files, and one denied file for each built-in glob (some in capitals, some deep, one only by the
    // name of its directory). `svc/late.yml` is ordinary until the hub says otherwise.
    let pem = with_marker(marker(&all, "denied_pem"), "-----BEGIN PRIVATE KEY-----");
    let jks = with_marker(marker(&all, "denied_jks"), "JKS");
    let private_dir = with_marker(marker(&all, "denied_private_dir"), "inside a private directory");
    let a = with_marker(marker(&all, "visible_a"), "a: 1");
    let dir = TempDir::new().unwrap();
    put(dir.path(), "keys/server.pem", &pem);
    put(dir.path(), "keys/DEEP/ER/Store.JKS", &jks);
    put(dir.path(), "private/notes.yml", &private_dir);
    put(dir.path(), "svc/a.yml", &a);
    put(dir.path(), "svc/late.yml", b"late: not yet special\n");

    let rig = Rig::real_time();
    rig.server.set_config(config(&["*.secret"]));
    let app = AppRig::start_on(
        rig,
        Setup {
            source: None,
            root: Some(dir),
            ..Setup::default()
        },
    )
    .await;
    let conn = app.connected().await;

    // ---- 1. The hub asks for everything. Denied files come as name, size and hash; the ordinary one with its bytes.
    conn.send(ToAgent::Command(HubCommand::RequestFullScan)).await;
    let pem_entry = wait_for_entry(&conn, "keys/server.pem").await;
    assert_denied_entry(&pem_entry, &pem);
    assert_denied_entry(&wait_for_entry(&conn, "keys/DEEP/ER/Store.JKS").await, &jks);
    assert_denied_entry(&wait_for_entry(&conn, "private/notes.yml").await, &private_dir);
    let a_entry = wait_for_entry(&conn, "svc/a.yml").await;
    assert!(!a_entry.denied);
    assert_eq!(
        a_entry.bytes.as_deref(),
        Some(&a[..]),
        "an ordinary file is sent whole"
    );

    // ---- 2. New files appear on the export while the agent runs: a keystore, a file the hub's glob denies, an ordinary one.
    let keystore = with_marker(marker(&all, "denied_keystore"), "keystore");
    let hub_glob = with_marker(marker(&all, "denied_hub_glob"), "hub says secret");
    let b = with_marker(marker(&all, "visible_b"), "b: 2");
    let root = app.dir.path().to_path_buf();
    put(&root, "svc/new.keystore", &keystore);
    put(&root, "svc/x.secret", &hub_glob);
    put(&root, "svc/b.yml", &b);
    let b_entry = wait_for_entry(&conn, "svc/b.yml").await;
    assert_eq!(b_entry.bytes.as_deref(), Some(&b[..]));
    assert_denied_entry(&wait_for_entry(&conn, "svc/new.keystore").await, &keystore);
    assert_denied_entry(&wait_for_entry(&conn, "svc/x.secret").await, &hub_glob);

    // ---- 3. The hub's commands on denied paths: refused with DENIED, no hash, nothing changed.
    let written = with_marker(marker(&all, "denied_written"), "planted by the hub");
    let read = command(
        &conn,
        "read-pem",
        HubCommand::ReadFile {
            request_id: rid("read-pem"),
            path: nfs("keys/server.pem"),
        },
    )
    .await;
    let read = op(read);
    assert_eq!(
        (read.ok, read.error, read.current_hash),
        (false, Some(OpError::Denied), None)
    );
    for (id, path) in [
        ("read-jks", "keys/DEEP/ER/Store.JKS"),
        ("read-private", "private/notes.yml"),
    ] {
        let result = op(command(
            &conn,
            id,
            HubCommand::ReadFile {
                request_id: rid(id),
                path: nfs(path),
            },
        )
        .await);
        assert_eq!(result.error, Some(OpError::Denied), "{path}");
    }
    let write = op(command(
        &conn,
        "write-wrong-hash",
        HubCommand::WriteFile {
            request_id: rid("write-wrong-hash"),
            path: nfs("keys/server.pem"),
            expected: Expected::Hash {
                hash: hash_of(b"a guess"),
            },
            bytes: Bytes::from(written.clone()),
        },
    )
    .await);
    assert_eq!(
        write.error,
        Some(OpError::Denied),
        "DENIED, not a CONFLICT with the current hash"
    );
    assert_eq!(write.current_hash, None);
    let write = op(command(
        &conn,
        "write-create",
        HubCommand::WriteFile {
            request_id: rid("write-create"),
            path: nfs("keys/created.pem"),
            expected: Expected::Absent,
            bytes: Bytes::from(written.clone()),
        },
    )
    .await);
    assert_eq!(write.error, Some(OpError::Denied));
    let delete = op(command(
        &conn,
        "delete-pem",
        HubCommand::DeleteFile {
            request_id: rid("delete-pem"),
            path: nfs("keys/server.pem"),
            expected: hash_of(&pem),
        },
    )
    .await);
    assert_eq!(delete.error, Some(OpError::Denied));
    assert_eq!(
        fs::read(root.join("keys/server.pem")).unwrap(),
        pem,
        "the key is untouched"
    );
    assert!(!root.join("keys/created.pem").exists(), "nothing was created");
    // And the agent still does its ordinary work: an ordinary read answers with the file.
    let ordinary = command(
        &conn,
        "read-a",
        HubCommand::ReadFile {
            request_id: rid("read-a"),
            path: nfs("svc/a.yml"),
        },
    )
    .await;
    assert!(
        matches!(&ordinary, FromAgent::Reply(AgentReply::File { bytes, .. }) if bytes[..] == a[..]),
        "{ordinary:?}"
    );

    // ---- 4. A path becomes denied after it was scanned and spooled. The hub goes away; the file changes and is spooled
    //         (with its bytes, because nothing denies it yet); the hub comes back with a new configuration that denies it.
    let before = app.rig.server.connection_count();
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    let spooled_before = app.spool.stats().entries;
    let late = with_marker(
        marker(&all, "denied_late"),
        "late: changed while the hub was away",
    );
    put(&root, "svc/late.yml", &late);
    let mut waited = Duration::ZERO;
    while app.spool.stats().entries <= spooled_before {
        assert!(waited < REACHES_THE_HUB, "the change was never spooled");
        sleep(Duration::from_millis(250)).await;
        waited += Duration::from_millis(250);
    }
    app.rig.server.set_config(config(&["*.secret", "late.yml"]));
    app.rig.server.net.set_reachable(true);
    let conn2 = timeout(REACHES_THE_HUB, app.rig.server.wait_for_connection(before + 1))
        .await
        .expect("the agent reconnects");
    let replayed = wait_for_entry(&conn2, "svc/late.yml").await;
    assert_denied_entry(&replayed, &late);
    // The same, when the hub asks for the whole tree on this connection.
    conn2.send(ToAgent::Command(HubCommand::RequestFullScan)).await;
    let full = timeout(REACHES_THE_HUB, async {
        loop {
            let entries = entries_for(&conn2, "svc/late.yml");
            if entries.len() >= 2 {
                return entries;
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the full listing arrives");
    for entry in &full {
        assert_denied_entry(entry, &late);
    }
    // The hub's earlier glob is still in force, and a file it denies is refused on the new connection too.
    let result = op(command(
        &conn2,
        "read-late",
        HubCommand::ReadFile {
            request_id: rid("read-late"),
            path: nfs("svc/late.yml"),
        },
    )
    .await);
    assert_eq!(result.error, Some(OpError::Denied));

    // ---- Where did the markers go?
    let connections = app.rig.server.connection_count();
    let mut on_the_stream = Vec::new();
    for index in 0..connections {
        on_the_stream.extend(capture.on_stream(&app.rig.server.connection(index)));
    }
    let leaked: Vec<_> = on_the_stream
        .iter()
        .filter(|found| !found.starts_with("visible_"))
        .collect();
    assert!(leaked.is_empty(), "denied bytes reached the hub: {leaked:#?}");
    for visible in ["visible_a", "visible_b"] {
        assert!(
            on_the_stream.iter().any(|found| found.starts_with(visible)),
            "{visible} never reached the hub: the search would not have noticed a leak"
        );
    }

    // The spool holds what the agent sent for the hub. The one marker allowed in it is the file that was spooled before
    // the hub denied it: it was an ordinary file when it was written, and it is stripped when it is replayed (the
    // threat model names this as a residual risk). Every other denied marker is in no segment.
    let in_spool = capture.in_spool(app.spool_dir.path());
    let leaked: Vec<_> = in_spool
        .iter()
        .filter(|found| !found.starts_with("visible_") && !found.starts_with("denied_late"))
        .collect();
    assert!(
        leaked.is_empty(),
        "denied bytes were written to the spool: {leaked:#?}"
    );

    // Logs and metrics never hold content, ordinary files' content included.
    let in_logs = capture.in_logs();
    assert!(in_logs.is_empty(), "file content reached the logs: {in_logs:#?}");
    let metrics = app.metrics.render();
    let in_metrics = capture.in_metrics(&metrics);
    assert!(
        in_metrics.is_empty(),
        "file content reached the metrics: {in_metrics:#?}"
    );
    // The log capture was not blind either: it holds what the agent says about denied files, by name only.
    assert!(
        capture.logs().text().contains("keys/server.pem"),
        "the agent logs its refusals by path; the capture saw no log at all"
    );
}
