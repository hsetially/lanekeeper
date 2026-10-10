//! File operations (T5, S11, S17, rule 11): byte-exact writes that carry the expected hash, reads, deletes, and what
//! happens at the edges of the root.
//!
//! Every test runs the real [`FileOps`] over a temporary directory through cap-std. The directory is read back with
//! `std::fs` here, in test code, to see what is really on disk.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use agent::fileops::{
    Crashed, DeniedReason, FileError, FileOps, LOCK_STRIPES, NoHooks, PathLocks, UnsupportedReason,
    WriteHooks,
};
use agent::root::NfsRoot;
use agent::tree::{FsSource, Pool, TreeSource, WalkConfig};
use bytes::Bytes;
use domain::{ContentHash, Expected, NfsPath};
use futures::FutureExt;
use support::recording_edits::{Edit, RecordingEdits};
use support::tree::sha;
use tempfile::TempDir;

const MAX: u64 = 2 * 1024 * 1024;

fn p(path: &str) -> NfsPath {
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

/// Every name in the tree that is one of the agent's temporary files.
fn temp_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".lanekeeper-tmp-") {
                out.push(name);
            }
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

struct Fixture<H: WriteHooks = NoHooks> {
    dir: TempDir,
    ops: FileOps<H>,
    edits: Arc<RecordingEdits>,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let edits = RecordingEdits::new();
    let ops = FileOps::new(NfsRoot::open(dir.path()).unwrap(), edits.clone());
    Fixture { dir, ops, edits }
}

fn fixture_with<H: WriteHooks>(hooks: H) -> Fixture<H> {
    let dir = TempDir::new().unwrap();
    let edits = RecordingEdits::new();
    let ops = FileOps::with_hooks(NfsRoot::open(dir.path()).unwrap(), edits.clone(), hooks);
    Fixture { dir, ops, edits }
}

// ------------------------------------------------------------------------------------------------ reading

#[tokio::test]
async fn read_returns_bytes_and_hash() {
    let f = fixture();
    let content = b"\xef\xbb\xbfa: 1\r\nb: 2\r\n";
    put(f.dir.path(), "svc/app.yml", content);
    let read = f.ops.read(&p("svc/app.yml"), MAX).await.unwrap();
    assert_eq!(&read.bytes[..], content);
    assert_eq!(read.hash, hash_of(content));
}

#[tokio::test]
async fn read_refuses_what_is_not_a_small_regular_file() {
    let f = fixture();
    put(f.dir.path(), "big.bin", &[7_u8; 101]);
    fs::create_dir(f.dir.path().join("d")).unwrap();
    assert_eq!(
        f.ops.read(&p("missing.yml"), MAX).await.unwrap_err(),
        FileError::NotFound
    );
    assert_eq!(
        f.ops.read(&p("big.bin"), 100).await.unwrap_err(),
        FileError::Unsupported(UnsupportedReason::TooLarge)
    );
    assert_eq!(
        f.ops.read(&p("d"), MAX).await.unwrap_err(),
        FileError::Unsupported(UnsupportedReason::NotRegular)
    );
    // Exactly at the limit is fine.
    assert!(f.ops.read(&p("big.bin"), 101).await.is_ok());
}

// ------------------------------------------------------------------------------------------------ writing

