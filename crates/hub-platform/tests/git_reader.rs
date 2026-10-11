//! T7: `GixReader`, the `GitReader` port over the mirrors. The shared conformance suite runs against a real repository made with
//! `git fast-import`, and the tests below pin what the suite leaves open: exact bytes, what a tree index holds, the read-through
//! fetch for a commit the mirror has not seen yet, and the shape of `diff_trees`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use domain::{BranchKind, CommitId, GitRef, PathChange, PathChangeKind, RepoKind, RepoPath};
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::{CacheResult, GitMetrics};
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::reader::{GixReader, ReaderSettings};
use ports::conformance::{GitExpectation, git_reader};
use ports::{GitError, GitReader, content_hash};
use support::fake_github::GitRemote;
use tempfile::TempDir;

struct Rig {
    _dir: TempDir,
    base: GitRemote,
    tenant: GitRemote,
    mirror: Arc<Mirror>,
    reader: Arc<GixReader>,
    metrics: GitMetrics,
}

async fn rig() -> Rig {
    let dir = TempDir::new().unwrap();
    let base = GitRemote::new();
    let tenant = GitRemote::new();
    base.commit("main", &[("config/seed.yml", Some(b"seed: 1\n"))], "seed");
    tenant.commit(
        "sit0",
        &[("data/config/seed-sit0.yml", Some(b"seed: 1\n"))],
        "seed",
    );
    let metrics = GitMetrics::default();
    let settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&base.url()),
        RemoteUrl::local(&tenant.url()),
    );
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), metrics.clone()).unwrap();
    mirror.initialize().await.unwrap();
    let reader =
        Arc::new(GixReader::new(Arc::clone(&mirror), &ReaderSettings::default(), metrics.clone()).unwrap());
    Rig {
        _dir: dir,
        base,
        tenant,
        mirror,
        reader,
        metrics,
    }
}

/// Run a blocking reader call off the async workers, as production code does.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

fn path(s: &str) -> RepoPath {
    RepoPath::parse(s).unwrap()
}

