//! T4 (D65, P7): `effective_tree_hash`, a hash for every directory of a swimlane's effective files, so that a comparison
//! can skip every subtree whose hash is the same on both sides.
//!
//! The encoding is pinned: the hub may need to reproduce a hash, and a change to the bytes would make every stored hash
//! stale. A directory's hash is SHA-256 over its entries sorted by raw name bytes, each entry
//! `u32_le(name length) || name || kind || 32-byte hash`, with kind 0 for a file and 1 for a directory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use bytes::Bytes;
use domain::{ContentHash, FileClass, LogicalFile, MergeMode, NfsPath, TenantId};
use engine::{
    Layer, Layers, TreeHashError, TreeHashes, effective, effective_tree_hash, normalised_hash, swimlane_hash,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

fn logical(s: &str) -> LogicalFile {
    LogicalFile::parse(s).unwrap()
}

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse_prefix(s).unwrap()
}

fn h(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

fn tree(files: &[(&str, u8)]) -> TreeHashes {
    let input: Vec<(LogicalFile, ContentHash)> = files.iter().map(|(p, n)| (logical(p), h(*n))).collect();
    effective_tree_hash(&input).unwrap()
}

fn entry(name: &str, kind: u8, hash: &ContentHash) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.push(kind);
    out.extend_from_slice(hash.as_bytes());
    out
}

fn sha(parts: &[Vec<u8>]) -> ContentHash {
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p);
    }
    ContentHash::from_bytes(hasher.finalize().into())
}

#[test]
fn encoding_matches_the_description_computed_independently() {
    let t = tree(&[("a/b.yml", 1), ("a/c.yml", 2), ("top.yml", 3)]);
    let a = sha(&[entry("b.yml", 0, &h(1)), entry("c.yml", 0, &h(2))]);
    let root = sha(&[entry("a", 1, &a), entry("top.yml", 0, &h(3))]);
    assert_eq!(t.get(&nfs("a")), Some(a));
    assert_eq!(t.get(&nfs("")), Some(root));
    assert_eq!(t.root(), Some(root));
}

#[test]
fn encoding_golden() {
    let t = tree(&[
        ("tx-infinity-api/tx-infinity-core.yml", 1),
        ("tx-infinity-api/entryGroupConfig.yml", 2),
        ("tx-infinity-api/channels/remote.yml", 3),
        ("document-service/resources/receipt-ci_1.bmp", 4),
        ("application.yml", 5),
        // Names that sort differently as paths and as names: `a` is a directory, `a-b.yml` and `a.yml` are files.
        ("a/x.yml", 6),
        ("a-b.yml", 7),
        ("a.yml", 8),
    ]);
    let mut out = String::new();
    for (dir, hash) in t.iter() {
        writeln!(
            out,
            "{:<40} {hash}",
            if dir.is_root() { "(root)" } else { dir.as_str() }
        )
        .unwrap();
    }
    insta::assert_snapshot!("tree_hash_encoding", out);
}

#[test]
fn names_sort_by_raw_bytes_inside_their_directory_not_as_paths() {
    // As whole paths `a-b.yml` < `a.yml` < `a/x.yml` ('-' and '.' sort before '/'), but inside the root the entries are
    // `a` < `a-b.yml` < `a.yml`.
    let t = tree(&[("a/x.yml", 1), ("a-b.yml", 2), ("a.yml", 3)]);
    let a = sha(&[entry("x.yml", 0, &h(1))]);
    let expected = sha(&[
        entry("a", 1, &a),
        entry("a-b.yml", 0, &h(2)),
        entry("a.yml", 0, &h(3)),
    ]);
    assert_eq!(t.root(), Some(expected));
}

#[test]
fn an_empty_swimlane_has_no_tree() {
    let t = effective_tree_hash(&[]).unwrap();
    assert_eq!(t.root(), None);
    assert_eq!(t.iter().count(), 0);
    assert_eq!(t.get(&nfs("a")), None);
}

#[test]
fn input_that_is_not_a_tree_is_refused() {
    let twice = [(logical("a/b.yml"), h(1)), (logical("a/b.yml"), h(1))];
    assert_eq!(
        effective_tree_hash(&twice).unwrap_err(),
        TreeHashError::Inconsistent
    );
    let clash = [(logical("a"), h(1)), (logical("a/b.yml"), h(2))];
    assert_eq!(
        effective_tree_hash(&clash).unwrap_err(),
        TreeHashError::Inconsistent
    );
    let clash_reversed = [(logical("a/b.yml"), h(2)), (logical("a"), h(1))];
    assert_eq!(
        effective_tree_hash(&clash_reversed).unwrap_err(),
        TreeHashError::Inconsistent
    );
}

#[test]
fn a_change_reaches_the_ancestors_and_no_other_directory() {
    let before = tree(&[("a/b/c.yml", 1), ("a/d.yml", 2), ("e/f.yml", 3), ("g.yml", 4)]);
    let after = tree(&[("a/b/c.yml", 9), ("a/d.yml", 2), ("e/f.yml", 3), ("g.yml", 4)]);
    for (dir, changed) in [("", true), ("a", true), ("a/b", true), ("e", false)] {
        assert_eq!(
            before.get(&nfs(dir)) != after.get(&nfs(dir)),
            changed,
            "directory {dir:?}"
        );
    }
}

