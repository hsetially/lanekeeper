//! The Merkle tree (T4, D63): the root does not depend on the order things were found in, any single change moves it, the
//! byte encoding is pinned, and the diff between two trees is exactly the set of file changes (S11, P1).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;

use agent::tree::{ChangedFile, Entry, MerkleTree, OtherReason, SkipReason};
use domain::ContentHash;
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use support::tree::{file, leaf, sha, touched};

/// A second, independent statement of the encoding in `docs` / D89's neighbours: for each entry sorted by raw name
/// bytes, `u32_le(name length) || name || kind || 32-byte child hash`. The tree under test must agree with it.
fn reference_dir_hash(entries: &[(&[u8], u8, [u8; 32])]) -> [u8; 32] {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = Sha256::new();
    for (name, kind, child) in sorted {
        h.update(u32::try_from(name.len()).unwrap().to_le_bytes());
        h.update(name);
        h.update([kind]);
        h.update(child);
    }
    h.finalize().into()
}

#[test]
fn the_encoding_is_the_documented_one() {
    // a/x (file), a/y (file), b (file), c/ (empty directory), d (symlink)
    let tree = MerkleTree::from_leaves([
        ("a/x", file(b"one")),
        ("a/y", file(b"two")),
        ("b", file(b"three")),
        ("d", Entry::Other(OtherReason::Symlink)),
    ])
    .put_empty_dir("c")
    .tree;
    let a = reference_dir_hash(&[(b"x", 0, sha(b"one")), (b"y", 0, sha(b"two"))]);
    let c = reference_dir_hash(&[]);
    let root = reference_dir_hash(&[
        (b"a", 1, a),
        (b"b", 0, sha(b"three")),
        (b"c", 1, c),
        (b"d", 2, [0; 32]),
    ]);
    assert_eq!(tree.root_hash(), ContentHash::from_bytes(root));
    // The empty directory is a node, so it is part of the root: an empty directory changes the root (C11).
    assert_ne!(c, reference_dir_hash(&[(b"never", 0, [1; 32])]));
}

#[test]
fn merkle_encoding_golden() {
    let tree = MerkleTree::from_leaves([
        ("app/application.yml", file(b"server:\r\n  port: 8080\r\n")),
        ("app/app-sit1.yml", file(b"\xef\xbb\xbfkey: value\n")),
        ("app/resources/logo.bmp", file(&[0x42, 0x4d, 0, 1, 2, 3])),
        ("channels.yml", file(b"channels:\n  - atm\n")),
        ("link", Entry::Other(OtherReason::Symlink)),
    ])
    .put_empty_dir("empty")
    .tree;
    let mut lines = vec![format!("root {}", tree.root_hash())];
    for (path, hash) in tree.dir_hashes() {
        lines.push(format!("dir  {path:<14} {hash}"));
    }
    insta::assert_snapshot!("merkle_encoding", lines.join("\n"));
}

#[test]
fn an_empty_tree_has_the_hash_of_empty_input() {
    let empty: [u8; 32] = Sha256::digest(b"").into();
    assert_eq!(MerkleTree::empty().root_hash(), ContentHash::from_bytes(empty));
    assert_eq!(MerkleTree::empty().file_count(), 0);
}

#[test]
fn names_that_are_not_utf8_are_part_of_the_root_by_their_raw_bytes() {
    let put = |name: &[u8]| {
        MerkleTree::empty()
            .put_raw("", name, Entry::Other(OtherReason::Unrepresentable))
            .tree
    };
    let (a, b) = (put(b"caf\xe9"), put(b"caf\xe8"));
    assert_ne!(a.root_hash(), b.root_hash());
    let expected = reference_dir_hash(&[(b"caf\xe9", 2, [0; 32])]);
    assert_eq!(a.root_hash(), ContentHash::from_bytes(expected));
}

#[test]
fn file_count_counts_files_only() {
    let tree = MerkleTree::from_leaves([
        ("a/x", file(b"1")),
        ("a/b/y", file(b"2")),
        ("l", Entry::Other(OtherReason::Symlink)),
    ])
    .put_empty_dir("e")
    .tree;
    assert_eq!(tree.file_count(), 2);
}

