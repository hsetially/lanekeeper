//! Deltas (T4, D63, D44, Q12): what goes to the hub when the tree changed, in messages of at most 3 MiB, with the exact
//! bytes of the files and hashes that match them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;

use agent::clock::Clock;
use agent::tree::delta::{DeltaBuilder, DeltaError, SeqCounter};
use agent::tree::{Entry, MerkleTree, ReadError};
use domain::{JobRef, ScanDelta, Timestamp};
use proptest::prelude::*;
use prost::Message;
use proto::convert::FromAgent;
use support::collect_sink::CollectSink;
use support::scripted_source::ScriptedSource;
use support::tree::sha;

const MIB: usize = 1024 * 1024;

#[derive(Debug)]
struct FixedClock(i64);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.0)
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

const NOW_MS: i64 = 1_800_000_000_000;

fn encoded_len(delta: &ScanDelta) -> usize {
    FromAgent::Delta(delta.clone()).into_proto().encoded_len()
}

struct Rig {
    source: ScriptedSource,
    sink: CollectSink,
    clock: FixedClock,
    seq: SeqCounter,
}

impl Rig {
    fn new() -> Self {
        Self {
            source: ScriptedSource::new(),
            sink: CollectSink::new(),
            clock: FixedClock(NOW_MS),
            seq: SeqCounter::new(1),
        }
    }

    fn builder(&self) -> DeltaBuilder<'_> {
        DeltaBuilder::new(&self.source, &self.clock, &self.seq)
    }

    async fn send(
        &self,
        base: Option<&MerkleTree>,
        tree: &MerkleTree,
    ) -> Result<agent::tree::delta::DeltaOutcome, DeltaError> {
        self.builder().send(base, tree, &self.sink).await
    }
}

fn paths(entries: &[domain::ScanEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.path.as_str()).collect()
}

#[tokio::test]
async fn a_delta_carries_the_exact_bytes_and_hashes_that_match_them() {
    let rig = Rig::new();
    let crlf: &[u8] = b"a: 1\r\nb: 2\r\n";
    let bom: &[u8] = b"\xef\xbb\xbfkey: v\n";
    let binary: &[u8] = &[0, 255, 1, 254, 13, 10, 13];
    rig.source.write("svc/crlf.yml", crlf);
    rig.source.write("svc/bom.yml", bom);
    rig.source.write("img/logo.bmp", binary);
    let tree = rig.source.tree();
    let base = MerkleTree::empty();

    let out = rig.send(Some(&base), &tree).await.unwrap();
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    let d = &deltas[0];
    assert_eq!(d.base_root, Some(base.root_hash()));
    assert_eq!(d.new_root, tree.root_hash());
    assert_eq!(d.new_root, out.tree.root_hash());
    assert!(!d.more);
    assert_eq!(d.part, 0);
    assert_eq!(paths(&d.entries), ["img/logo.bmp", "svc/bom.yml", "svc/crlf.yml"]);
    for e in &d.entries {
        let bytes = e.bytes.as_ref().unwrap();
        assert_eq!(
            e.hash.as_bytes(),
            &sha(bytes),
            "{}: hash must be the SHA-256 of the bytes sent",
            e.path
        );
        assert_eq!(e.size, bytes.len() as u64);
        assert!(!e.denied);
        assert_eq!(e.observed_at, Timestamp::from_unix_millis(NOW_MS));
    }
    // Raw bytes: nothing normalised, byte for byte.
    let by_path: BTreeMap<_, _> = d
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.bytes.clone().unwrap()))
        .collect();
    assert_eq!(&by_path["svc/crlf.yml"][..], crlf);
    assert_eq!(&by_path["svc/bom.yml"][..], bom);
    assert_eq!(&by_path["img/logo.bmp"][..], binary);
}

#[tokio::test]
async fn a_full_listing_has_no_base_root() {
    let rig = Rig::new();
    rig.source.write("a", b"1");
    let tree = rig.source.tree();
    rig.send(None, &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(d.base_root, None);
    assert_eq!(paths(&d.entries), ["a"]);
}

#[tokio::test]
async fn a_delta_lists_changed_removed_and_skipped_paths() {
    let rig = Rig::new();
    rig.source.write("keep", b"k");
    rig.source.write("edit", b"before");
    rig.source.write("gone/a", b"a");
    rig.source.write("gone/b", b"b");
    let base = rig.source.tree();
    rig.source.write("edit", b"after");
    rig.source.write("new", b"n");
    rig.source.remove("gone/a");
    rig.source.remove("gone/b");
    let tree = rig
        .source
        .tree()
        .put("link", Entry::Other(agent::tree::OtherReason::Symlink))
        .tree;

    rig.send(Some(&base), &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["edit", "new"]);
    let removed: Vec<_> = d.removed.iter().map(domain::NfsPath::as_str).collect();
    assert_eq!(removed, ["gone/a", "gone/b"]);
    let skipped: Vec<_> = d
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.reason.as_str()))
        .collect();
    assert!(skipped.contains(&("link", "symlink")), "{skipped:?}");
}