fn commit_id(s: &str) -> CommitId {
    CommitId::parse(s).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn gix_reader_conformance() {
    let rig = rig().await;
    // The shape `ports::conformance::git_reader` expects: three commits on one branch, a tag on the first, and a file that
    // three of them changed.
    let first = rig.tenant.commit(
        "sit1",
        &[
            ("app/core.yml", Some(b"a: 1\r\n")),
            ("app/other.yml", Some(b"b: 1\n")),
        ],
        "initial",
    );
    rig.tenant.annotated_tag("v1.0.0", &first);
    rig.tenant
        .commit("sit1", &[("app/core.yml", Some(b"a: 2\r\n"))], "change core");
    rig.tenant.commit(
        "sit1",
        &[("app/core.yml", Some(b"a: 3\r\n")), ("new.txt", Some(b"hi"))],
        "change core again",
    );
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();

    let expectation = GitExpectation {
        repo: RepoKind::Tenant,
        branch: "sit1".to_owned(),
        files: [
            ("app/core.yml", &b"a: 3\r\n"[..]),
            ("app/other.yml", b"b: 1\n"),
            ("new.txt", b"hi"),
        ]
        .into_iter()
        .map(|(p, b)| (path(p), Bytes::copy_from_slice(b)))
        .collect(),
        history_path: path("app/core.yml"),
        history_commits: 3,
        tag: Some((GitRef::parse("v1.0.0").unwrap(), 2)),
    };
    let reader = Arc::clone(&rig.reader);
    blocking(move || git_reader(&*reader, &expectation)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gix_reader_conformance_with_a_lightweight_tag() {
    let rig = rig().await;
    let first = rig.tenant.commit(
        "sit1",
        &[
            ("app/core.yml", Some(b"a: 1\r\n")),
            ("app/other.yml", Some(b"b: 1\n")),
        ],
        "initial",
    );
    rig.tenant.tag("v1.0.0", &first);
    rig.tenant
        .commit("sit1", &[("app/core.yml", Some(b"a: 2\r\n"))], "change core");
    rig.tenant.commit(
        "sit1",
        &[("app/core.yml", Some(b"a: 3\r\n")), ("new.txt", Some(b"hi"))],
        "again",
    );
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let expectation = GitExpectation {
        repo: RepoKind::Tenant,
        branch: "sit1".to_owned(),
        files: [
            ("app/core.yml", &b"a: 3\r\n"[..]),
            ("app/other.yml", b"b: 1\n"),
            ("new.txt", b"hi"),
        ]
        .into_iter()
        .map(|(p, b)| (path(p), Bytes::copy_from_slice(b)))
        .collect(),
        history_path: path("app/core.yml"),
        history_commits: 3,
        tag: Some((GitRef::parse("v1.0.0").unwrap(), 2)),
    };
    let reader = Arc::clone(&rig.reader);
    blocking(move || git_reader(&*reader, &expectation)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn read_returns_the_exact_bytes_of_crlf_bom_and_binary_files() {
    let rig = rig().await;
    let bom_crlf: &[u8] = b"\xef\xbb\xbfkey: value\r\nother: 2\r\n";
    let binary: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    let head = rig.base.commit(
        "main",
        &[
            ("config/crlf.yml", Some(bom_crlf)),
            ("config/image.bmp", Some(&binary)),
            ("config/empty.txt", Some(b"")),
        ],
        "files",
    );
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let head = commit_id(&head);
    blocking(move || {
        assert_eq!(
            reader
                .read(RepoKind::Base, &head, &path("config/crlf.yml"))
                .unwrap()
                .unwrap(),
            Bytes::from_static(bom_crlf)
        );
        assert_eq!(
            reader
                .read(RepoKind::Base, &head, &path("config/image.bmp"))
                .unwrap()
                .unwrap()
                .as_ref(),
            binary.as_slice()
        );
        assert_eq!(
            reader
                .read(RepoKind::Base, &head, &path("config/empty.txt"))
                .unwrap()
                .unwrap()
                .len(),
            0
        );
        // A directory is not a file.
        assert_eq!(reader.read(RepoKind::Base, &head, &path("config")).unwrap(), None);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_index_lists_regular_files_with_blob_ids_and_sha256_and_skips_symlinks() {
    let rig = rig().await;
    let head = rig.base.commit_modes(
        "main",
        &[
            ("100644", "config/a.yml", Some(b"a: 1\n")),
            ("100755", "config/run.sh", Some(b"#!/bin/sh\n")),
            ("100644", "config/deep/er/b.yml", Some(b"b: 1\r\n")),
            ("120000", "config/link.yml", Some(b"../../etc/passwd")),
        ],
        "mixed",
    );
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let head_id = commit_id(&head);
    let oid_of = |p: &str| {
        support::fake_github::run({
            let mut c = std::process::Command::new("git");
            c.arg("-C")
                .arg(&rig.base.path)
                .args(["rev-parse", &format!("{head}:{p}")]);
            c
        })
    };
    let a_oid = oid_of("config/a.yml");
    let index = blocking(move || reader.tree_index(RepoKind::Base, &head_id).unwrap()).await;
    let paths: Vec<_> = index
        .entries()
        .iter()
        .map(|e| e.path.as_str().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            "config/a.yml",
            "config/deep/er/b.yml",
            "config/run.sh",
            "config/seed.yml"
        ],
        "sorted, regular files only: the symlink is not listed"
    );
    let a = index.get(&path("config/a.yml")).unwrap();
    assert_eq!(a.git_oid.as_str(), a_oid, "the blob id is Git's own");
    assert_eq!(
        a.sha256,
        content_hash(b"a: 1\n"),
        "the hash is SHA-256 of the exact bytes"
    );
    assert_eq!(a.size, 5);
    assert_eq!(
        index.get(&path("config/deep/er/b.yml")).unwrap().sha256,
        content_hash(b"b: 1\r\n")
    );
    assert_eq!(index.commit, commit_id(&head));
    assert_eq!(index.repo, RepoKind::Base);
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_index_is_built_once_per_commit() {
    let rig = rig().await;
    let head = commit_id(&rig.base.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "a"));
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let (first, second) = blocking(move || {
        let first = reader.tree_index(RepoKind::Base, &head).unwrap();
        let second = reader.tree_index(RepoKind::Base, &head).unwrap();
        (first, second)
    })
    .await;
    assert!(
        Arc::ptr_eq(&first, &second),
        "the second read is the cached value"
    );
    assert_eq!(rig.metrics.tree_index_reads(CacheResult::Miss), 1);
    assert_eq!(rig.metrics.tree_index_reads(CacheResult::Hit), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_shared_by_many_commits_is_hashed_once() {
    let rig = rig().await;
    let big = vec![b'z'; 300_000];
    let c1 = rig.base.commit(
        "main",
        &[("config/big.xsl", Some(&big)), ("config/a.yml", Some(b"1\n"))],
        "one",
    );
    let c2 = rig.base.commit("main", &[("config/a.yml", Some(b"2\n"))], "two");
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let (i1, i2) = blocking(move || {
        (
            reader.tree_index(RepoKind::Base, &commit_id(&c1)).unwrap(),
            reader.tree_index(RepoKind::Base, &commit_id(&c2)).unwrap(),
        )
    })
    .await;
    let p = path("config/big.xsl");
    assert_eq!(i1.get(&p).unwrap(), i2.get(&p).unwrap());
    assert_eq!(i1.get(&p).unwrap().sha256, content_hash(&big));
    assert_eq!(
        rig.reader.blob_hashes_computed(),
        4,
        "seed.yml, big.xsl and a.yml for the first commit, then only the new a.yml: nothing is hashed twice"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_trees_reports_added_modified_and_deleted_files_sorted_by_path() {
    let rig = rig().await;
    let from = rig.tenant.commit(
        "sit1",
        &[
            ("data/config/keep.yml", Some(b"k: 1\n")),
            ("data/config/change.yml", Some(b"c: 1\n")),
            ("data/config/gone.yml", Some(b"g: 1\n")),
            ("data/config/dir/nested.yml", Some(b"n: 1\n")),
            ("data/config/dir/stay.yml", Some(b"s: 1\n")),
            ("data/config/swap", Some(b"was a file\n")),
        ],
        "from",
    );
    let to = rig.tenant.commit_modes(
        "sit1",
        &[
            ("100644", "data/config/change.yml", Some(b"c: 2\n")),
            ("100644", "data/config/gone.yml", None),
            ("100644", "data/config/new.yml", Some(b"w: 1\n")),
            ("100644", "data/config/dir/nested.yml", None),
            // A file becomes a directory.
            ("100644", "data/config/swap", None),
            ("100644", "data/config/swap/inside.yml", Some(b"i: 1\n")),
            // Only the mode changes: the content is the same, so it is not a change.
            ("100755", "data/config/keep.yml", Some(b"k: 1\n")),
        ],
        "to",
    );
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let (from, to) = (commit_id(&from), commit_id(&to));
    let reader = Arc::clone(&rig.reader);
    let (forward, back) = blocking({
        let (from, to) = (from.clone(), to.clone());
        move || {
            (
                reader.diff_trees(RepoKind::Tenant, &from, &to).unwrap(),
                reader.diff_trees(RepoKind::Tenant, &to, &from).unwrap(),
            )
        }
    })
    .await;
    let got: Vec<(String, PathChangeKind)> = forward
        .iter()
        .map(|c| (c.path.as_str().to_owned(), c.kind))
        .collect();
    assert_eq!(
        got,
        [
            ("data/config/change.yml".to_owned(), PathChangeKind::Modified),
            ("data/config/dir/nested.yml".to_owned(), PathChangeKind::Deleted),
            ("data/config/gone.yml".to_owned(), PathChangeKind::Deleted),
            ("data/config/new.yml".to_owned(), PathChangeKind::Added),
            ("data/config/swap".to_owned(), PathChangeKind::Deleted),
            ("data/config/swap/inside.yml".to_owned(), PathChangeKind::Added),
        ]
    );

    // The other direction mirrors it.
    assert_eq!(back.len(), 6);
    let find = |p: &str, kind: PathChangeKind| {
        back.iter()
            .any(|c: &PathChange| c.path.as_str() == p && c.kind == kind)
    };
    assert!(find("data/config/new.yml", PathChangeKind::Deleted));
    assert!(find("data/config/gone.yml", PathChangeKind::Added));
    assert!(find("data/config/change.yml", PathChangeKind::Modified));
    assert!(find("data/config/swap", PathChangeKind::Added));
    assert!(find("data/config/swap/inside.yml", PathChangeKind::Deleted));
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_trees_of_two_commits_in_different_branches_with_unchanged_subtrees_only_lists_the_difference() {
    let rig = rig().await;
    let mut files: Vec<(String, Vec<u8>)> = (0..200)
        .map(|i| {
            (
                format!("data/config/svc{}/file{i}.yml", i % 20),
                format!("v: {i}\n").into_bytes(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, Option<&[u8]>)> = files
        .iter()
        .map(|(p, b)| (p.as_str(), Some(b.as_slice())))
        .collect();
    let a = rig.tenant.commit("sit1", &borrowed, "base");
    rig.tenant.force_branch("sit2", &a);
    files[7].1 = b"v: changed\n".to_vec();
    let one = [(files[7].0.as_str(), Some(files[7].1.as_slice()))];
    let b = rig.tenant.commit("sit2", &one, "one file");
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let changes = blocking(move || {
        reader
            .diff_trees(RepoKind::Tenant, &commit_id(&a), &commit_id(&b))
            .unwrap()
    })
    .await;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path.as_str(), files[7].0);
    assert_eq!(changes[0].kind, PathChangeKind::Modified);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_pushed_after_the_last_fetch_is_found_by_one_read_through_fetch() {
    let rig = rig().await;
    let first = rig
        .base
        .commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "first");
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    // Pushed upstream, never fetched: the mirror does not have it.
    let second = commit_id(
        &rig.base
            .commit("main", &[("config/a.yml", Some(b"a: 2\n"))], "second"),
    );
    assert_ne!(first, second.as_str());

    let reader = Arc::clone(&rig.reader);
    let second_for_read = second.clone();
    let bytes = blocking(move || reader.read(RepoKind::Base, &second_for_read, &path("config/a.yml"))).await;
    assert_eq!(bytes.unwrap().unwrap(), Bytes::from_static(b"a: 2\n"));
    assert_eq!(rig.metrics.readthrough_total(RepoKind::Base), 1);

    // Known now: no further fetch.
    let reader = Arc::clone(&rig.reader);
    blocking(move || reader.tree_index(RepoKind::Base, &second).unwrap()).await;
    assert_eq!(rig.metrics.readthrough_total(RepoKind::Base), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_commit_is_not_found_after_one_fetch_and_not_fetched_for_again_soon() {
    let rig = rig().await;
    let unknown = commit_id(&"ab".repeat(20));
    let reader = Arc::clone(&rig.reader);
    let u = unknown.clone();
    let (first, second, third) = blocking(move || {
        (
            reader.read(RepoKind::Base, &u, &path("config/a.yml")),
            reader.tree_index(RepoKind::Base, &u).err(),
            reader.diff_trees(RepoKind::Base, &u, &u).err(),
        )
    })
    .await;
    assert_eq!(first, Err(GitError::NotFound));
    assert_eq!(second, Some(GitError::NotFound));
    assert_eq!(third, Some(GitError::NotFound));
    assert_eq!(
        rig.metrics.readthrough_total(RepoKind::Base),
        1,
        "a commit that one fetch did not bring is remembered as missing, so a flood of lookups costs one fetch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sha256_commit_id_is_not_found_in_a_sha1_mirror() {
    let rig = rig().await;
    let long = commit_id(&"cd".repeat(32));
    let reader = Arc::clone(&rig.reader);
    let result = blocking(move || reader.tree_index(RepoKind::Base, &long).err()).await;
    assert_eq!(result, Some(GitError::NotFound));
}

#[tokio::test(flavor = "multi_thread")]
async fn branches_are_classified_and_tags_are_listed() {
    let rig = rig().await;
    let c = rig
        .tenant
        .commit("sit1", &[("data/config/a-sit1.yml", Some(b"a\n"))], "sit1");
    rig.tenant
        .commit("templates/core", &[("data/config/c.yml", Some(b"c\n"))], "core");
    rig.tenant
        .commit("templates/common-docs", &[("docs/a.md", Some(b"# a\n"))], "docs");
    rig.tenant
        .commit("feature/x", &[("data/config/x.yml", Some(b"x\n"))], "x");
    rig.tenant.tag("release-1", &c);
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let (branches, tags, base) = blocking(move || {
        (
            reader.branches(RepoKind::Tenant).unwrap(),
            reader.tags(RepoKind::Tenant).unwrap(),
            reader.branches(RepoKind::Base).unwrap(),
        )
    })
    .await;
    let kind = |name: &str| branches.iter().find(|b| b.name.as_str() == name).map(|b| b.kind);
    assert_eq!(kind("sit1"), Some(BranchKind::Deployable));
    assert_eq!(kind("templates/core"), Some(BranchKind::Template));
    assert_eq!(kind("templates/common-docs"), Some(BranchKind::CommonDocs));
    assert_eq!(kind("feature/x"), Some(BranchKind::Other));
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name.as_str(), "release-1");
    assert_eq!(tags[0].commit.as_str(), c);
    assert!(base.iter().all(|b| b.kind == BranchKind::Default));
    assert!(
        branches.windows(2).all(|w| w[0].name < w[1].name),
        "listed in name order"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn history_lists_only_commits_that_changed_the_path_newest_first_with_clean_text() {
    let rig = rig().await;
    rig.base.commit(
        "main",
        &[("config/a.yml", Some(b"1\n")), ("config/b.yml", Some(b"1\n"))],
        "add both",
    );
    rig.base
        .commit("main", &[("config/b.yml", Some(b"2\n"))], "touch b only");
    rig.base.commit(
        "main",
        &[("config/a.yml", Some(b"2\n"))],
        "change a\nsecond line is not in the summary",
    );
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    let reader = Arc::clone(&rig.reader);
    let history = blocking(move || {
        reader
            .history(RepoKind::Base, "main", &path("config/a.yml"), 10)
            .unwrap()
    })
    .await;
    assert_eq!(history.len(), 2, "the commit that touched only b is not listed");
    assert_eq!(history[0].summary.as_str(), "change a");
    assert_eq!(history[1].summary.as_str(), "add both");
    assert_eq!(history[0].author.as_str(), "Test Author");
    assert!(history[0].time > history[1].time);
}

#[tokio::test(flavor = "multi_thread")]
async fn head_of_a_branch_with_an_unusable_name_is_invalid_ref_and_nothing_reaches_gix() {
    let rig = rig().await;
    let reader = Arc::clone(&rig.reader);
    blocking(move || {
        for bad in [
            "",
            "a b",
            "x..y",
            "-rf",
            "refs/../x",
            "x:y",
            "a\nb",
            &"a".repeat(300),
        ] {
            assert_eq!(
                reader.head(RepoKind::Base, bad),
                Err(GitError::InvalidRef),
                "{bad:?}"
            );
            assert_eq!(
                reader.history(RepoKind::Base, bad, &path("config/a.yml"), 3),
                Err(GitError::InvalidRef)
            );
        }
    })
    .await;
}