// ------------------------------------------------------------------------------------------ properties

/// File paths that can never collide with a directory: files end in `.f`, directories never do.
fn tree_paths() -> impl Strategy<Value = BTreeMap<String, Vec<u8>>> {
    let component = prop::sample::select(vec!["a", "b", "c", "dd", "e e", "ünï"]);
    let dirs = prop::collection::vec(component, 0..4);
    let name = prop::sample::select(vec!["x.f", "y.f", "z z.f", "ü.f"]);
    prop::collection::btree_map(
        (dirs, name).prop_map(|(dirs, name)| {
            let mut p = dirs.join("/");
            if !p.is_empty() {
                p.push('/');
            }
            p.push_str(name);
            p
        }),
        prop::collection::vec(any::<u8>(), 0..24),
        0..24,
    )
}

fn build(files: &BTreeMap<String, Vec<u8>>, order: &[usize]) -> MerkleTree {
    let all: Vec<(&String, &Vec<u8>)> = files.iter().collect();
    MerkleTree::from_leaves(order.iter().map(|&i| (all[i].0.as_str(), file(all[i].1))))
}

proptest! {
    #[test]
    fn merkle_root_independent_of_walk_order(files in tree_paths(), seed in any::<u64>()) {
        let n = files.len();
        let forward: Vec<usize> = (0..n).collect();
        let mut shuffled = forward.clone();
        // A deterministic shuffle from the seed (Fisher-Yates over a splitmix64 stream).
        let mut state = seed;
        for i in (1..n).rev() {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            shuffled.swap(i, usize::try_from(z % (i as u64 + 1)).unwrap());
        }
        prop_assert_eq!(build(&files, &forward).root_hash(), build(&files, &shuffled).root_hash());
        // Building it by editing an empty tree one file at a time gives the same root as well.
        let mut edited = MerkleTree::empty();
        for &i in &shuffled {
            let (path, content) = files.iter().nth(i).unwrap();
            edited = edited.put(path, file(content)).tree;
        }
        prop_assert_eq!(build(&files, &forward).root_hash(), edited.root_hash());
    }

    #[test]
    fn any_single_file_change_changes_root(files in tree_paths(), pick in any::<prop::sample::Index>(), flip in any::<u8>()) {
        prop_assume!(!files.is_empty());
        let all: Vec<usize> = (0..files.len()).collect();
        let before = build(&files, &all);
        let (path, content) = files.iter().nth(pick.index(files.len())).unwrap();

        // Content changes.
        let mut changed = content.clone();
        if let Some(first) = changed.first_mut() { *first ^= flip | 1 } else { changed.push(flip) }
        let after = before.put(path, file(&changed)).tree;
        prop_assert_ne!(before.root_hash(), after.root_hash());

        // The file disappears.
        let removed = before.remove(path).tree;
        prop_assert_ne!(before.root_hash(), removed.root_hash());

        // The file is renamed (same bytes, other name).
        let renamed = removed.put(&format!("{path}.renamed"), file(content)).tree;
        prop_assert_ne!(before.root_hash(), renamed.root_hash());

        // Putting back the same bytes restores the root.
        prop_assert_eq!(before.root_hash(), removed.put(path, file(content)).tree.root_hash());
    }

    #[test]
    fn a_stat_only_change_does_not_move_the_root(files in tree_paths(), pick in any::<prop::sample::Index>()) {
        prop_assume!(!files.is_empty());
        let all: Vec<usize> = (0..files.len()).collect();
        let before = build(&files, &all);
        let (path, content) = files.iter().nth(pick.index(files.len())).unwrap();
        let touched = Entry::File(touched(leaf(content), 99_999));
        prop_assert_eq!(before.root_hash(), before.put(path, touched).tree.root_hash());
    }
}

// ------------------------------------------------------------------------------------------ the diff

fn changed_paths(changed: &[ChangedFile]) -> Vec<String> {
    changed
        .iter()
        .map(|c| String::from_utf8(c.path.clone()).unwrap())
        .collect()
}