#[tokio::test]
async fn files_over_2mib_skipped_and_reported() {
    let rig = Rig::new();
    rig.source.write("big.bin", &vec![9_u8; 2 * MIB + 1]);
    rig.source.write("exact.bin", &vec![8_u8; 2 * MIB]);
    rig.source.write("small", b"s");
    let tree = rig.source.tree();
    rig.builder()
        .with_max_file_bytes(2 * MIB as u64)
        .send(None, &tree, &rig.sink)
        .await
        .unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["exact.bin", "small"]);
    let skipped: Vec<_> = d
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(skipped, [("big.bin", "too_large")]);
    // The big file was never read.
    assert_eq!(rig.source.reads(), 2);
}

#[tokio::test]
async fn a_lower_limit_from_the_hub_skips_more() {
    let rig = Rig::new();
    rig.source.write("a", &[1; 600]);
    rig.source.write("b", &[2; 100]);
    let tree = rig.source.tree();
    rig.builder()
        .with_max_file_bytes(500)
        .send(None, &tree, &rig.sink)
        .await
        .unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["b"]);
    assert_eq!(d.skipped[0].path.as_str(), "a");
}

#[tokio::test]
async fn a_denied_file_has_its_name_size_and_hash_but_no_bytes_and_is_never_read() {
    let rig = Rig::new();
    rig.source.write("svc/app.yml", b"public");
    rig.source.write("svc/store.jks", b"PRIVATE-KEY-BYTES");
    let plain = rig.source.tree();
    let leaf = match plain.get("svc/store.jks") {
        Some(Entry::File(l)) => *l,
        other => panic!("{other:?}"),
    };
    let tree = plain
        .put("svc/store.jks", Entry::File(leaf.with_denied(true)))
        .tree;
    rig.send(None, &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    let denied = d
        .entries
        .iter()
        .find(|e| e.path.as_str() == "svc/store.jks")
        .unwrap();
    assert!(denied.denied);
    assert!(denied.bytes.is_none());
    assert_eq!(denied.hash.as_bytes(), &sha(b"PRIVATE-KEY-BYTES"));
    assert_eq!(denied.size, 17);
    assert_eq!(rig.source.reads(), 1, "only the public file was read");
    // And nothing on the wire holds the bytes.
    let wire = FromAgent::Delta(d.clone()).into_proto().encode_to_vec();
    assert!(!wire.windows(17).any(|w| w == b"PRIVATE-KEY-BYTES"));
}

#[tokio::test]
async fn delta_messages_at_most_3mib() {
    let rig = Rig::new();
    // Twenty files of 1.9 MiB: no two fit in one message.
    for i in 0..20_u8 {
        rig.source.write(&format!("big/f{i:02}.bin"), &vec![i; 1_900_000]);
    }
    let tree = rig.source.tree();
    rig.send(None, &tree).await.unwrap();
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 20);
    check_stream(&deltas, tree.root_hash());
    assert_eq!(deltas.iter().map(|d| d.entries.len()).sum::<usize>(), 20);
    assert!(deltas.iter().all(|d| d.payload_bytes() <= ScanDelta::MAX_BYTES));
    // Memory is one message, not the delta: files are read a message at a time, never ahead of the limit.
    assert_eq!(rig.source.largest_read_call(), 1);

    // Files that fit together are packed together, up to the byte limit.
    let rig = Rig::new();
    for i in 0..10 {
        rig.source.write(&format!("f{i}"), &vec![1_u8; 700_000]);
    }
    let tree = rig.source.tree();
    rig.send(None, &tree).await.unwrap();
    let deltas = rig.sink.take();
    assert_eq!(
        deltas.iter().map(|d| d.entries.len()).collect::<Vec<_>>(),
        [4, 4, 2]
    );
    check_stream(&deltas, tree.root_hash());
}

#[tokio::test]
async fn delta_messages_are_also_bounded_by_count() {
    let rig = Rig::new();
    for i in 0..10_500 {
        rig.source.write(&format!("many/f{i:05}"), b"x");
    }
    let tree = rig.source.tree();
    rig.send(None, &tree).await.unwrap();
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].entries.len(), ScanDelta::MAX_ENTRIES);
    check_stream(&deltas, tree.root_hash());

    // Removals are bounded the same way.
    let base = tree;
    rig.source.remove_all();
    let empty = rig.source.tree();
    rig.send(Some(&base), &empty).await.unwrap();
    let deltas = rig.sink.take();
    assert!(deltas.len() >= 2);
    assert_eq!(deltas.iter().map(|d| d.removed.len()).sum::<usize>(), 10_500);
    assert!(deltas.iter().all(|d| d.removed.len() <= ScanDelta::MAX_ENTRIES));
    check_stream(&deltas, empty.root_hash());
}