#[tokio::test]
async fn write_creates_a_file_that_was_expected_to_be_absent() {
    let f = fixture();
    let content = Bytes::from_static(b"new: true\n");
    let done = f
        .ops
        .write(&p("svc/new.yml"), Expected::Absent, content.clone(), MAX)
        .await;
    // `svc` does not exist, and writes never create directories (decision A12).
    assert_eq!(done.unwrap_err(), FileError::NotFound);
    fs::create_dir(f.dir.path().join("svc")).unwrap();
    let done = f
        .ops
        .write(&p("svc/new.yml"), Expected::Absent, content.clone(), MAX)
        .await
        .unwrap();
    assert_eq!(done.current_hash, Some(hash_of(&content)));
    assert_eq!(fs::read(f.dir.path().join("svc/new.yml")).unwrap(), &content[..]);
    assert_eq!(
        fs::metadata(f.dir.path().join("svc/new.yml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert!(temp_files(f.dir.path()).is_empty(), "no temporary file is left");
}

#[tokio::test]
async fn write_replaces_a_file_whose_hash_matches() {
    let f = fixture();
    put(f.dir.path(), "a.yml", b"old\n");
    let done = f
        .ops
        .write(
            &p("a.yml"),
            Expected::Hash {
                hash: hash_of(b"old\n"),
            },
            Bytes::from_static(b"new\n"),
            MAX,
        )
        .await
        .unwrap();
    assert_eq!(done.current_hash, Some(hash_of(b"new\n")));
    assert_eq!(fs::read(f.dir.path().join("a.yml")).unwrap(), b"new\n");
    assert!(temp_files(f.dir.path()).is_empty());
}

#[tokio::test]
async fn hash_mismatch_leaves_file_untouched() {
    let f = fixture();
    put(f.dir.path(), "a.yml", b"someone else changed this\n");
    let before = fs::metadata(f.dir.path().join("a.yml")).unwrap();
    let error = f
        .ops
        .write(
            &p("a.yml"),
            Expected::Hash {
                hash: hash_of(b"what the hub saw\n"),
            },
            Bytes::from_static(b"overwrite\n"),
            MAX,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        FileError::Conflict {
            current: Some(hash_of(b"someone else changed this\n"))
        }
    );
    assert_eq!(
        fs::read(f.dir.path().join("a.yml")).unwrap(),
        b"someone else changed this\n"
    );
    let after = fs::metadata(f.dir.path().join("a.yml")).unwrap();
    assert_eq!(
        (before.ino(), before.mtime(), before.mtime_nsec()),
        (after.ino(), after.mtime(), after.mtime_nsec())
    );
    assert!(
        temp_files(f.dir.path()).is_empty(),
        "a refused write leaves no temporary file"
    );
    assert!(f.edits.edits().is_empty(), "and tells the tree nothing");
}

#[tokio::test]
async fn expected_absent_conflicts_when_file_exists() {
    let f = fixture();
    put(f.dir.path(), "a.yml", b"already here\n");
    let error = f
        .ops
        .write(&p("a.yml"), Expected::Absent, Bytes::from_static(b"x"), MAX)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        FileError::Conflict {
            current: Some(hash_of(b"already here\n"))
        }
    );
    assert_eq!(fs::read(f.dir.path().join("a.yml")).unwrap(), b"already here\n");
}

#[tokio::test]
async fn expected_hash_conflicts_when_file_is_absent() {
    let f = fixture();
    let error = f
        .ops
        .write(
            &p("gone.yml"),
            Expected::Hash { hash: hash_of(b"x") },
            Bytes::from_static(b"y"),
            MAX,
        )
        .await
        .unwrap_err();
    assert_eq!(error, FileError::Conflict { current: None });
    assert!(!f.dir.path().join("gone.yml").exists());
}

/// A hook that runs a closure at the one moment between "the hash was checked" and "the file is replaced".
struct AtRename<F: Fn() + Send + Sync + 'static>(F);

impl<F: Fn() + Send + Sync + 'static> WriteHooks for AtRename<F> {
    fn before_rename(&self, _path: &NfsPath) -> Result<(), Crashed> {
        (self.0)();
        Ok(())
    }
}

#[tokio::test]
async fn a_create_never_overwrites_a_file_that_appears_while_it_is_written() {
    let dir = TempDir::new().unwrap();
    let racing = dir.path().join("race.yml");
    let edits = RecordingEdits::new();
    let ops = FileOps::with_hooks(
        NfsRoot::open(dir.path()).unwrap(),
        edits.clone(),
        AtRename(move || fs::write(&racing, b"appeared in the window\n").unwrap()),
    );
    let error = ops
        .write(
            &p("race.yml"),
            Expected::Absent,
            Bytes::from_static(b"mine\n"),
            MAX,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        FileError::Conflict {
            current: Some(hash_of(b"appeared in the window\n"))
        }
    );
    assert_eq!(
        fs::read(dir.path().join("race.yml")).unwrap(),
        b"appeared in the window\n"
    );
    assert!(temp_files(dir.path()).is_empty());
    assert!(edits.edits().is_empty());
}

#[tokio::test]
async fn crlf_and_bom_roundtrip_byte_exact() {
    let f = fixture();
    let content: &[u8] = b"\xef\xbb\xbfserver:\r\n  port: 8080\r\n  mixed: yes\n\r\ntrailing\r";
    f.ops
        .write(
            &p("crlf.yml"),
            Expected::Absent,
            Bytes::copy_from_slice(content),
            MAX,
        )
        .await
        .unwrap();
    assert_eq!(fs::read(f.dir.path().join("crlf.yml")).unwrap(), content);
    let read = f.ops.read(&p("crlf.yml"), MAX).await.unwrap();
    assert_eq!(&read.bytes[..], content);
    assert_eq!(read.hash, hash_of(content));
}

#[tokio::test]
async fn binary_roundtrip() {
    let f = fixture();
    let content: Vec<u8> = (0..=255_u8).cycle().take(100_000).collect();
    f.ops
        .write(
            &p("logo.bmp"),
            Expected::Absent,
            Bytes::from(content.clone()),
            MAX,
        )
        .await
        .unwrap();
    let read = f.ops.read(&p("logo.bmp"), MAX).await.unwrap();
    assert_eq!(&read.bytes[..], &content[..]);
    // And an empty file is a file.
    f.ops
        .write(&p("empty"), Expected::Absent, Bytes::new(), MAX)
        .await
        .unwrap();
    assert_eq!(fs::read(f.dir.path().join("empty")).unwrap(), b"");
}

/// A crash at the worst moment: the new bytes are in a temporary file, nothing has replaced the target.
struct CrashBeforeRename;

impl WriteHooks for CrashBeforeRename {
    fn before_rename(&self, _path: &NfsPath) -> Result<(), Crashed> {
        Err(Crashed)
    }
}

#[tokio::test]
async fn crash_before_rename_keeps_original() {
    let f = fixture_with(CrashBeforeRename);
    put(f.dir.path(), "svc/a.yml", b"original\r\n");
    let original = fs::metadata(f.dir.path().join("svc/a.yml")).unwrap();
    let error = f
        .ops
        .write(
            &p("svc/a.yml"),
            Expected::Hash {
                hash: hash_of(b"original\r\n"),
            },
            Bytes::from_static(b"never lands\n"),
            MAX,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, FileError::Io(_)), "{error:?}");
    let now = fs::metadata(f.dir.path().join("svc/a.yml")).unwrap();
    assert_eq!(fs::read(f.dir.path().join("svc/a.yml")).unwrap(), b"original\r\n");
    assert_eq!(original.ino(), now.ino());
    assert!(f.edits.edits().is_empty());

    // A killed process cleans nothing up, so the temporary file is still there ...
    let left = temp_files(f.dir.path());
    assert_eq!(left.len(), 1, "{left:?}");
    // ... and the scanner does not see it: the name is in the built-in ignore globs.
    let source = FsSource::new(
        NfsRoot::open(f.dir.path()).unwrap(),
        Pool::new(1).unwrap(),
        WalkConfig::default(),
    );
    let tree = source.scan(None, agent::tree::ScanMode::Full).await.unwrap().tree;
    let names: Vec<String> = tree.files().into_iter().map(|(path, _)| path).collect();
    assert_eq!(names, ["svc/a.yml"]);
}