#[test]
fn the_same_content_under_another_name_or_in_another_directory_is_another_tree() {
    let base = tree(&[("a/x.yml", 1)]);
    assert_ne!(base.root(), tree(&[("a/y.yml", 1)]).root());
    assert_ne!(base.root(), tree(&[("b/x.yml", 1)]).root());
    assert_ne!(base.root(), tree(&[("x.yml", 1)]).root());
    // A file `a` and a directory `a` with the same child hash do not collide: the kind byte tells them apart.
    let as_dir = sha(&[entry("x.yml", 0, &h(1))]);
    let file_instead = tree(&[("a", 0)]);
    assert_ne!(file_instead.root(), tree(&[("a/x.yml", 1)]).root());
    assert_ne!(Some(as_dir), file_instead.root());
}

// --- the hash of a file's content ---

#[test]
fn line_endings_do_not_change_the_hash_of_structured_and_text_files() {
    let lf = Bytes::from_static(b"a: 1\nb: 2\n");
    let crlf = Bytes::from_static(b"a: 1\r\nb: 2\r\n");
    for class in [FileClass::Structured, FileClass::Text] {
        assert_eq!(
            normalised_hash(&lf, class),
            normalised_hash(&crlf, class),
            "{class:?}"
        );
    }
    // A binary file is compared as it is: a CR in an image is data.
    assert_ne!(
        normalised_hash(&lf, FileClass::Binary),
        normalised_hash(&crlf, FileClass::Binary)
    );
    // And the hash is SHA-256 of the normalised bytes.
    assert_eq!(
        normalised_hash(&crlf, FileClass::Structured),
        ContentHash::from_bytes(Sha256::digest(lf.as_ref()).into())
    );
    // A different content is a different hash.
    assert_ne!(
        normalised_hash(&lf, FileClass::Structured),
        normalised_hash(&Bytes::from_static(b"a: 1\nb: 3\n"), FileClass::Structured)
    );
}

// --- a swimlane with several tenants (Q34) ---

fn t(s: &str) -> TenantId {
    TenantId::parse(s).unwrap()
}

#[test]
fn swimlane_hash_is_the_common_hash_when_every_tenant_agrees() {
    assert_eq!(swimlane_hash(&[]), None);
    assert_eq!(swimlane_hash(&[(t("sit1"), h(1))]), Some(h(1)));
    assert_eq!(
        swimlane_hash(&[(t("sit1"), h(1)), (t("sit2"), h(1)), (t("sit3"), h(1))]),
        Some(h(1))
    );
}

#[test]
fn swimlane_hash_covers_the_sorted_tenant_hash_pairs_when_they_differ() {
    let pairs = [(t("sit2"), h(2)), (t("sit1"), h(1))];
    let reversed = [(t("sit1"), h(1)), (t("sit2"), h(2))];
    let expected = sha(&[entry("sit1", 0, &h(1)), entry("sit2", 0, &h(2))]);
    assert_eq!(swimlane_hash(&pairs), Some(expected));
    assert_eq!(swimlane_hash(&pairs), swimlane_hash(&reversed));
    // Swapping which tenant has which hash is a different swimlane, even though the set of hashes is the same.
    assert_ne!(
        swimlane_hash(&pairs),
        swimlane_hash(&[(t("sit1"), h(2)), (t("sit2"), h(1))])
    );
    // And it never equals one tenant's own hash.
    assert_ne!(swimlane_hash(&pairs), Some(h(1)));
    assert_ne!(swimlane_hash(&pairs), Some(h(2)));
}

// --- properties ---

fn name() -> impl Strategy<Value = String> {
    // Includes '-' and '.' on purpose: they sort before '/'.
    "[a-c-][a-c.-]{0,2}"
}

fn path() -> impl Strategy<Value = String> {
    (proptest::collection::vec(name(), 0..3), name()).prop_map(|(dirs, file)| {
        let mut parts = dirs;
        parts.push(file);
        parts.join("/")
    })
}

fn files() -> impl Strategy<Value = Vec<(String, u8)>> {
    proptest::collection::btree_map(path(), 0_u8..4, 0..14).prop_map(|m| m.into_iter().collect::<Vec<_>>())
}

/// The input as a tree, or `None` when the paths clash (a file that is also a directory), which the generator can make.
fn build(files: &[(String, u8)]) -> Option<TreeHashes> {
    let input: Vec<(LogicalFile, ContentHash)> = files
        .iter()
        .map(|(p, n)| (LogicalFile::parse(p).unwrap(), h(*n)))
        .collect();
    effective_tree_hash(&input).ok()
}

