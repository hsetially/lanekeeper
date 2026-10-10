//! Deny globs in the tree and in deltas (T11, D79, S17): a denied file is walked and hashed by streaming, its leaf says
//! `denied`, and no delta ever carries its bytes, whatever the leaf says and whenever the glob arrived.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

use agent::clock::Clock;
use agent::deny::DenyList;
use agent::root::NfsRoot;
use agent::tree::delta::{DeltaBuilder, SeqCounter};
use agent::tree::hash::{HASH_LIMIT, hash_stream};
use agent::tree::walk::{ScanMode, WalkConfig, walk};
use agent::tree::{
    Entry, FsSource, MerkleTree, OtherReason, Pool, ReadError, ReadRequest, ScanOutcome, TreeSource,
};
use domain::{NfsPath, Timestamp};
use support::collect_sink::CollectSink;
use support::scripted_source::ScriptedSource;
use support::tree::sha;
use tempfile::TempDir;

/// Content that must never reach a delta, in a form no ordinary config file would contain.
const SECRET: &[u8] =
    b"-----BEGIN PRIVATE KEY-----\nMARKER-7f3a91c2-do-not-send\n-----END PRIVATE KEY-----\n";

fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn scan(dir: &Path, previous: Option<&MerkleTree>, mode: ScanMode, cfg: &WalkConfig) -> ScanOutcome {
    let root = NfsRoot::open(dir).unwrap();
    walk(&Pool::new(2).unwrap(), root.dir(), previous, mode, cfg).unwrap()
}

fn leaves(tree: &MerkleTree) -> std::collections::BTreeMap<String, agent::tree::FileLeaf> {
    tree.files().into_iter().collect()
}

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000)
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

// ------------------------------------------------------------------------------------------------ the walk

#[test]
fn the_built_in_globs_apply_to_a_walk_with_no_configuration() {
    let dir = TempDir::new().unwrap();
    for name in [
        "a.jks",
        "b.p12",
        "c.pfx",
        "d.pem",
        "e.key",
        "f.keystore",
        "g-private-x.yml",
        "UPPER.KEY",
    ] {
        put(dir.path(), &format!("svc/{name}"), SECRET);
    }
    put(dir.path(), "svc/plain.yml", b"a: 1\n");
    let out = scan(dir.path(), None, ScanMode::Full, &WalkConfig::default());
    let map = leaves(&out.tree);
    assert_eq!(map.len(), 9);
    for (path, leaf) in &map {
        assert_eq!(leaf.denied, path != "svc/plain.yml", "{path}");
    }
    // Hashed, though: a denied file is seen to exist and to change.
    assert_eq!(map["svc/d.pem"].hash.as_bytes(), &sha(SECRET));
    assert_eq!(map["svc/d.pem"].stat.size, SECRET.len() as u64);
}

#[test]
fn a_glob_added_after_a_scan_marks_the_file_on_the_next_stat_walk_and_removing_it_releases_it() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "svc/settings.secret", b"not special yet");
    put(dir.path(), "svc/app.yml", b"a: 1\n");
    let deny = DenyList::default();
    let cfg = WalkConfig::default().with_deny_list(deny.clone());

    let first = scan(dir.path(), None, ScanMode::Full, &cfg);
    assert!(!leaves(&first.tree)["svc/settings.secret"].denied);

    // The hub's configuration arrives: nothing on disk changed, but the next walk, a stat walk, re-marks the leaf.
    assert!(deny.set_hub_globs(&["*.secret"]).changed);
    let second = scan(dir.path(), Some(&first.tree), ScanMode::Stat, &cfg);
    let map = leaves(&second.tree);
    assert!(map["svc/settings.secret"].denied);
    assert!(!map["svc/app.yml"].denied);
    assert_eq!(
        map["svc/settings.secret"].hash.as_bytes(),
        &sha(b"not special yet")
    );
    assert_eq!(second.stats.hashed, 1, "only the re-marked file was read again");
    // The root commits to content, not to policy, so a mark alone does not move it (and nothing is re-reported until the
    // file changes). What matters is that from here on no delta can carry the file's bytes.
    assert_eq!(second.tree.root_hash(), first.tree.root_hash());

    // And the hub takes its glob back: the file is released on the next walk. The built-ins are not affected.
    deny.set_hub_globs(&[]);
    put(dir.path(), "svc/keep.pem", SECRET);
    let third = scan(dir.path(), Some(&second.tree), ScanMode::Stat, &cfg);
    let map = leaves(&third.tree);
    assert!(!map["svc/settings.secret"].denied);
    assert!(map["svc/keep.pem"].denied);
}

#[test]
fn one_walk_uses_the_list_as_it_was_when_it_began() {
    // The snapshot is what makes a walk consistent: a glob added by the hub half way through applies to the next walk.
    let deny = DenyList::default();
    let before = deny.snapshot();
    deny.set_hub_globs(&["*.late"]);
    assert!(!before.is_denied("x.late"));
    assert!(deny.snapshot().is_denied("x.late"));
}