fn removed_paths(removed: &[Vec<u8>]) -> Vec<String> {
    removed
        .iter()
        .map(|p| String::from_utf8(p.clone()).unwrap())
        .collect()
}

#[test]
fn diff_lists_exactly_the_changed_added_and_removed_files() {
    let old = MerkleTree::from_leaves([
        ("keep/a", file(b"a")),
        ("keep/b", file(b"b")),
        ("gone/x", file(b"x")),
        ("gone/deep/y", file(b"y")),
        ("top", file(b"t")),
    ]);
    let new = old
        .put("keep/b", file(b"B"))
        .tree
        .put("keep/new", file(b"n"))
        .tree
        .remove("gone/x")
        .tree
        .remove("gone/deep/y")
        .tree
        .remove("top")
        .tree;
    let diff = new.diff(&old);
    assert_eq!(changed_paths(&diff.changed), ["keep/b", "keep/new"]);
    assert_eq!(removed_paths(&diff.removed), ["gone/deep/y", "gone/x", "top"]);
    assert!(
        diff.skipped
            .iter()
            .all(|s| s.reason == SkipReason::EmptyDirectory)
    );
}

#[test]
fn a_removed_directory_removes_every_file_under_it() {
    let old = MerkleTree::from_leaves([("d/a", file(b"a")), ("d/s/b", file(b"b")), ("e", file(b"e"))]);
    let new = old.remove("d/a").tree.remove("d/s/b").tree;
    let diff = new.diff(&old);
    assert_eq!(removed_paths(&diff.removed), ["d/a", "d/s/b"]);
    assert!(diff.changed.is_empty());
}

#[test]
fn replacing_a_file_with_a_directory_removes_the_file_and_adds_the_files_under_it() {
    let old = MerkleTree::from_leaves([("x", file(b"file")), ("y", file(b"y"))]);
    let new = old.remove("x").tree.put("x/inner", file(b"in")).tree;
    let diff = new.diff(&old);
    assert_eq!(removed_paths(&diff.removed), ["x"]);
    assert_eq!(changed_paths(&diff.changed), ["x/inner"]);
}

#[test]
fn replacing_a_directory_with_a_file_removes_the_files_under_it() {
    let old = MerkleTree::from_leaves([("x/inner", file(b"in")), ("x/two", file(b"2"))]);
    let new = MerkleTree::from_leaves([("x", file(b"file"))]);
    let diff = new.diff(&old);
    assert_eq!(removed_paths(&diff.removed), ["x/inner", "x/two"]);
    assert_eq!(changed_paths(&diff.changed), ["x"]);
}

#[test]
fn a_stat_only_change_is_not_a_change() {
    let old = MerkleTree::from_leaves([("a", file(b"same"))]);
    let new = old.put("a", Entry::File(touched(leaf(b"same"), 5))).tree;
    let diff = new.diff(&old);
    assert!(diff.changed.is_empty() && diff.removed.is_empty() && diff.skipped.is_empty());
}

#[test]
fn symlinks_and_special_files_are_skips_not_changes() {
    let old = MerkleTree::from_leaves([("a", file(b"a"))]);
    let new = old
        .put("l", Entry::Other(OtherReason::Symlink))
        .tree
        .put("dev", Entry::Other(OtherReason::Special))
        .tree;
    let diff = new.diff(&old);
    assert!(diff.changed.is_empty() && diff.removed.is_empty());
    let skips: Vec<(String, &str)> = diff
        .skipped
        .iter()
        .map(|s| (String::from_utf8(s.path.clone()).unwrap(), s.reason.as_str()))
        .collect();
    assert_eq!(
        skips,
        [("dev".to_owned(), "special_file"), ("l".to_owned(), "symlink")]
    );
    // The same symlink on the next diff is not reported again.
    assert!(new.diff(&new).skipped.is_empty());
}