proptest! {
    #[test]
    fn order_independent_prop(files in files(), seed in any::<u64>()) {
        let Some(sorted) = build(&files) else { return Ok(()) };
        let mut shuffled = files.clone();
        // A deterministic shuffle from the seed (no rng dependency): rotate and reverse by seed bits.
        if !shuffled.is_empty() {
            let len = shuffled.len();
            shuffled.rotate_left(usize::try_from(seed % len as u64).unwrap());
            if seed & 1 == 1 { shuffled.reverse(); }
        }
        prop_assert_eq!(build(&shuffled), Some(sorted));
    }

    /// The root changes exactly when a file's effective hash changes, was added or was removed; and the directories whose
    /// hash changes are exactly the ancestors of that file.
    #[test]
    fn tree_hash_changes_exactly_where_a_file_changes_prop(files in files(), pick in any::<usize>(), new_hash in 0_u8..6) {
        prop_assume!(!files.is_empty());
        let Some(before) = build(&files) else { return Ok(()) };
        let index = pick % files.len();
        let mut changed = files.clone();
        changed[index].1 = new_hash;
        let Some(after) = build(&changed) else { return Ok(()) };
        let same = files[index].1 == new_hash;
        prop_assert_eq!(before == after, same);
        if !same {
            // Which directories differ: those on the path to the file.
            let file = LogicalFile::parse(&files[index].0).unwrap();
            let mut ancestors = vec![NfsPath::root()];
            let mut dir = file.as_path().parent();
            let mut chain = Vec::new();
            while !dir.is_root() { chain.push(dir.clone()); dir = dir.parent(); }
            ancestors.extend(chain);
            for (dir, hash) in before.iter() {
                let differs = after.get(dir) != Some(*hash);
                prop_assert_eq!(differs, ancestors.contains(dir), "directory {:?}", dir);
            }
        }
    }

    #[test]
    fn adding_or_removing_a_file_changes_the_root_prop(files in files(), extra in path()) {
        let Some(before) = build(&files) else { return Ok(()) };
        let mut more = files.clone();
        if more.iter().any(|(p, _)| *p == extra) { return Ok(()); }
        more.push((extra, 7));
        let Some(after) = build(&more) else { return Ok(()) };
        prop_assert_ne!(before.root(), after.root());
    }
}

// --- the tree of effective files, as a swimlane builds it ---

/// One swimlane: base files and tenant files by logical file.
#[derive(Debug, Clone)]
struct Lane {
    base: BTreeMap<String, u8>,
    tenant: BTreeMap<String, u8>,
}

fn lane() -> impl Strategy<Value = Lane> {
    (
        proptest::collection::btree_map("[a-b]{1,2}/[a-c]{1,2}\\.yml", 0_u8..4, 1..8),
        proptest::collection::btree_map("[a-b]{1,2}/[a-c]{1,2}\\.yml", 0_u8..4, 0..8),
    )
        .prop_map(|(base, tenant)| Lane { base, tenant })
}

/// The effective tree of a lane in `whole_file` mode: for every logical file, the tenant file if there is one, otherwise
/// the base file.
fn effective_tree(lane: &Lane) -> TreeHashes {
    let logicals: BTreeSet<&String> = lane.base.keys().chain(lane.tenant.keys()).collect();
    let mut input = Vec::new();
    for l in logicals {
        let tenant_path = nfs(&l.replace(".yml", "-sit1.yml"));
        let base_path = nfs(l);
        let tenant = lane.tenant.get(l).map(|n| Layer {
            path: &tenant_path,
            hash: h(*n),
            settings: &[],
        });
        let base = lane.base.get(l).map(|n| Layer {
            path: &base_path,
            hash: h(*n),
            settings: &[],
        });
        if let Some(e) = effective(
            MergeMode::WholeFile,
            &Layers {
                tenant,
                base,
                root: None,
            },
        ) {
            input.push((logical(l), e.hash));
        }
    }
    effective_tree_hash(&input).unwrap()
}

proptest! {
    /// "Tree hashes change exactly when any effective file changes": editing a physical file changes the tree if the file
    /// is what is served, and does not when a tenant file shadows it.
    #[test]
    fn changes_exactly_when_an_effective_file_changes_prop(
        lane in lane(),
        edit_tenant in any::<bool>(),
        pick in any::<usize>(),
        new in 4_u8..9,
    ) {
        let before = effective_tree(&lane);
        let edit_tenant = edit_tenant && !lane.tenant.is_empty();
        let files = if edit_tenant { &lane.tenant } else { &lane.base };
        let key = files.keys().nth(pick % files.len()).unwrap().clone();
        let mut edited = lane.clone();
        if edit_tenant {
            edited.tenant.insert(key.clone(), new);
        } else {
            edited.base.insert(key.clone(), new);
        }
        let after = effective_tree(&edited);
        // The new hash is outside 0..4, so the file really changed. A tenant file is always what is served; a base file is
        // served only while no tenant file replaces it.
        let served = edit_tenant || !lane.tenant.contains_key(&key);
        prop_assert_eq!(before != after, served, "edited {} on the {} side", key, if edit_tenant { "tenant" } else { "base" });
    }
}
