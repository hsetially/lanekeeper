//! Deny globs and the hub's file commands (T11, D79, S17): a denied path is refused for a read, a write and a delete, with
//! `DENIED` and no hash, before anything on disk is looked at.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use agent::deny::DenyList;
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::{DeniedReason, FileError, FileOps};
use agent::root::NfsRoot;
use bytes::Bytes;
use domain::{AgentReply, ContentHash, Expected, HubCommand, NfsPath, OpError, OpResult, RequestId};
use proto::convert::FromAgent;
use support::recording_edits::RecordingEdits;
use support::tree::sha;
use tempfile::TempDir;

const MAX: u64 = 2 * 1024 * 1024;
const LIMITS: OpLimits = OpLimits { max_file_bytes: MAX };
const SECRET: &[u8] =
    b"-----BEGIN PRIVATE KEY-----\nMARKER-4b2e07d1-do-not-read\n-----END PRIVATE KEY-----\n";

fn p(path: &str) -> NfsPath {
    NfsPath::parse(path).unwrap()
}

fn hash_of(content: &[u8]) -> ContentHash {
    ContentHash::from_bytes(sha(content))
}

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

struct Fixture {
    dir: TempDir,
    ops: FileOps,
    edits: Arc<RecordingEdits>,
    deny: DenyList,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let edits = RecordingEdits::new();
    let deny = DenyList::default();
    let ops = FileOps::new(NfsRoot::open(dir.path()).unwrap(), edits.clone()).with_deny(deny.clone());
    Fixture {
        dir,
        ops,
        edits,
        deny,
    }
}

const DENIED: FileError = FileError::Denied(DeniedReason::DenyGlob);

#[tokio::test]
async fn denied_read_write_delete_refused() {
    let f = fixture();
    put(f.dir.path(), "keys/server.pem", SECRET);
    put(f.dir.path(), "svc/store.JKS", SECRET);
    put(f.dir.path(), "svc/private-notes.yml", SECRET);

    for path in ["keys/server.pem", "svc/store.JKS", "svc/private-notes.yml"] {
        // Read: refused, and no bytes.
        assert_eq!(
            f.ops.read(&p(path), MAX).await.unwrap_err(),
            DENIED,
            "read {path}"
        );
        // Write with the right hash, with a wrong one, and as a create: all DENIED, none a conflict.
        for expected in [
            Expected::Hash {
                hash: hash_of(SECRET),
            },
            Expected::Hash {
                hash: hash_of(b"something else"),
            },
            Expected::Absent,
        ] {
            let outcome = f
                .ops
                .write(&p(path), expected, Bytes::from_static(b"planted"), MAX)
                .await;
            assert_eq!(outcome.unwrap_err(), DENIED, "write {path} {expected:?}");
        }
        // Delete with the right hash and with a wrong one.
        for expected in [hash_of(SECRET), hash_of(b"something else")] {
            assert_eq!(
                f.ops.delete(&p(path), expected).await.unwrap_err(),
                DENIED,
                "delete {path}"
            );
        }
        // Nothing changed on disk.
        assert_eq!(fs::read(f.dir.path().join(path)).unwrap(), SECRET, "{path}");
    }
    // The tree was told about nothing.
    assert!(f.edits.edits().is_empty(), "{:?}", f.edits.edits());
}

#[tokio::test]
async fn a_denied_path_that_does_not_exist_cannot_be_created() {
    let f = fixture();
    fs::create_dir_all(f.dir.path().join("keys")).unwrap();
    for path in [
        "keys/new.pem",
        "keys/new.KEY",
        "keys/id.keystore",
        "newdir/private.txt",
    ] {
        let outcome = f
            .ops
            .write(&p(path), Expected::Absent, Bytes::from_static(b"planted"), MAX)
            .await;
        assert_eq!(outcome.unwrap_err(), DENIED, "{path}");
        assert!(!f.dir.path().join(path).exists(), "{path} was created");
    }
    // A missing denied file is DENIED as well, not NOT_FOUND: the answer says nothing about what exists.
    assert_eq!(f.ops.read(&p("keys/missing.pem"), MAX).await.unwrap_err(), DENIED);
    assert_eq!(
        f.ops
            .delete(&p("keys/missing.pem"), hash_of(b"x"))
            .await
            .unwrap_err(),
        DENIED
    );
}