#[test]
fn a_file_replaced_by_a_symlink_is_removed_and_skipped() {
    let old = MerkleTree::from_leaves([("a", file(b"a"))]);
    let new = old.put("a", Entry::Other(OtherReason::Symlink)).tree;
    let diff = new.diff(&old);
    assert_eq!(removed_paths(&diff.removed), ["a"]);
    assert_eq!(diff.skipped.len(), 1);
}

#[test]
fn a_new_empty_directory_is_reported_and_changes_the_root() {
    let old = MerkleTree::from_leaves([("a", file(b"a"))]);
    let new = old.put_empty_dir("newdir").tree;
    assert_ne!(old.root_hash(), new.root_hash());
    let diff = new.diff(&old);
    assert!(diff.changed.is_empty() && diff.removed.is_empty());
    assert_eq!(diff.skipped.len(), 1);
    assert_eq!(diff.skipped[0].path, b"newdir");
    assert_eq!(diff.skipped[0].reason, SkipReason::EmptyDirectory);
}

#[test]
fn unrepresentable_names_are_reported_on_the_parent_directory_once() {
    let old = MerkleTree::from_leaves([("dir/ok", file(b"ok"))]);
    let new = old
        .put_raw("dir", b"caf\xe9", Entry::Other(OtherReason::Unrepresentable))
        .tree
        .put_raw("dir", b"caf\xe8", Entry::Other(OtherReason::Unrepresentable))
        .tree;
    let diff = new.diff(&old);
    assert_eq!(diff.skipped.len(), 1, "{:?}", diff.skipped);
    assert_eq!(diff.skipped[0].path, b"dir");
    assert_eq!(diff.skipped[0].reason, SkipReason::UnrepresentableName);
}

/// Apply a diff to a mirror of the old tree, as the hub does: files by path to their hash.
fn apply(mirror: &mut BTreeMap<String, ContentHash>, diff: &agent::tree::TreeDiff) {
    for path in &diff.removed {
        mirror.remove(std::str::from_utf8(path).unwrap());
    }
    for c in &diff.changed {
        mirror.insert(String::from_utf8(c.path.clone()).unwrap(), c.leaf.hash);
    }
}

fn mirror_of(tree: &MerkleTree) -> BTreeMap<String, ContentHash> {
    tree.files().into_iter().map(|(p, l)| (p, l.hash)).collect()
}

proptest! {
    /// The diff from any earlier tree, applied to a mirror of that tree, gives a mirror of the new one.
    #[test]
    fn diff_applied_to_the_old_mirror_gives_the_new_mirror(
        a in tree_paths(),
        b in tree_paths(),
    ) {
        let old = build(&a, &(0..a.len()).collect::<Vec<_>>());
        let new = build(&b, &(0..b.len()).collect::<Vec<_>>());
        let mut mirror = mirror_of(&old);
        apply(&mut mirror, &new.diff(&old));
        prop_assert_eq!(mirror, mirror_of(&new));
        // And a tree has no difference from itself.
        let same = new.diff(&new);
        prop_assert!(same.changed.is_empty() && same.removed.is_empty() && same.skipped.is_empty());
    }
}

#[test]
fn edits_share_what_they_do_not_touch() {
    let base = MerkleTree::from_leaves(
        (0..50)
            .flat_map(|d| (0..40).map(move |f| (format!("d{d}/f{f}"), file(format!("{d}-{f}").as_bytes())))),
    );
    let edited = base.put("d7/f3", file(b"changed"));
    // One directory and the root were rebuilt: the other 49 are the same nodes, so the edit costs a path, not the tree.
    assert!(edited.new_bytes > 0);
    assert!(
        edited.new_bytes * 10 < base.retained_bytes(),
        "{} vs {}",
        edited.new_bytes,
        base.retained_bytes()
    );
    let same_node = |name: &str| match (base.get(name), edited.tree.get(name)) {
        (Some(Entry::Dir(a)), Some(Entry::Dir(b))) => Arc::ptr_eq(a, b),
        other => panic!("{name} is not a directory in both trees: {other:?}"),
    };
    assert!(same_node("d8"));
    assert!(!same_node("d7"));
}