#[test]
fn denied_hash_streaming_memory_bounded() {
    // A reader that is 64 MiB of zeros and remembers the largest buffer it was asked to fill. Hashing a denied file is
    // the same code as hashing any file, and it reads through one fixed buffer, never the file.
    struct Zeros {
        left: u64,
        largest_buffer: usize,
        reads: u64,
    }
    impl Read for Zeros {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.largest_buffer = self.largest_buffer.max(buf.len());
            self.reads += 1;
            let n = usize::try_from(self.left.min(buf.len() as u64)).unwrap();
            buf[..n].fill(0);
            self.left -= n as u64;
            Ok(n)
        }
    }
    let mut zeros = Zeros {
        left: HASH_LIMIT,
        largest_buffer: 0,
        reads: 0,
    };
    let (_, read) = hash_stream(&mut zeros, HASH_LIMIT).unwrap().unwrap();
    assert_eq!(read, HASH_LIMIT);
    assert!(
        zeros.largest_buffer <= 64 * 1024,
        "the buffer was {} bytes",
        zeros.largest_buffer
    );
    assert!(zeros.reads >= HASH_LIMIT / (64 * 1024), "{} reads", zeros.reads);

    // Through the walker, on a real denied file of several buffers: the hash is of the whole file.
    let dir = TempDir::new().unwrap();
    let big: Vec<u8> = (0..(300 * 1024))
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    put(dir.path(), "keys/big.keystore", &big);
    let out = scan(dir.path(), None, ScanMode::Full, &WalkConfig::default());
    let leaf = leaves(&out.tree)["keys/big.keystore"];
    assert!(leaf.denied);
    assert_eq!(leaf.hash.as_bytes(), &sha(&big));
    assert_eq!(out.stats.hashed_bytes, big.len() as u64);
}

#[test]
fn a_denied_file_over_the_hash_limit_is_kind_2_and_still_has_no_bytes_anywhere() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "keys/huge.p12", &vec![9_u8; 5000]);
    let cfg = WalkConfig::default().with_hash_limit(4096);
    let out = scan(dir.path(), None, ScanMode::Full, &cfg);
    assert!(matches!(
        out.tree.get("keys/huge.p12"),
        Some(Entry::Other(OtherReason::TooLarge))
    ));
}

// ------------------------------------------------------------------------------------------------ the source

fn source_over(dir: &Path) -> FsSource {
    FsSource::new(
        NfsRoot::open(dir).unwrap(),
        Pool::new(2).unwrap(),
        WalkConfig::default(),
    )
}

fn read_request(path: &str) -> ReadRequest {
    ReadRequest {
        path: NfsPath::parse(path).unwrap(),
        max_bytes: 1024 * 1024,
    }
}

#[tokio::test]
async fn the_source_refuses_to_read_a_denied_path_and_reads_the_rest() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "keys/server.pem", SECRET);
    put(dir.path(), "svc/app.yml", b"a: 1\n");
    put(dir.path(), "svc/x.secret", b"hub-denied");
    let source = source_over(dir.path());
    let answers = source
        .read(vec![
            read_request("keys/server.pem"),
            read_request("svc/app.yml"),
            read_request("svc/x.secret"),
        ])
        .await;
    assert_eq!(answers[0].as_ref().unwrap_err(), &ReadError::Denied);
    assert_eq!(&answers[1].as_ref().unwrap().bytes[..], b"a: 1\n");
    assert!(answers[2].is_ok(), "not denied until the hub says so");

    // The hub's glob reaches the source through the list it hands out.
    source.deny().set_hub_globs(&["*.secret"]);
    let answers = source.read(vec![read_request("svc/x.secret")]).await;
    assert_eq!(answers[0].as_ref().unwrap_err(), &ReadError::Denied);
}

#[tokio::test]
async fn the_list_a_source_hands_out_is_the_list_it_walks_by() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "svc/x.secret", b"x");
    let source = source_over(dir.path());
    let tree = source.scan(None, ScanMode::Full).await.unwrap().tree;
    assert!(!leaves(&tree)["svc/x.secret"].denied);
    source.deny().set_hub_globs(&["*.secret"]);
    let tree = source.scan(Some(tree), ScanMode::Stat).await.unwrap().tree;
    assert!(leaves(&tree)["svc/x.secret"].denied);
}

// ------------------------------------------------------------------------------------------------ the delta builder

async fn send_all(
    source: &dyn TreeSource,
    base: Option<&MerkleTree>,
    tree: &MerkleTree,
) -> Vec<domain::ScanDelta> {
    let sink = CollectSink::new();
    let seq = SeqCounter::new(1);
    DeltaBuilder::new(source, &FixedClock, &seq)
        .send(base, tree, &sink)
        .await
        .unwrap();
    sink.take()
}