/// The messages of one logical delta: numbered from 0, `more` on all but the last, the same roots, rising seq, and each
/// small enough for the wire.
fn check_stream(deltas: &[ScanDelta], new_root: domain::ContentHash) {
    for (i, d) in deltas.iter().enumerate() {
        assert_eq!(d.part, u32::try_from(i).unwrap());
        assert_eq!(d.more, i + 1 < deltas.len(), "message {i}");
        assert_eq!(d.new_root, new_root);
        assert_eq!(d.base_root, deltas[0].base_root);
        assert!(
            d.payload_bytes() <= ScanDelta::MAX_BYTES,
            "message {i}: {} payload bytes",
            d.payload_bytes()
        );
        assert!(d.entries.len() <= ScanDelta::MAX_ENTRIES);
        assert!(
            encoded_len(d) <= 4 * MIB,
            "message {i} is {} bytes on the wire",
            encoded_len(d)
        );
        if i > 0 {
            assert!(d.seq > deltas[i - 1].seq, "seq rises");
        }
    }
}

#[tokio::test]
async fn a_file_changed_after_the_scan_is_sent_as_it_is_now_and_the_root_follows() {
    let rig = Rig::new();
    rig.source.write("a", b"scanned");
    rig.source.write("b", b"other");
    let base = rig.source.tree();
    rig.source.write("a", b"scanned and then edited");
    let scanned = rig.source.tree(); // what the walk saw
    rig.source.write("a", b"edited again, after the walk");

    let out = rig.send(Some(&base), &scanned).await.unwrap();
    let d = &rig.sink.deltas()[0];
    let e = d.entries.iter().find(|e| e.path.as_str() == "a").unwrap();
    assert_eq!(&e.bytes.as_ref().unwrap()[..], b"edited again, after the walk");
    assert_eq!(e.hash.as_bytes(), &sha(b"edited again, after the walk"));
    // `new_root` is the root of what is sent, not of the stale walk, and it is the tree the agent keeps.
    assert_ne!(d.new_root, scanned.root_hash());
    assert_eq!(d.new_root, out.tree.root_hash());
    assert_eq!(d.new_root, rig.source.tree().root_hash());
    assert_eq!(out.late_patches, 0);
    // The walk's tree and the corrected one are both worth remembering, for a hub that saw only one of the two.
    assert_eq!(out.trees.len(), 1);
}

