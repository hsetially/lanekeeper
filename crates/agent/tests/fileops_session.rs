//! File commands on a real connection (T5, S10, S11, S17): the hub sends `ReadFile`, `WriteFile` and `DeleteFile` over
//! TLS 1.3 and gets exactly one answer each, the tree learns about the writes without a delta, a hostile command never
//! reaches the file operations, and no answer or log line holds a path's content or the OS's words.
//!
//! The agent runs on a real temporary directory in real time (the file system calls are real blocking calls).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::os::unix::fs::symlink;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::FileOps;
use agent::root::NfsRoot;
use agent::transport::wire;
use agent::tree::{FsSource, Pool, ScanMode, TreeSource, WalkConfig};
use async_trait::async_trait;
use bytes::Bytes;
use domain::{AgentReply, ContentHash, Expected, HubCommand, NfsPath, OpError, RequestId};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::log_capture::LogCapture;
use support::oob::Harness;
use support::rig::Rig;
use support::tree::sha;
use tempfile::TempDir;
use tokio::sync::Semaphore;

fn hash_of(content: &[u8]) -> ContentHash {
    ContentHash::from_bytes(sha(content))
}

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn path(text: &str) -> NfsPath {
    NfsPath::parse(text).unwrap()
}

fn source_over(dir: &std::path::Path) -> Arc<FsSource> {
    Arc::new(FsSource::new(
        NfsRoot::open(dir).unwrap(),
        Pool::new(2).unwrap(),
        WalkConfig::default(),
    ))
}

/// The agent over a real directory, with the real dispatcher.
async fn start(dir: &TempDir) -> Harness {
    let root = NfsRoot::open(dir.path()).unwrap();
    Harness::start_with_commands(source_over(dir.path()), Rig::new(), move |scanner| {
        let ops = FileOps::new(root, Arc::new(scanner.clone()));
        Some(Arc::new(Dispatcher::new(ops)) as Arc<dyn CommandHandler>)
    })
    .await
}

async fn reply_to(harness: &Harness, request_id: &str) -> AgentReply {
    let wanted = request_id.to_owned();
    let message = tokio::time::timeout(
        Duration::from_secs(30),
        harness.conn.wait_for(move |m| match m {
            FromAgent::Reply(AgentReply::Op(r)) => r.request_id.as_str() == wanted,
            FromAgent::Reply(AgentReply::File { request_id, .. }) => request_id.as_str() == wanted,
            _ => false,
        }),
    )
    .await
    .expect("the hub gets an answer");
    let FromAgent::Reply(reply) = message else {
        unreachable!()
    };
    reply
}