#[tokio::test]
async fn the_refusal_comes_before_every_other_check() {
    let f = fixture();
    // A write over the size limit, to a denied path: DENIED, not UNSUPPORTED. One answer for one reason.
    let huge = Bytes::from(vec![0_u8; usize::try_from(MAX).unwrap() + 1]);
    assert_eq!(
        f.ops
            .write(&p("keys/a.pem"), Expected::Absent, huge, MAX)
            .await
            .unwrap_err(),
        DENIED
    );
    // A reserved name that is also a denied one: either refusal is DENIED.
    let error = f.ops.read(&p(".lanekeeper-tmp-1.pem"), MAX).await.unwrap_err();
    assert!(matches!(error, FileError::Denied(_)), "{error:?}");
}

#[tokio::test]
async fn other_files_are_unaffected() {
    let f = fixture();
    put(f.dir.path(), "svc/app.yml", b"a: 1\n");
    put(f.dir.path(), "svc/keystore-notes.txt", b"not a key by its name");
    assert_eq!(
        &f.ops.read(&p("svc/app.yml"), MAX).await.unwrap().bytes[..],
        b"a: 1\n"
    );
    assert!(f.ops.read(&p("svc/keystore-notes.txt"), MAX).await.is_ok());
    let wrote = f
        .ops
        .write(
            &p("svc/app.yml"),
            Expected::Hash {
                hash: hash_of(b"a: 1\n"),
            },
            Bytes::from_static(b"a: 2\n"),
            MAX,
        )
        .await;
    assert!(wrote.is_ok(), "{wrote:?}");
}

#[tokio::test]
async fn a_glob_the_hub_adds_applies_to_the_next_command_and_taking_it_back_releases_the_file() {
    let f = fixture();
    put(f.dir.path(), "svc/x.secret", b"s");
    assert!(f.ops.read(&p("svc/x.secret"), MAX).await.is_ok());
    f.deny.set_hub_globs(&["*.secret"]);
    assert_eq!(f.ops.read(&p("svc/x.secret"), MAX).await.unwrap_err(), DENIED);
    f.deny.set_hub_globs(&[]);
    assert!(f.ops.read(&p("svc/x.secret"), MAX).await.is_ok());
    // The built-in globs stay, whatever the hub does.
    put(f.dir.path(), "svc/y.pem", b"p");
    assert_eq!(f.ops.read(&p("svc/y.pem"), MAX).await.unwrap_err(), DENIED);
}

#[tokio::test]
async fn file_ops_without_a_configured_list_still_follow_the_built_in_globs() {
    // `FileOps::new` is what a test or a tool gets: the safe default, never an empty list.
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a.key", SECRET);
    let ops = FileOps::new(NfsRoot::open(dir.path()).unwrap(), RecordingEdits::new());
    assert_eq!(ops.read(&p("a.key"), MAX).await.unwrap_err(), DENIED);
}

// ------------------------------------------------------------------------------------------------ the command level

fn op(reply: Option<FromAgent>) -> OpResult {
    match reply {
        Some(FromAgent::Reply(AgentReply::Op(result))) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

#[tokio::test]
async fn the_hub_gets_denied_and_no_hash_and_no_bytes() {
    let f = fixture();
    put(f.dir.path(), "keys/server.pem", SECRET);
    let dispatcher = Dispatcher::new(f.ops);

    let read = dispatcher
        .handle(
            HubCommand::ReadFile {
                request_id: rid("r1"),
                path: p("keys/server.pem"),
            },
            LIMITS,
        )
        .await;
    let shown = format!("{read:?}");
    assert!(!shown.contains("MARKER-4b2e07d1"), "{shown}");
    let result = op(read);
    assert_eq!(
        (result.ok, result.error, result.current_hash),
        (false, Some(OpError::Denied), None)
    );

    // A write with a wrong hash on a normal file would be a CONFLICT with the current hash; on a denied one it is DENIED
    // and says nothing about the content.
    let write = dispatcher
        .handle(
            HubCommand::WriteFile {
                request_id: rid("w1"),
                path: p("keys/server.pem"),
                expected: Expected::Hash {
                    hash: hash_of(b"guess"),
                },
                bytes: Bytes::from_static(b"planted"),
            },
            LIMITS,
        )
        .await;
    let result = op(write);
    assert_eq!(result.error, Some(OpError::Denied));
    assert_eq!(
        result.current_hash, None,
        "a conflict hash would leak the content's hash"
    );

    let delete = dispatcher
        .handle(
            HubCommand::DeleteFile {
                request_id: rid("d1"),
                path: p("keys/server.pem"),
                expected: hash_of(SECRET),
            },
            LIMITS,
        )
        .await;
    let result = op(delete);
    assert_eq!(
        (result.ok, result.error, result.current_hash),
        (false, Some(OpError::Denied), None)
    );
    assert_eq!(fs::read(f.dir.path().join("keys/server.pem")).unwrap(), SECRET);
}