#[tokio::test]
async fn a_file_that_changes_between_the_check_and_the_read_is_sent_true_to_its_bytes() {
    let rig = Rig::new();
    rig.source.write("a", b"as scanned");
    let tree = rig.source.tree();
    // The check sees no change, and then the read finds other bytes.
    rig.source.serve_stale("a", b"as read, a moment later");
    let out = rig.send(None, &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    let e = &d.entries[0];
    assert_eq!(&e.bytes.as_ref().unwrap()[..], b"as read, a moment later");
    assert_eq!(
        e.hash.as_bytes(),
        &sha(b"as read, a moment later"),
        "hash and bytes always agree"
    );
    assert_eq!(out.late_patches, 1);
    // The tree the agent keeps knows the truth; `new_root` in the message is the root it could compute before the send,
    // and the next heartbeat (a different root) makes the hub ask again, from a root that is still in the ring.
    assert_ne!(d.new_root, out.tree.root_hash());
    assert_eq!(out.trees.len(), 2);
    assert_eq!(out.trees[0].tree.root_hash(), d.new_root);
    assert_eq!(out.trees[1].tree.root_hash(), out.tree.root_hash());
}

#[tokio::test]
async fn a_file_gone_when_it_is_read_becomes_a_removal() {
    let rig = Rig::new();
    rig.source.write("a", b"1");
    rig.source.write("b", b"2");
    let tree = rig.source.tree();
    rig.source.fail_reads("a", ReadError::NotFound);
    rig.send(None, &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["b"]);
    assert_eq!(
        d.removed.iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
        ["a"]
    );
}

#[tokio::test]
async fn a_file_found_gone_by_the_check_is_a_removal_and_the_root_follows() {
    let rig = Rig::new();
    rig.source.write("a", b"1");
    rig.source.write("b", b"2");
    let base = MerkleTree::empty();
    let tree = rig.source.tree();
    rig.source.remove("a");
    let out = rig.send(Some(&base), &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["b"]);
    assert!(d.removed.is_empty(), "the hub never had it: nothing to remove");
    assert_eq!(d.new_root, rig.source.tree().root_hash());
    assert_eq!(out.tree.root_hash(), d.new_root);
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_reported_not_dropped_silently() {
    let rig = Rig::new();
    rig.source.write("a", b"1");
    rig.source.write("b", b"2");
    let tree = rig.source.tree();
    rig.source.fail_reads("a", ReadError::Io);
    rig.send(None, &tree).await.unwrap();
    let d = &rig.sink.deltas()[0];
    assert_eq!(paths(&d.entries), ["b"]);
    let skipped: Vec<_> = d
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(skipped, [("a", "unreadable")]);
}

#[tokio::test]
async fn an_empty_difference_still_sends_one_message_so_the_hub_learns_the_root() {
    let rig = Rig::new();
    rig.source.write("a", b"1");
    let tree = rig.source.tree();
    rig.send(Some(&tree), &tree).await.unwrap();
    let deltas = rig.sink.take();
    assert_eq!(deltas.len(), 1);
    assert!(deltas[0].entries.is_empty() && deltas[0].removed.is_empty() && deltas[0].skipped.is_empty());
    assert_eq!(deltas[0].base_root, Some(tree.root_hash()));
    assert_eq!(deltas[0].new_root, tree.root_hash());
    assert!(!deltas[0].more);
}

#[tokio::test]
async fn a_refusing_sink_stops_the_delta() {
    let rig = Rig::new();
    for i in 0..6 {
        rig.source.write(&format!("big{i}"), &vec![1_u8; 2_000_000]);
    }
    let tree = rig.source.tree();
    rig.sink.fail_after(2);
    let err = rig.send(None, &tree).await.unwrap_err();
    assert!(matches!(err, DeltaError::Sink(_)), "{err:?}");
    // The two messages that went out say `more`, so the hub does not apply a half delta.
    let sent = rig.sink.deltas();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|d| d.more));
}

#[tokio::test]
async fn every_message_carries_the_sync_job() {
    let rig = Rig::new();
    for i in 0..4 {
        rig.source.write(&format!("f{i}"), &vec![1_u8; 1_800_000]);
    }
    let tree = rig.source.tree();
    let job = JobRef::new("csp-dataload-1", "uid-1").unwrap();
    rig.builder()
        .with_job(Some(&job))
        .send(None, &tree, &rig.sink)
        .await
        .unwrap();
    let deltas = rig.sink.take();
    assert!(deltas.len() > 1);
    assert!(deltas.iter().all(|d| d.during_job.as_ref() == Some(&job)));
}

// ------------------------------------------------------------------------------------------ the property

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// From any root in the ring, the delta to the current tree turns a mirror of that root into a mirror of the
    /// current tree: the hub that knows any of the last hour's roots can be brought up to date (D63).
    #[test]
    fn delta_from_any_root_in_last_hour(
        steps in prop::collection::vec(
            (0_usize..8, prop::option::of(prop::collection::vec(any::<u8>(), 0..30))),
            1..14,
        ),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        runtime.block_on(async {
            let rig = Rig::new();
            let names = ["a.yml", "b.yml", "d1/c.yml", "d1/d.yml", "d1/s/e.yml", "d2/f.yml", "d2/s/g.yml", "h.yml"];
            let mut ring = agent::tree::RootRing::default();
            let mut history: Vec<(MerkleTree, BTreeMap<String, Vec<u8>>)> = vec![(MerkleTree::empty(), BTreeMap::new())];
            for (which, content) in steps {
                match content {
                    Some(bytes) => rig.source.write(names[which], &bytes),
                    None => rig.source.remove(names[which]),
                }
                let tree = rig.source.tree();
                ring.push(tree.clone(), tree.retained_bytes(), tokio::time::Instant::now());
                history.push((tree, rig.source.contents()));
            }
            let (current, current_content) = history.last().cloned().unwrap();
            for (old_tree, old_content) in &history {
                let Some(known) = ring.get(&old_tree.root_hash()).cloned().or_else(|| (old_tree.file_count() == 0).then(MerkleTree::empty)) else { continue };
                let out = rig.send(Some(&known), &current).await.unwrap();
                let deltas = rig.sink.take();
                prop_assert_eq!(out.tree.root_hash(), current.root_hash());
                let mut mirror = old_content.clone();
                CollectSink::apply_to(&mut mirror, &deltas);
                prop_assert_eq!(&mirror, &current_content);
                prop_assert!(deltas.iter().all(|d| d.base_root == Some(known.root_hash()) && d.new_root == current.root_hash()));
            }
            Ok(())
        })?;
    }
}