fn op(reply: AgentReply) -> domain::OpResult {
    match reply {
        AgentReply::Op(result) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

fn deltas(harness: &Harness) -> usize {
    harness
        .conn
        .received()
        .iter()
        .filter(|m| matches!(m, FromAgent::Delta(_)))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_can_read_write_and_delete_over_the_stream() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("svc")).unwrap();
    let harness = start(&dir).await;
    let content: &[u8] = b"\xef\xbb\xbfserver:\r\n  port: 8080\r\n";

    harness
        .conn
        .send(ToAgent::Command(HubCommand::WriteFile {
            request_id: rid("w1"),
            path: path("svc/app.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from_static(content),
        }))
        .await;
    let written = op(reply_to(&harness, "w1").await);
    assert!(written.ok, "{written:?}");
    assert_eq!(written.current_hash, Some(hash_of(content)));
    assert_eq!(fs::read(dir.path().join("svc/app.yml")).unwrap(), content);

    harness
        .conn
        .send(ToAgent::Command(HubCommand::ReadFile {
            request_id: rid("r1"),
            path: path("svc/app.yml"),
        }))
        .await;
    let AgentReply::File {
        hash,
        bytes,
        path: read_path,
        ..
    } = reply_to(&harness, "r1").await
    else {
        panic!("file content");
    };
    assert_eq!(
        (&bytes[..], hash, read_path.as_str()),
        (content, hash_of(content), "svc/app.yml")
    );

    // A stale hash: refused, with the hash that is really there.
    harness
        .conn
        .send(ToAgent::Command(HubCommand::DeleteFile {
            request_id: rid("d1"),
            path: path("svc/app.yml"),
            expected: hash_of(b"stale"),
        }))
        .await;
    let refused = op(reply_to(&harness, "d1").await);
    assert!(!refused.ok);
    assert_eq!(refused.error, Some(OpError::Conflict));
    assert_eq!(refused.current_hash, Some(hash_of(content)));
    assert!(dir.path().join("svc/app.yml").exists());

    harness
        .conn
        .send(ToAgent::Command(HubCommand::DeleteFile {
            request_id: rid("d2"),
            path: path("svc/app.yml"),
            expected: hash_of(content),
        }))
        .await;
    assert!(op(reply_to(&harness, "d2").await).ok);
    assert!(!dir.path().join("svc/app.yml").exists());

    // And a missing file is its own answer.
    harness
        .conn
        .send(ToAgent::Command(HubCommand::ReadFile {
            request_id: rid("r2"),
            path: path("svc/app.yml"),
        }))
        .await;
    assert_eq!(op(reply_to(&harness, "r2").await).error, Some(OpError::NotFound));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_from_the_hub_updates_the_tree_and_is_not_sent_back_as_a_delta() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("existing.yml"), b"a: 1\n").unwrap();
    let harness = start(&dir).await;
    let before = harness.scanner.snapshot().unwrap().root;

    harness
        .conn
        .send(ToAgent::Command(HubCommand::WriteFile {
            request_id: rid("w2"),
            path: path("new.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"b: 2\n"),
        }))
        .await;
    assert!(op(reply_to(&harness, "w2").await).ok);

    // The tree has the file at once ...
    let after = harness.scanner.snapshot().unwrap();
    assert_ne!(after.root, before);
    assert_eq!(after.file_count, 2);
    // ... exactly as a fresh full walk sees the disk.
    let walked = source_over(dir.path())
        .scan(None, ScanMode::Full)
        .await
        .unwrap()
        .tree;
    assert_eq!(after.root, walked.root_hash());

    // The next walk finds nothing new to say: the stat in the tree is the stat on disk.
    harness.scanner.scan_once().await;
    harness.scanner.scan_once().await;
    assert_eq!(harness.scanner.snapshot().unwrap().root, after.root);
    assert_eq!(
        deltas(&harness),
        0,
        "a tool write is not pushed as an out-of-band change"
    );

    // A delete is the same.
    harness
        .conn
        .send(ToAgent::Command(HubCommand::DeleteFile {
            request_id: rid("d3"),
            path: path("new.yml"),
            expected: hash_of(b"b: 2\n"),
        }))
        .await;
    assert!(op(reply_to(&harness, "d3").await).ok);
    assert_eq!(harness.scanner.snapshot().unwrap().root, before);
    harness.scanner.scan_once().await;
    assert_eq!(deltas(&harness), 0);
}

/// Counts the commands that reach it, and answers `UNSUPPORTED`.
#[derive(Debug, Default)]
struct Counting {
    calls: AtomicUsize,
}

#[async_trait]
impl CommandHandler for Counting {
    async fn handle(&self, command: HubCommand, _limits: OpLimits) -> Option<FromAgent> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let id = agent::dispatch::request_id_of(&command)?.clone();
        Some(agent::dispatch::failure(id, OpError::Unsupported, None))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_command_is_answered_denied_before_io() {
    use pb::hub_message::Kind;
    let dir = TempDir::new().unwrap();
    let counting = Arc::new(Counting::default());
    let handler: Arc<dyn CommandHandler> = counting.clone();
    let harness = Harness::start_with_commands(source_over(dir.path()), Rig::new(), |_| Some(handler)).await;

    let write = |id: &str, path: &str| pb::HubMessage {
        kind: Some(Kind::WriteFile(pb::WriteFile {
            request_id: id.to_owned(),
            path: path.to_owned(),
            expected: Some(pb::Expected {
                state: Some(pb::expected::State::Absent(pb::expected::Absent {})),
            }),
            content: Bytes::from_static(b"x"),
        })),
    };
    for (n, hostile) in ["../etc/passwd", "/abs", "a/../../b", "a\0b", "a\\b", ""]
        .into_iter()
        .enumerate()
    {
        harness.conn.send_raw(write(&format!("h{n}"), hostile)).await;
        let denied = op(reply_to(&harness, &format!("h{n}")).await);
        assert_eq!(denied.error, Some(OpError::Denied), "{hostile:?}");
    }
    // A hash that is not 32 bytes is refused the same way.
    harness
        .conn
        .send_raw(pb::HubMessage {
            kind: Some(Kind::DeleteFile(pb::DeleteFile {
                request_id: "h-hash".into(),
                path: "a.yml".into(),
                expected_hash: Bytes::from_static(b"short"),
            })),
        })
        .await;
    assert_eq!(
        op(reply_to(&harness, "h-hash").await).error,
        Some(OpError::Denied)
    );
    assert_eq!(
        counting.calls.load(Ordering::SeqCst),
        0,
        "nothing hostile got as far as the handler"
    );

    // The stream is fine, and a valid command does arrive.
    harness
        .conn
        .send(ToAgent::Command(HubCommand::ReadFile {
            request_id: rid("good"),
            path: path("a.yml"),
        }))
        .await;
    assert_eq!(
        op(reply_to(&harness, "good").await).error,
        Some(OpError::Unsupported)
    );
    assert_eq!(counting.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_this_build_does_not_carry_out_are_answered_unsupported() {
    let dir = TempDir::new().unwrap();
    let harness = start(&dir).await;
    harness
        .conn
        .send(ToAgent::Command(HubCommand::RequestClusterReport {
            request_id: rid("c1"),
        }))
        .await;
    harness
        .conn
        .send(ToAgent::Command(HubCommand::NotifyConfigServer {
            request_id: rid("n1"),
            paths: vec![path("a.yml")],
        }))
        .await;
    assert_eq!(
        op(reply_to(&harness, "c1").await).error,
        Some(OpError::Unsupported)
    );
    assert_eq!(
        op(reply_to(&harness, "n1").await).error,
        Some(OpError::Unsupported)
    );
}

/// Holds every command until the test lets it go.
#[derive(Debug)]
struct Parked {
    gate: Arc<Semaphore>,
    started: AtomicUsize,
}

#[async_trait]
impl CommandHandler for Parked {
    async fn handle(&self, command: HubCommand, _limits: OpLimits) -> Option<FromAgent> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let _permit = self.gate.acquire().await.unwrap();
        let id = agent::dispatch::request_id_of(&command)?.clone();
        Some(agent::dispatch::failure(id, OpError::NotFound, None))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_many_commands_at_once_are_answered_busy_not_queued() {
    let dir = TempDir::new().unwrap();
    let parked = Arc::new(Parked {
        gate: Arc::new(Semaphore::new(0)),
        started: AtomicUsize::new(0),
    });
    let handler: Arc<dyn CommandHandler> = parked.clone();
    let harness = Harness::start_with_commands(source_over(dir.path()), Rig::new(), |_| Some(handler)).await;

    for n in 0..5 {
        harness
            .conn
            .send(ToAgent::Command(HubCommand::ReadFile {
                request_id: rid(&format!("p{n}")),
                path: path("a.yml"),
            }))
            .await;
    }
    // The fifth is over the limit of four running at once: it is answered at once, with a code, and never runs.
    let busy = op(reply_to(&harness, "p4").await);
    assert_eq!(busy.error, Some(OpError::Io));
    assert!(parked.started.load(Ordering::SeqCst) <= 4);
    parked.gate.add_permits(4);
    for n in 0..4 {
        assert_eq!(
            op(reply_to(&harness, &format!("p{n}")).await).error,
            Some(OpError::NotFound)
        );
    }
    assert_eq!(parked.started.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn error_replies_carry_codes_only() {
    const NAME: &str = "marker-dir-71c2/marker-file-9d4e.yml";
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("root");
    fs::create_dir_all(root.join("marker-dir-71c2")).unwrap();
    fs::write(root.join(NAME), b"present").unwrap();
    symlink(
        "marker-file-9d4e.yml",
        root.join("marker-dir-71c2/marker-link-5b0a.yml"),
    )
    .unwrap();
    fs::create_dir(root.join("marker-dir-71c2/marker-sub-3e8f")).unwrap();
    let dispatcher = Dispatcher::new(FileOps::new(
        NfsRoot::open(&root).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    ));
    let limits = OpLimits {
        max_file_bytes: 2 * 1024 * 1024,
    };

    let commands = [
        HubCommand::ReadFile {
            request_id: rid("e1"),
            path: path("marker-dir-71c2/missing-1a2b.yml"),
        },
        HubCommand::ReadFile {
            request_id: rid("e2"),
            path: path("marker-dir-71c2/marker-link-5b0a.yml"),
        },
        HubCommand::ReadFile {
            request_id: rid("e3"),
            path: path("marker-dir-71c2/marker-sub-3e8f"),
        },
        HubCommand::WriteFile {
            request_id: rid("e4"),
            path: path(NAME),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"x"),
        },
        HubCommand::WriteFile {
            request_id: rid("e5"),
            path: path("no-such-dir-44aa/f.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"x"),
        },
        HubCommand::DeleteFile {
            request_id: rid("e6"),
            path: path(NAME),
            expected: hash_of(b"wrong"),
        },
    ];
    for command in commands {
        let reply = dispatcher.handle(command, limits).await.unwrap();
        let encoded = prost::Message::encode_to_vec(&wire::encode(reply));
        let text = String::from_utf8_lossy(&encoded);
        for forbidden in [
            "marker-",
            "no-such-dir",
            "missing-1a2b",
            "/tmp",
            "os error",
            "No such file",
            "root",
        ] {
            assert!(
                !text.contains(forbidden),
                "an answer contains {forbidden:?}: {text:?}"
            );
        }
    }
}

#[tokio::test]
async fn log_capture_contains_only_paths_and_hashes() {
    const MARKER: &str = "FILE-CONTENT-MARKER-77ab";
    let capture = LogCapture::new();
    let _guard = capture.install();
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("svc")).unwrap();
    fs::write(dir.path().join("svc/old.yml"), format!("password: {MARKER}")).unwrap();
    let dispatcher = Dispatcher::new(FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    ));
    let limits = OpLimits {
        max_file_bytes: 2 * 1024 * 1024,
    };
    let content = format!("token: {MARKER}\n");
    for command in [
        HubCommand::ReadFile {
            request_id: rid("l1"),
            path: path("svc/old.yml"),
        },
        HubCommand::WriteFile {
            request_id: rid("l2"),
            path: path("svc/new.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from(content.clone()),
        },
        // A conflict, a missing file and a refused path: the log lines of the failure paths.
        HubCommand::WriteFile {
            request_id: rid("l3"),
            path: path("svc/new.yml"),
            expected: Expected::Absent,
            bytes: Bytes::from(content),
        },
        HubCommand::ReadFile {
            request_id: rid("l4"),
            path: path("svc/missing.yml"),
        },
        HubCommand::ReadFile {
            request_id: rid("l5"),
            path: path("svc/.lanekeeper-tmp-x"),
        },
    ] {
        dispatcher.handle(command, limits).await.unwrap();
    }
    let log = capture.text();
    assert!(log.contains("svc/new.yml"), "the path is logged: {log}");
    assert!(!log.contains(MARKER), "file content reached a log line");
    assert!(!log.contains("password:") && !log.contains("token:"));
}
