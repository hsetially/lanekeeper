//! The walker (T4, decision A1, S17): an own parallel walk over the cap-std root. What it tracks, what it leaves alone,
//! when it rehashes, and that it never leaves the root.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;

use agent::root::NfsRoot;
use agent::tree::walk::{ScanError, ScanMode, ScanOutcome, WalkConfig, walk};
use agent::tree::{Entry, MerkleTree, OtherReason, Pool, SkipReason};
use proptest::prelude::*;
use support::tree::{file, sha};
use tempfile::TempDir;

type Tweak = fn(&mut agent::tree::FileLeaf);

fn put(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn tree_of(files: &[(&str, &[u8])]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (path, content) in files {
        put(dir.path(), path, content);
    }
    dir
}

fn tree_of_owned(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (path, content) in files {
        put(dir.path(), &path, &content);
    }
    dir
}

fn scan_with(
    dir: &Path,
    previous: Option<&MerkleTree>,
    mode: ScanMode,
    cfg: &WalkConfig,
    threads: usize,
) -> Result<ScanOutcome, ScanError> {
    let root = NfsRoot::open(dir).unwrap_or_else(|e| panic!("{e}"));
    walk(&Pool::new(threads).unwrap(), root.dir(), previous, mode, cfg)
}

fn scan(dir: &Path, previous: Option<&MerkleTree>, mode: ScanMode) -> ScanOutcome {
    scan_with(dir, previous, mode, &WalkConfig::default(), 4).unwrap()
}

fn files_of(tree: &MerkleTree) -> Vec<String> {
    tree.files().into_iter().map(|(p, _)| p).collect()
}

#[test]
fn the_walk_builds_the_tree_of_the_directory() {
    let dir = tree_of(&[
        ("application.yml", b"a: 1\r\n"),
        ("svc/svc.yml", b"\xef\xbb\xbfb: 2\n"),
        ("svc/sub/deep.properties", b"c=3"),
        ("svc/sub/image.bmp", &[0, 1, 2, 3, 255]),
    ]);
    let out = scan(dir.path(), None, ScanMode::Full);
    let expected = MerkleTree::from_leaves([
        ("application.yml", file(b"a: 1\r\n")),
        ("svc/svc.yml", file(b"\xef\xbb\xbfb: 2\n")),
        ("svc/sub/deep.properties", file(b"c=3")),
        ("svc/sub/image.bmp", file(&[0, 1, 2, 3, 255])),
    ]);
    assert_eq!(out.tree.root_hash(), expected.root_hash());
    assert_eq!(out.tree.file_count(), 4);
    assert_eq!(out.stats.hashed, 4);
    assert!(out.new_bytes > 0);
}

#[test]
fn the_root_does_not_depend_on_how_many_threads_walked() {
    let dir = tree_of_owned((0..120).map(|i| {
        (
            format!("d{}/e{}/f{i}.yml", i % 7, i % 3),
            format!("content {i}").into_bytes(),
        )
    }));
    let one = scan_with(dir.path(), None, ScanMode::Full, &WalkConfig::default(), 1).unwrap();
    let eight = scan_with(dir.path(), None, ScanMode::Full, &WalkConfig::default(), 8).unwrap();
    assert_eq!(one.tree.root_hash(), eight.tree.root_hash());
}

#[test]
fn hidden_files_are_tracked() {
    let dir = tree_of(&[
        (".hidden", b"h"),
        (".gitignore", b"*"),
        (".dot/inner", b"i"),
        ("plain", b"p"),
    ]);
    let out = scan(dir.path(), None, ScanMode::Full);
    assert_eq!(
        files_of(&out.tree),
        [".dot/inner", ".gitignore", ".hidden", "plain"]
    );
}

#[test]
fn nfs_silly_files_are_ignored() {
    let dir = tree_of(&[
        ("keep.yml", b"k"),
        (".nfs0000000000a1b2c3", b"silly"),
        ("svc/.nfs1234", b"silly"),
        (".lanekeeper-tmp-9f", b"our own temp file"),
        ("svc/.lanekeeper-tmp-x/inner", b"in an ignored directory"),
    ]);
    let out = scan(dir.path(), None, ScanMode::Full);
    assert_eq!(files_of(&out.tree), ["keep.yml"]);
    // Ignored files are not even stat-ed into the tree: the root is the same as for the directory without them.
    assert_eq!(
        out.tree.root_hash(),
        MerkleTree::from_leaves([("keep.yml", file(b"k"))])
            .put_empty_dir("svc")
            .tree
            .root_hash()
    );
}

#[test]
fn configured_ignore_globs_match_names_and_paths() {
    let dir = tree_of(&[
        ("a.bak", b"1"),
        ("svc/b.bak", b"2"),
        ("logs/today.txt", b"3"),
        ("logs/deep/old.txt", b"4"),
        ("keep.yml", b"5"),
    ]);
    let cfg = WalkConfig::new(&["*.bak", "logs/**"], &[]).unwrap();
    let out = scan_with(dir.path(), None, ScanMode::Full, &cfg, 2).unwrap();
    assert_eq!(files_of(&out.tree), ["keep.yml"]);
}

#[test]
fn symlinks_are_not_followed_and_are_reported() {
    let outside = TempDir::new().unwrap();
    put(outside.path(), "secret.txt", b"outside the root");
    put(outside.path(), "dir/inner.txt", b"also outside");
    let dir = tree_of(&[("real/inside.yml", b"inside"), ("top.yml", b"top")]);
    symlink(
        outside.path().join("secret.txt"),
        dir.path().join("link-to-outside-file"),
    )
    .unwrap();
    symlink(outside.path().join("dir"), dir.path().join("link-to-outside-dir")).unwrap();
    symlink(dir.path().join("real"), dir.path().join("link-to-inside-dir")).unwrap();
    symlink("top.yml", dir.path().join("link-to-inside-file")).unwrap();
    symlink("does-not-exist", dir.path().join("dangling")).unwrap();

    let out = scan(dir.path(), None, ScanMode::Full);
    assert_eq!(files_of(&out.tree), ["real/inside.yml", "top.yml"]);
    assert_eq!(out.stats.hashed, 2, "nothing behind a link was opened");
    for name in [
        "link-to-outside-file",
        "link-to-outside-dir",
        "link-to-inside-dir",
        "link-to-inside-file",
        "dangling",
    ] {
        assert!(
            matches!(out.tree.get(name), Some(Entry::Other(OtherReason::Symlink))),
            "{name}: {:?}",
            out.tree.get(name)
        );
    }
    let skipped: Vec<_> = out
        .tree
        .diff(&MerkleTree::empty())
        .skipped
        .into_iter()
        .filter(|s| s.reason == SkipReason::Symlink)
        .collect();
    assert_eq!(skipped.len(), 5);
}

#[test]
fn special_files_are_other_not_files() {
    let dir = tree_of(&[("plain", b"p")]);
    // A named pipe: opening it for reading would block, so the walker must never try.
    let status = std::process::Command::new("mkfifo")
        .arg(dir.path().join("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    let out = scan(dir.path(), None, ScanMode::Full);
    assert_eq!(files_of(&out.tree), ["plain"]);
    assert!(matches!(
        out.tree.get("pipe"),
        Some(Entry::Other(OtherReason::Special))
    ));
}

#[test]
fn empty_directories_are_nodes() {
    let dir = tree_of(&[("a/f", b"x")]);
    let before = scan(dir.path(), None, ScanMode::Full).tree;
    fs::create_dir(dir.path().join("a/new")).unwrap();
    let after = scan(dir.path(), Some(&before), ScanMode::Stat).tree;
    assert_ne!(before.root_hash(), after.root_hash());
    assert!(matches!(after.get("a/new"), Some(Entry::Dir(d)) if d.entries().is_empty()));
}

#[test]
fn a_file_over_two_mib_is_hashed_by_streaming() {
    let big: Vec<u8> = (0..3 * 1024 * 1024 + 17_u32)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let dir = tree_of(&[("big.bin", &big), ("small", b"s")]);
    let out = scan(dir.path(), None, ScanMode::Full);
    let leaf = out
        .tree
        .files()
        .into_iter()
        .find(|(p, _)| p == "big.bin")
        .unwrap()
        .1;
    assert_eq!(leaf.hash.as_bytes(), &sha(&big));
    assert_eq!(leaf.stat.size, big.len() as u64);
    assert_eq!(out.stats.hashed_bytes, big.len() as u64 + 1);
}

#[test]
fn a_file_over_the_hash_limit_is_kind_other_and_never_read() {
    let dir = tree_of(&[("huge.bin", &vec![7_u8; 5000])]);
    let cfg = WalkConfig::default().with_hash_limit(4096);
    let out = scan_with(dir.path(), None, ScanMode::Full, &cfg, 2).unwrap();
    assert!(matches!(
        out.tree.get("huge.bin"),
        Some(Entry::Other(OtherReason::TooLarge))
    ));
    assert_eq!(out.stats.hashed_bytes, 0);
}

#[test]
fn bytes_are_never_normalised() {
    let crlf = b"line one\r\nline two\r\n";
    let bom = b"\xef\xbb\xbfkey: value\n";
    let lone_cr = b"a\rb\rc";
    let dir = tree_of(&[("crlf.yml", crlf), ("bom.yml", bom), ("cr.txt", lone_cr)]);
    let out = scan(dir.path(), None, ScanMode::Full);
    for (path, content) in [
        ("crlf.yml", &crlf[..]),
        ("bom.yml", &bom[..]),
        ("cr.txt", &lone_cr[..]),
    ] {
        let leaf = out.tree.files().into_iter().find(|(p, _)| p == path).unwrap().1;
        assert_eq!(
            leaf.hash.as_bytes(),
            &sha(content),
            "{path} was normalised before it was hashed"
        );
    }
}

#[test]
fn unrepresentable_names_are_reported_not_dropped() {
    let dir = tree_of(&[("ok/fine.yml", b"fine")]);
    let ok = dir.path().join("ok");
    fs::write(ok.join(std::ffi::OsStr::from_bytes(b"latin1-caf\xe9.yml")), b"1").unwrap();
    fs::write(ok.join("back\\slash.yml"), b"2").unwrap();
    fs::write(ok.join("new\nline.yml"), b"3").unwrap();
    // A path longer than an `NfsPath` may be, built from components that are each fine.
    let mut deep = dir.path().to_path_buf();
    for _ in 0..5 {
        deep.push("d".repeat(250));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("far.yml"), b"4").unwrap();

    let out = scan(dir.path(), None, ScanMode::Full);
    let unrepresentable = |name: &[u8]| {
        let Some(Entry::Dir(d)) = out.tree.get("ok") else {
            panic!("no ok/")
        };
        matches!(
            d.entries().get(name),
            Some(Entry::Other(OtherReason::Unrepresentable))
        )
    };
    assert!(unrepresentable(b"latin1-caf\xe9.yml"));
    assert!(unrepresentable(b"back\\slash.yml"));
    assert!(unrepresentable(b"new\nline.yml"));
    // The files with valid names are still tracked, and the long path is cut off where it stops being a path.
    let files = files_of(&out.tree);
    assert!(files.contains(&"ok/fine.yml".to_owned()));
    assert!(!files.iter().any(|f| f.ends_with("far.yml")), "{files:?}");
    // They are reported once, on the directory that holds them.
    let diff = out.tree.diff(&MerkleTree::empty());
    let on_ok: Vec<_> = diff
        .skipped
        .iter()
        .filter(|s| s.reason == SkipReason::UnrepresentableName && s.path == b"ok")
        .collect();
    assert_eq!(on_ok.len(), 1, "{:?}", diff.skipped);
    assert!(
        diff.skipped
            .iter()
            .any(|s| s.reason == SkipReason::UnrepresentableName && s.path.starts_with(b"dddd"))
    );
}

#[test]
fn denied_files_are_hashed_and_flagged() {
    let dir = tree_of(&[
        ("svc/keystore.jks", b"jks bytes"),
        ("svc/app.yml", b"y"),
        ("cert.pem", b"pem bytes"),
    ]);
    let cfg = WalkConfig::new(&[], &["*.jks", "*.pem"]).unwrap();
    let out = scan_with(dir.path(), None, ScanMode::Full, &cfg, 2).unwrap();
    let leaves: std::collections::BTreeMap<_, _> = out.tree.files().into_iter().collect();
    assert!(leaves["svc/keystore.jks"].denied && leaves["cert.pem"].denied);
    assert!(!leaves["svc/app.yml"].denied);
    assert_eq!(leaves["cert.pem"].hash.as_bytes(), &sha(b"pem bytes"));
}

#[test]
fn too_many_entries_stop_the_scan_instead_of_growing_without_bound() {
    let dir = tree_of_owned((0..50).map(|i| (format!("f{i}"), b"x".to_vec())));
    let cfg = WalkConfig::default().with_max_entries(20);
    assert!(matches!(
        scan_with(dir.path(), None, ScanMode::Full, &cfg, 2),
        Err(ScanError::TooManyEntries(20))
    ));
}

// ------------------------------------------------------------------------------------- when files are rehashed

#[test]
fn an_unchanged_tree_is_not_rehashed_and_shares_every_node() {
    let dir = tree_of(&[("a/x.yml", b"1"), ("a/y.yml", b"2"), ("b/z.yml", b"3")]);
    let first = scan(dir.path(), None, ScanMode::Full);
    let second = scan(dir.path(), Some(&first.tree), ScanMode::Stat);
    assert_eq!(second.stats.hashed, 0);
    assert_eq!(second.new_bytes, 0, "nothing changed, so nothing was allocated");
    assert!(std::sync::Arc::ptr_eq(first.tree.root(), second.tree.root()));
}

#[test]
fn stat_walk_rehashes_only_changed_files() {
    let dir = tree_of(&[("a/x.yml", b"one"), ("a/y.yml", b"two"), ("b/z.yml", b"three")]);
    let first = scan(dir.path(), None, ScanMode::Full);

    // A real change: a new size.
    put(dir.path(), "a/x.yml", b"one, longer");
    let second = scan(dir.path(), Some(&first.tree), ScanMode::Stat);
    assert_eq!(second.stats.hashed, 1);
    let diff = second.tree.diff(&first.tree);
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].path, b"a/x.yml");
    assert_eq!(diff.changed[0].leaf.hash.as_bytes(), &sha(b"one, longer"));
    // Only the directories on the path were rebuilt.
    assert!(
        matches!(first.tree.get("b"), Some(Entry::Dir(b1)) if matches!(second.tree.get("b"), Some(Entry::Dir(b2)) if std::sync::Arc::ptr_eq(b1, b2)))
    );
}

/// Each of the four attributes, changed on its own, is enough to make the stat walk read the file. The tree is doctored
/// so that exactly one attribute differs from what the file system reports.
#[test]
fn each_stat_attribute_alone_triggers_a_rehash() {
    let dir = tree_of(&[("a/x.yml", b"one"), ("a/y.yml", b"two")]);
    let first = scan(dir.path(), None, ScanMode::Full);
    let leaf = |tree: &MerkleTree| tree.files().into_iter().find(|(p, _)| p == "a/x.yml").unwrap().1;
    let original = leaf(&first.tree);

    let tweaks: [(&str, Tweak); 4] = [
        ("size", |l| l.stat.size += 1),
        ("mtime", |l| l.stat.mtime.nanos ^= 1),
        ("ctime", |l| l.stat.ctime.nanos ^= 1),
        ("inode", |l| l.stat.ino += 1),
    ];
    for (name, tweak) in tweaks {
        let mut doctored = original;
        tweak(&mut doctored);
        let stale = first.tree.put("a/x.yml", Entry::File(doctored)).tree;
        let out = scan(dir.path(), Some(&stale), ScanMode::Stat);
        assert_eq!(
            out.stats.hashed, 1,
            "a changed {name} must make the walk read the file"
        );
        // The file did not change, so the answer is the original leaf: the walk found its own mistake.
        assert_eq!(leaf(&out.tree), original, "{name}");
    }
    // With nothing doctored, nothing is read.
    assert_eq!(
        scan(dir.path(), Some(&first.tree), ScanMode::Stat).stats.hashed,
        0
    );
}

/// NFS attribute caching can show the old size and times for a file that changed. Nothing in a stat walk can see
/// that; the 15-minute full rehash can.
#[test]
fn full_rehash_finds_a_change_with_restored_mtime() {
    let dir = tree_of(&[("a/x.yml", b"aaaa"), ("b.yml", b"keep")]);
    let first = scan(dir.path(), None, ScanMode::Full);
    let old_leaf = first
        .tree
        .files()
        .into_iter()
        .find(|(p, _)| p == "a/x.yml")
        .unwrap()
        .1;

    // Same size, different bytes.
    put(dir.path(), "a/x.yml", b"bbbb");
    let truth = scan(dir.path(), None, ScanMode::Full);
    let new_leaf = truth
        .tree
        .files()
        .into_iter()
        .find(|(p, _)| p == "a/x.yml")
        .unwrap()
        .1;
    assert_ne!(old_leaf.hash, new_leaf.hash);

    // What a stale attribute cache shows: the file system's current stat, next to the old hash.
    let stale_leaf = agent::tree::FileLeaf::new(old_leaf.hash, new_leaf.stat);
    let stale = first.tree.put("a/x.yml", Entry::File(stale_leaf)).tree;

    let stat_walk = scan(dir.path(), Some(&stale), ScanMode::Stat);
    assert_eq!(stat_walk.stats.hashed, 0);
    assert_eq!(
        stat_walk.tree.root_hash(),
        stale.root_hash(),
        "the stat walk cannot see the change"
    );

    let full = scan(dir.path(), Some(&stale), ScanMode::Full);
    assert_eq!(full.stats.hashed, 2, "a full rehash reads every file");
    assert_eq!(full.tree.root_hash(), truth.tree.root_hash());
}

#[test]
fn removals_and_new_files_show_up_in_a_stat_walk() {
    let dir = tree_of(&[("a/x.yml", b"1"), ("a/y.yml", b"2"), ("gone/z.yml", b"3")]);
    let first = scan(dir.path(), None, ScanMode::Full);
    fs::remove_file(dir.path().join("a/x.yml")).unwrap();
    fs::remove_dir_all(dir.path().join("gone")).unwrap();
    put(dir.path(), "a/new.yml", b"n");
    let second = scan(dir.path(), Some(&first.tree), ScanMode::Stat);
    assert_eq!(files_of(&second.tree), ["a/new.yml", "a/y.yml"]);
    assert_eq!(second.stats.hashed, 1, "only the new file was read");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The walker agrees with the in-memory builder for any set of files.
    #[test]
    fn the_walk_of_any_directory_matches_the_tree_built_from_its_contents(
        files in prop::collection::btree_map(
            "(d[0-3]/){0,2}f[0-5]\\.yml",
            prop::collection::vec(any::<u8>(), 0..40),
            0..24,
        ),
    ) {
        let dir = TempDir::new().unwrap();
        // A name that is a directory in one path and a file in another cannot exist on disk.
        let mut written = Vec::new();
        for (path, content) in &files {
            let target = dir.path().join(path);
            if target.parent().is_some_and(|p| p.exists() && !p.is_dir()) || target.is_dir() { continue }
            if fs::create_dir_all(target.parent().unwrap()).is_err() || fs::write(&target, content).is_err() { continue }
            written.push((path.clone(), content.clone()));
        }
        let expected = MerkleTree::from_leaves(written.iter().map(|(p, c)| (p.as_str(), file(c))));
        let walked = scan(dir.path(), None, ScanMode::Full).tree;
        // Directories that were created and ended up empty are nodes in the walked tree too.
        let mut with_dirs = expected;
        for entry in walkdir_dirs(dir.path()) {
            if with_dirs.get(&entry).is_none() { with_dirs = with_dirs.put_empty_dir(&entry).tree }
        }
        prop_assert_eq!(walked.root_hash(), with_dirs.root_hash());
    }
}

/// Relative paths of every directory below `root`.
fn walkdir_dirs(root: &Path) -> Vec<String> {
    fn go(base: &Path, rel: &str, out: &mut Vec<String>) {
        for entry in fs::read_dir(base).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                let path = if rel.is_empty() {
                    entry.file_name().to_string_lossy().into_owned()
                } else {
                    format!("{rel}/{}", entry.file_name().to_string_lossy())
                };
                out.push(path.clone());
                go(&entry.path(), &path, out);
            }
        }
    }
    let mut out = Vec::new();
    go(root, "", &mut out);
    out
}