fn entries_of(deltas: &[domain::ScanDelta]) -> Vec<&domain::ScanEntry> {
    deltas.iter().flat_map(|d| d.entries.iter()).collect()
}

#[tokio::test]
async fn a_denied_file_goes_as_name_size_and_hash_and_never_as_bytes() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "keys/server.pem", SECRET);
    put(dir.path(), "svc/app.yml", b"a: 1\n");
    let source = source_over(dir.path());
    let tree = source.scan(None, ScanMode::Full).await.unwrap().tree;
    let deltas = send_all(&source, Some(&MerkleTree::empty()), &tree).await;
    let entries = entries_of(&deltas);
    assert_eq!(entries.len(), 2);
    let pem = entries
        .iter()
        .find(|e| e.path.as_str() == "keys/server.pem")
        .unwrap();
    assert!(pem.denied);
    assert!(pem.bytes.is_none());
    assert_eq!(pem.size, SECRET.len() as u64);
    assert_eq!(pem.hash.as_bytes(), &sha(SECRET));
    let app = entries.iter().find(|e| e.path.as_str() == "svc/app.yml").unwrap();
    assert!(!app.denied);
    assert_eq!(app.bytes.as_deref(), Some(&b"a: 1\n"[..]));
}

#[tokio::test]
async fn a_leaf_that_predates_the_glob_still_goes_without_bytes() {
    // The tree was walked before the hub's glob arrived, so its leaf is not marked; the builder asks the list again.
    let source = ScriptedSource::new();
    source.write("svc/x.secret", SECRET);
    source.write("svc/app.yml", b"a: 1\n");
    let tree = source.tree();
    assert!(!leaves(&tree)["svc/x.secret"].denied, "the leaf is not marked");
    source.deny().set_hub_globs(&["*.secret"]);
    let deltas = send_all(&source, Some(&MerkleTree::empty()), &tree).await;
    let entries = entries_of(&deltas);
    let secret = entries
        .iter()
        .find(|e| e.path.as_str() == "svc/x.secret")
        .unwrap();
    assert!(secret.denied && secret.bytes.is_none());
    assert_eq!(secret.hash.as_bytes(), &sha(SECRET));
    assert_eq!(source.reads(), 1, "only the plain file was read");
    for delta in &deltas {
        assert!(!format!("{delta:?}").contains("MARKER-7f3a91c2"));
    }
}

#[tokio::test]
async fn the_tree_the_builder_keeps_learns_the_mark() {
    let source = ScriptedSource::new();
    source.write("svc/x.secret", SECRET);
    let tree = source.tree();
    source.deny().set_hub_globs(&["*.secret"]);
    let sink = CollectSink::new();
    let seq = SeqCounter::new(1);
    let out = DeltaBuilder::new(&source, &FixedClock, &seq)
        .send(Some(&MerkleTree::empty()), &tree, &sink)
        .await
        .unwrap();
    assert!(leaves(&out.tree)["svc/x.secret"].denied);
}

#[tokio::test]
async fn a_file_denied_between_the_plan_and_the_read_goes_without_bytes() {
    // The hub's configuration arrives while a delta is being built: after the plan, before the files are read.
    let source = Arc::new(ScriptedSource::new());
    source.write("svc/x.late", SECRET);
    source.write("svc/app.yml", b"a: 1\n");
    let tree = source.tree();
    let deny = source.deny();
    source.before_next_refresh(move || {
        deny.set_hub_globs(&["*.late"]);
    });
    let deltas = send_all(source.as_ref(), Some(&MerkleTree::empty()), &tree).await;
    let entries = entries_of(&deltas);
    let late = entries.iter().find(|e| e.path.as_str() == "svc/x.late").unwrap();
    assert!(late.denied && late.bytes.is_none());
    assert_eq!(late.hash.as_bytes(), &sha(SECRET));
    for delta in &deltas {
        assert!(!format!("{delta:?}").contains("MARKER-7f3a91c2"));
    }
}

#[tokio::test]
async fn a_changed_denied_file_is_seen_to_change_by_its_hash_alone() {
    let dir = TempDir::new().unwrap();
    put(dir.path(), "keys/server.pem", b"version one");
    let source = source_over(dir.path());
    let first = source.scan(None, ScanMode::Full).await.unwrap().tree;
    put(dir.path(), "keys/server.pem", b"version two, longer");
    let second = source
        .scan(Some(first.clone()), ScanMode::Full)
        .await
        .unwrap()
        .tree;
    assert_ne!(first.root_hash(), second.root_hash());
    let deltas = send_all(&source, Some(&first), &second).await;
    let entries = entries_of(&deltas);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].denied && entries[0].bytes.is_none());
    assert_eq!(entries[0].hash.as_bytes(), &sha(b"version two, longer"));
}