#[tokio::test]
async fn write_preserves_mode() {
    let f = fixture();
    put(f.dir.path(), "secret.yml", b"old");
    fs::set_permissions(f.dir.path().join("secret.yml"), fs::Permissions::from_mode(0o640)).unwrap();
    f.ops
        .write(
            &p("secret.yml"),
            Expected::Hash {
                hash: hash_of(b"old"),
            },
            Bytes::from_static(b"new"),
            MAX,
        )
        .await
        .unwrap();
    let mode = fs::metadata(f.dir.path().join("secret.yml"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o640);
}

#[tokio::test]
async fn write_does_not_create_parent_directories() {
    let f = fixture();
    for path in ["newdir/a.yml", "a/b/c/d.yml"] {
        let error = f
            .ops
            .write(&p(path), Expected::Absent, Bytes::from_static(b"x"), MAX)
            .await
            .unwrap_err();
        assert_eq!(error, FileError::NotFound, "{path}");
    }
    assert_eq!(fs::read_dir(f.dir.path()).unwrap().count(), 0);
    // A file where a directory would have to be is the same answer.
    put(f.dir.path(), "file", b"x");
    let error = f
        .ops
        .write(
            &p("file/child.yml"),
            Expected::Absent,
            Bytes::from_static(b"x"),
            MAX,
        )
        .await
        .unwrap_err();
    assert_eq!(error, FileError::NotFound);
}

#[tokio::test]
async fn write_over_2mib_refused() {
    let f = fixture();
    let big = Bytes::from(vec![1_u8; 2 * 1024 * 1024 + 1]);
    let error = f
        .ops
        .write(&p("big.bin"), Expected::Absent, big, MAX)
        .await
        .unwrap_err();
    assert_eq!(error, FileError::Unsupported(UnsupportedReason::TooLarge));
    assert!(!f.dir.path().join("big.bin").exists());
    assert!(temp_files(f.dir.path()).is_empty());
    // Exactly 2 MiB is allowed, and the hub can lower the limit but this call cannot raise it.
    let exact = Bytes::from(vec![1_u8; 2 * 1024 * 1024]);
    f.ops
        .write(&p("exact.bin"), Expected::Absent, exact.clone(), MAX)
        .await
        .unwrap();
    let error = f
        .ops
        .write(&p("lower.bin"), Expected::Absent, exact, 1024)
        .await
        .unwrap_err();
    assert_eq!(error, FileError::Unsupported(UnsupportedReason::TooLarge));
    // A limit above the cap does not raise the cap: the 2 MiB is the agent's own, whatever the caller passes.
    let over = Bytes::from(vec![1_u8; 2 * 1024 * 1024 + 1]);
    let error = f
        .ops
        .write(&p("raised.bin"), Expected::Absent, over, u64::MAX)
        .await
        .unwrap_err();
    assert_eq!(error, FileError::Unsupported(UnsupportedReason::TooLarge));
    put(f.dir.path(), "huge.bin", &vec![2_u8; 2 * 1024 * 1024 + 1]);
    assert_eq!(
        f.ops.read(&p("huge.bin"), u64::MAX).await.unwrap_err(),
        FileError::Unsupported(UnsupportedReason::TooLarge)
    );
}

#[tokio::test]
async fn a_successful_write_tells_the_tree_what_is_on_disk() {
    let f = fixture();
    put(f.dir.path(), "a.yml", b"old");
    f.ops
        .write(
            &p("a.yml"),
            Expected::Hash {
                hash: hash_of(b"old"),
            },
            Bytes::from_static(b"brand new"),
            MAX,
        )
        .await
        .unwrap();
    let edits = f.edits.edits();
    assert_eq!(edits.len(), 1);
    let Edit::Written(path, leaf) = &edits[0] else {
        panic!("{edits:?}");
    };
    assert_eq!(path, "a.yml");
    let on_disk = fs::metadata(f.dir.path().join("a.yml")).unwrap();
    assert_eq!(leaf.hash, hash_of(b"brand new"));
    assert_eq!(leaf.stat.size, 9);
    // The stat is the one of the file that is there now, so the next walk finds it unchanged and reads nothing.
    assert_eq!(leaf.stat.ino, on_disk.ino());
    assert_eq!(
        leaf.stat.mtime.unix_millis(),
        on_disk.mtime() * 1000 + on_disk.mtime_nsec() / 1_000_000
    );
    assert!(!leaf.denied);
}

// ------------------------------------------------------------------------------------------------ deleting

#[tokio::test]
async fn delete_requires_matching_hash() {
    let f = fixture();
    put(f.dir.path(), "a.yml", b"keep me");
    let error = f.ops.delete(&p("a.yml"), hash_of(b"stale")).await.unwrap_err();
    assert_eq!(
        error,
        FileError::Conflict {
            current: Some(hash_of(b"keep me"))
        }
    );
    assert!(f.dir.path().join("a.yml").exists());
    assert!(f.edits.edits().is_empty());

    let done = f.ops.delete(&p("a.yml"), hash_of(b"keep me")).await.unwrap();
    assert_eq!(done.current_hash, None);
    assert!(!f.dir.path().join("a.yml").exists());
    assert_eq!(f.edits.edits(), [Edit::Removed("a.yml".to_owned())]);

    assert_eq!(
        f.ops.delete(&p("a.yml"), hash_of(b"keep me")).await.unwrap_err(),
        FileError::NotFound
    );
    assert_eq!(
        f.ops
            .delete(&p("no/such/dir.yml"), hash_of(b"x"))
            .await
            .unwrap_err(),
        FileError::NotFound
    );
}

#[tokio::test]
async fn delete_refuses_a_directory() {
    let f = fixture();
    fs::create_dir(f.dir.path().join("d")).unwrap();
    assert_eq!(
        f.ops.delete(&p("d"), hash_of(b"x")).await.unwrap_err(),
        FileError::Unsupported(UnsupportedReason::NotRegular)
    );
    assert!(f.dir.path().join("d").is_dir());
}

// ------------------------------------------------------------------------------------------------ symlinks and names

#[tokio::test]
async fn symlinked_file_refused() {
    let f = fixture();
    put(f.dir.path(), "real.yml", b"real");
    symlink("real.yml", f.dir.path().join("link.yml")).unwrap();
    let denied = FileError::Denied(DeniedReason::Symlink);
    assert_eq!(f.ops.read(&p("link.yml"), MAX).await.unwrap_err(), denied);
    assert_eq!(
        f.ops
            .write(
                &p("link.yml"),
                Expected::Hash {
                    hash: hash_of(b"real")
                },
                Bytes::from_static(b"x"),
                MAX
            )
            .await
            .unwrap_err(),
        denied
    );
    assert_eq!(
        f.ops
            .write(&p("link.yml"), Expected::Absent, Bytes::from_static(b"x"), MAX)
            .await
            .unwrap_err(),
        denied
    );
    assert_eq!(
        f.ops.delete(&p("link.yml"), hash_of(b"real")).await.unwrap_err(),
        denied
    );
    assert_eq!(fs::read(f.dir.path().join("real.yml")).unwrap(), b"real");
    assert!(
        fs::symlink_metadata(f.dir.path().join("link.yml"))
            .unwrap()
            .is_symlink()
    );
    // A dangling link is a link too.
    symlink("nowhere", f.dir.path().join("dangling")).unwrap();
    assert_eq!(f.ops.read(&p("dangling"), MAX).await.unwrap_err(), denied);
}

#[tokio::test]
async fn symlinked_dir_refused() {
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(outer.path().join("outside")).unwrap();
    put(outer.path(), "outside/secret.yml", b"outside secret");
    put(&root, "real/inside.yml", b"inside");
    symlink("real", root.join("alias")).unwrap();
    symlink("../outside", root.join("escape")).unwrap();
    let edits = RecordingEdits::new();
    let ops = FileOps::new(NfsRoot::open(&root).unwrap(), edits.clone());
    let denied = FileError::Denied(DeniedReason::Symlink);

    // A link to a directory inside the root is still a link: the agent does not follow any.
    assert_eq!(ops.read(&p("alias/inside.yml"), MAX).await.unwrap_err(), denied);
    assert_eq!(ops.read(&p("escape/secret.yml"), MAX).await.unwrap_err(), denied);
    assert_eq!(
        ops.write(
            &p("escape/planted.yml"),
            Expected::Absent,
            Bytes::from_static(b"x"),
            MAX
        )
        .await
        .unwrap_err(),
        denied
    );
    assert_eq!(
        ops.delete(&p("escape/secret.yml"), hash_of(b"outside secret"))
            .await
            .unwrap_err(),
        denied
    );
    assert_eq!(
        fs::read(outer.path().join("outside/secret.yml")).unwrap(),
        b"outside secret"
    );
    assert!(!outer.path().join("outside/planted.yml").exists());
    assert!(edits.edits().is_empty());
}

#[tokio::test]
async fn reserved_names_are_not_for_the_hub() {
    let f = fixture();
    for path in [".lanekeeper-tmp-0123", "svc/.lanekeeper-tmp-x", ".nfs0000beef"] {
        fs::create_dir_all(f.dir.path().join("svc")).unwrap();
        assert_eq!(
            f.ops
                .write(&p(path), Expected::Absent, Bytes::from_static(b"x"), MAX)
                .await
                .unwrap_err(),
            FileError::Denied(DeniedReason::ReservedName),
            "{path}"
        );
        assert_eq!(
            f.ops.read(&p(path), MAX).await.unwrap_err(),
            FileError::Denied(DeniedReason::ReservedName),
            "{path}"
        );
    }
    assert!(temp_files(f.dir.path()).is_empty());
}

/// Paths a hostile hub might send. Whatever the parser accepts, the operations must never touch anything outside the
/// root, and the sentinel next to the root must come out of every one exactly as it went in.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn traversal_corpus_never_escapes_root() {
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("root");
    fs::create_dir(&root).unwrap();
    put(outer.path(), "sentinel.txt", b"sentinel");
    put(&root, "a/inside.yml", b"inside");
    symlink("../sentinel.txt", root.join("link")).unwrap();
    symlink("..", root.join("up")).unwrap();
    symlink("/etc", root.join("etc")).unwrap();
    symlink(outer.path(), root.join("abs-outer")).unwrap();
    let edits = RecordingEdits::new();
    let ops = FileOps::new(NfsRoot::open(&root).unwrap(), edits.clone());

    let corpus = [
        "../sentinel.txt",
        "/sentinel.txt",
        "/etc/passwd",
        "a/../../sentinel.txt",
        "a/../../../etc/passwd",
        "./../sentinel.txt",
        "..",
        ".",
        "up/sentinel.txt",
        "up/root/a/inside.yml",
        "link",
        "link/x",
        "etc/passwd",
        "etc/hostname",
        "abs-outer/sentinel.txt",
        "..\\sentinel.txt",
        "a\\..\\..\\sentinel.txt",
        "a\0/../../sentinel.txt",
        "sentinel.txt\0",
        "%2e%2e/sentinel.txt",
        "..%2fsentinel.txt",
        "....//sentinel.txt",
        "a//../../sentinel.txt",
        "a/./../../sentinel.txt",
        "a/inside.yml/../../../sentinel.txt",
        " ../sentinel.txt",
        "\u{202e}txt.tnileS/..",
        "C:\\Windows\\win.ini",
        "//server/share/x",
        "root/../sentinel.txt",
    ];
    let mut parsed = 0;
    let mut succeeded = Vec::new();
    for raw in corpus {
        let Ok(path) = NfsPath::parse(raw) else { continue };
        parsed += 1;
        let results = [
            ops.read(&path, MAX).await.map(|_| ()),
            ops.write(&path, Expected::Absent, Bytes::from_static(b"planted"), MAX)
                .await
                .map(|_| ()),
            ops.write(
                &path,
                Expected::Hash {
                    hash: hash_of(b"sentinel"),
                },
                Bytes::from_static(b"planted"),
                MAX,
            )
            .await
            .map(|_| ()),
            ops.delete(&path, hash_of(b"sentinel")).await.map(|_| ()),
        ];
        for result in results {
            // A literal odd name such as `..%2fsentinel.txt` is a legal file name in the root, so an operation on it may
            // succeed. Where it does, the file must really be inside the root.
            if result.is_ok() {
                succeeded.push(raw);
            }
        }
    }
    // The corpus is not trivially refused at the parser: some of it gets as far as the file operations.
    assert!(
        parsed >= 8,
        "only {parsed} of {} reached the operations",
        corpus.len()
    );
    let canonical_root = fs::canonicalize(&root).unwrap();
    for raw in &succeeded {
        let on_disk = root.join(raw);
        let md = fs::symlink_metadata(&on_disk);
        // A success is a plain file that the operation created in the root (a write), or removed from it (a delete).
        if let Ok(md) = md {
            assert!(
                md.is_file(),
                "{raw:?} succeeded on something that is not a plain file"
            );
            assert!(
                fs::canonicalize(&on_disk).unwrap().starts_with(&canonical_root),
                "{raw:?}"
            );
        }
        assert!(
            !raw.contains("sentinel") || raw.starts_with("..%2f") || raw.starts_with("%2e"),
            "{raw:?} touched the sentinel"
        );
    }
    assert_eq!(fs::read(outer.path().join("sentinel.txt")).unwrap(), b"sentinel");
    let mut outer_names: Vec<String> = fs::read_dir(outer.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    outer_names.sort();
    assert_eq!(outer_names, ["root", "sentinel.txt"]);
    assert_eq!(fs::read(root.join("a/inside.yml")).unwrap(), b"inside");
    // The tree is told only about what really changed, and only inside the root.
    for edit in edits.edits() {
        let (Edit::Written(path, _) | Edit::Removed(path)) = &edit;
        assert!(succeeded.contains(&path.as_str()), "{edit:?}");
    }
    assert!(temp_files(outer.path()).is_empty());
}

// ------------------------------------------------------------------------------------------------ concurrency

#[test]
fn lock_stripes_are_bounded() {
    let locks = PathLocks::new();
    assert_eq!(locks.stripes(), LOCK_STRIPES);
    let mut used = std::collections::BTreeSet::new();
    for i in 0..10_000 {
        used.insert(locks.stripe_of(&p(&format!("dir-{}/file-{i}.yml", i % 97))));
    }
    assert!(used.len() <= LOCK_STRIPES, "{} stripes in use", used.len());
    assert!(
        used.len() > LOCK_STRIPES / 2,
        "the stripes are used: {}",
        used.len()
    );
    assert!(used.iter().all(|s| *s < LOCK_STRIPES));
    // The same path always takes the same stripe.
    assert_eq!(locks.stripe_of(&p("a/b.yml")), locks.stripe_of(&p("a/b.yml")));
}

#[tokio::test]
async fn same_path_operations_are_serialised() {
    let locks = PathLocks::new();
    let target = p("svc/app.yml");
    let other = (0..10_000)
        .map(|i| p(&format!("other-{i}.yml")))
        .find(|candidate| locks.stripe_of(candidate) != locks.stripe_of(&target))
        .unwrap();

    let first = locks.lock(&target).await;
    // A second operation on the same path waits ...
    assert!(locks.lock(&target).now_or_never().is_none());
    // ... while one on a path in another stripe does not.
    assert!(locks.lock(&other).now_or_never().is_some());
    drop(first);
    assert!(locks.lock(&target).now_or_never().is_some());
}

/// A hook that makes every rename slow, and notices if two writers are ever at the rename together.
struct SlowRename {
    inside: AtomicUsize,
    overlapped: AtomicBool,
}

struct SlowHook(Arc<SlowRename>);

impl WriteHooks for SlowHook {
    fn before_rename(&self, _path: &NfsPath) -> Result<(), Crashed> {
        let this = &self.0;
        if this.inside.fetch_add(1, Ordering::SeqCst) > 0 {
            this.overlapped.store(true, Ordering::SeqCst);
        }
        // Long enough that, without the lock, every writer is here at once.
        std::thread::sleep(Duration::from_millis(30));
        this.inside.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_writes_to_one_path_never_overlap() {
    let slow = Arc::new(SlowRename {
        inside: AtomicUsize::new(0),
        overlapped: AtomicBool::new(false),
    });
    let dir = TempDir::new().unwrap();
    put(dir.path(), "a.yml", b"v0");
    let ops = Arc::new(FileOps::with_hooks(
        NfsRoot::open(dir.path()).unwrap(),
        RecordingEdits::new(),
        SlowHook(slow.clone()),
    ));
    // Eight writers all expect to replace `v0`. Serialised, the first one wins and the other seven find `v1` there and
    // get a conflict. Unserialised, they would all pass the hash check before any of them renamed, and all "win".
    let writers: Vec<_> = (1..=8)
        .map(|i| {
            let ops = ops.clone();
            tokio::spawn(async move {
                ops.write(
                    &p("a.yml"),
                    Expected::Hash { hash: hash_of(b"v0") },
                    Bytes::from(format!("v{i}")),
                    MAX,
                )
                .await
            })
        })
        .collect();
    let mut won = 0;
    let mut conflicted = 0;
    for writer in writers {
        match writer.await.unwrap() {
            Ok(_) => won += 1,
            Err(FileError::Conflict { .. }) => conflicted += 1,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert_eq!((won, conflicted), (1, 7));
    assert!(
        !slow.overlapped.load(Ordering::SeqCst),
        "two writers were at the rename together"
    );
    assert!(temp_files(dir.path()).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_lock_wait_is_bounded() {
    let dir = TempDir::new().unwrap();
    let ops = FileOps::new(NfsRoot::open(dir.path()).unwrap(), RecordingEdits::new());
    let held = ops.locks().lock(&p("a.yml")).await;
    let target = p("a.yml");
    let waiting = ops.write(&target, Expected::Absent, Bytes::from_static(b"x"), MAX);
    let started = tokio::time::Instant::now();
    let error = waiting.await.unwrap_err();
    assert!(
        matches!(error, FileError::Io(std::io::ErrorKind::TimedOut)),
        "{error:?}"
    );
    assert!(started.elapsed() >= Duration::from_secs(30) && started.elapsed() < Duration::from_secs(40));
    drop(held);
    assert!(!dir.path().join("a.yml").exists());
}
