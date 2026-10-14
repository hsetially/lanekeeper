//! The Git side of drift (T2, D65, P2): which swimlane paths a moved head changes, and the normalised hash of a Git file read once
//! per object. Over a real Postgres 16 (swimlanes, tenants, inventories) and the in-memory Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use domain::{CommitId, DomainEvent, FileClass, GitRef, RepoKind, RepoPath, SwimlaneId};
use engine::drift_hash;
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::{CacheResult, MoveResult};
use hub_registry::{Affected, GitIndex, GitIndexDeps, LaneImpact, RegistryError, RegistryMetrics};
use ports::GitError;
use ports::fakes::FakeGit;
use support::counting_git::CountingGit;
use support::rig::Rig;

struct Setup {
    rig: Rig,
    git: CountingGit,
    index: GitIndex,
    metrics: RegistryMetrics,
}

async fn setup(config: RegistryConfig) -> Option<Setup> {
    let rig = Rig::new().await?;
    let git = CountingGit::new(FakeGit::new());
    let metrics = RegistryMetrics::default();
    let index = GitIndex::new(GitIndexDeps {
        pool: rig.pool.clone(),
        git: Arc::new(git.clone()),
        config,
        metrics: metrics.clone(),
    })
    .unwrap();
    Some(Setup {
        rig,
        git,
        index,
        metrics,
    })
}

fn lane(id: &str) -> SwimlaneId {
    SwimlaneId::parse(id).unwrap()
}

fn repo_path(p: &str) -> RepoPath {
    RepoPath::parse(p).unwrap()
}

/// The paths of one swimlane, or `None` when the swimlane is not concerned.
fn paths(a: &Affected, id: &str) -> Option<Vec<String>> {
    match a.lanes.get(&lane(id))? {
        LaneImpact::Paths(set) => Some(set.iter().map(|p| p.as_str().to_owned()).collect()),
        LaneImpact::Whole => Some(vec!["<whole>".to_owned()]),
    }
}

fn names(a: &Affected) -> Vec<&str> {
    a.lanes.keys().map(SwimlaneId::as_str).collect()
}

async fn moved(s: &Setup, repo: RepoKind, branch: &str, from: Option<&CommitId>, to: &CommitId) -> Affected {
    s.index.affected(repo, branch, from, to).await.unwrap()
}

// ------------------------------------------------------------------------------------------------ a moved head

#[tokio::test]
async fn head_moved_maps_changed_repo_paths_to_swimlane_paths() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    s.rig.add_swimlane("sit2", &["sit2", "sit3"], true).await;
    s.rig.add_swimlane("fresh", &["sit1"], false).await;
    let fake = s.git.git();

    // The base repo: one change to a base file, one new base file, one deleted, and two that are not configuration.
    let b1 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[
                ("config/app/core.yml", Some(b"a: 1\n")),
                ("config/gone.yml", Some(b"g: 1\n")),
                ("README.md", Some(b"x")),
            ],
            "base 1",
        )
        .unwrap();
    let b2 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[
                ("config/app/core.yml", Some(b"a: 2\n")),
                ("config/new.yml", Some(b"n: 1\n")),
                ("config/gone.yml", None),
                ("README.md", Some(b"y")),
                (".github/ci.yml", Some(b"ci")),
            ],
            "base 2",
        )
        .unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&b1), &b2).await;
    assert_eq!(
        names(&a),
        ["sit1", "sit2"],
        "every swimlane with an inventory, and not the one without"
    );
    for id in ["sit1", "sit2"] {
        assert_eq!(
            paths(&a, id).unwrap(),
            ["app/core.yml", "gone.yml", "new.yml"],
            "a change, a deletion and an addition, below the base root"
        );
    }
    assert_eq!(
        (a.changed, a.unmapped),
        (5, 2),
        "the README and the workflow are not configuration"
    );
    // Every swimlane shares one set: it is not copied per swimlane.
    let (LaneImpact::Paths(x), LaneImpact::Paths(y)) = (&a.lanes[&lane("sit1")], &a.lanes[&lane("sit2")])
    else {
        panic!("paths");
    };
    assert!(Arc::ptr_eq(x, y));

    // A tenant branch: only the swimlanes that carry it, at the suffixed name.
    let t1 = fake
        .commit(
            RepoKind::Tenant,
            "sit1",
            &[
                ("data/config/app/core.yml", Some(b"a: 1\n")),
                ("data/config/app/logo.bmp", Some(b"bmp")),
            ],
            "t1",
        )
        .unwrap();
    let t2 = fake
        .commit(
            RepoKind::Tenant,
            "sit1",
            &[
                ("data/config/app/core.yml", Some(b"a: 9\n")),
                ("config/app/core.yml", Some(b"stray")),
            ],
            "t2",
        )
        .unwrap();
    let a = moved(&s, RepoKind::Tenant, "sit1", Some(&t1), &t2).await;
    assert_eq!(
        names(&a),
        ["sit1"],
        "sit2 does not carry the tenant sit1, and `fresh` has no inventory"
    );
    assert_eq!(paths(&a, "sit1").unwrap(), ["app/core-sit1.yml"]);
    assert_eq!(
        (a.changed, a.unmapped),
        (2, 1),
        "a base path on a tenant branch is not a tenant path"
    );

    // A tenant that two tenants of one swimlane share the swimlane: sit3 belongs to sit2's set.
    let t3 = fake
        .commit(
            RepoKind::Tenant,
            "sit3",
            &[("data/config/x/y.yml", Some(b"1"))],
            "t3",
        )
        .unwrap();
    let a = moved(&s, RepoKind::Tenant, "sit3", None, &t3).await;
    assert_eq!(names(&a), ["sit2"]);
    assert_eq!(
        paths(&a, "sit2").unwrap(),
        ["x/y-sit3.yml"],
        "a new branch (no `from`) is every file of its tree"
    );

    assert_eq!(s.metrics.head_move_count(MoveResult::Affected), 3);
}

#[tokio::test]
async fn a_branch_that_no_swimlane_deploys_concerns_nothing() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    let fake = s.git.git();
    let files: &[(&str, Option<&[u8]>)] = &[("data/config/a.yml", Some(b"1"))];
    let template = fake
        .commit(RepoKind::Tenant, "templates/core", files, "t")
        .unwrap();
    let docs = fake
        .commit(RepoKind::Tenant, "templates/common-docs", files, "d")
        .unwrap();
    let nobody = fake.commit(RepoKind::Tenant, "sit9", files, "n").unwrap();
    for (branch, to) in [
        ("templates/core", &template),
        ("templates/common-docs", &docs),
        ("sit9", &nobody),
    ] {
        let a = moved(&s, RepoKind::Tenant, branch, None, to).await;
        assert!(a.is_empty(), "{branch}: {a:?}");
    }
    // A branch the mirror does not have, and a name that is not a ref at all.
    let ghost = moved(&s, RepoKind::Tenant, "no-such-branch", None, &nobody).await;
    assert!(ghost.is_empty());
    let odd = moved(&s, RepoKind::Tenant, "has space", None, &nobody).await;
    assert!(odd.is_empty());
    assert_eq!(s.metrics.head_move_count(MoveResult::Unconcerned), 5);
    assert_eq!(s.git.diffs(), 0, "no tree was diffed for a branch nobody deploys");
}

#[tokio::test]
async fn a_move_that_changes_no_configuration_is_unconcerned() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    let fake = s.git.git();
    let c1 = fake
        .commit(RepoKind::Base, "master", &[("README.md", Some(b"1"))], "1")
        .unwrap();
    let c2 = fake
        .commit(RepoKind::Base, "master", &[("README.md", Some(b"2"))], "2")
        .unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&c1), &c2).await;
    assert!(a.is_empty());
    assert_eq!((a.changed, a.unmapped), (1, 1));
    let same = moved(&s, RepoKind::Base, "master", Some(&c2), &c2).await;
    assert!(same.is_empty());
    assert_eq!(s.metrics.head_move_count(MoveResult::Unconcerned), 2);
}

#[tokio::test]
async fn the_admins_roots_decide_what_is_configuration() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    for (repo, root) in [("base", "cfg"), ("tenant", "tenants/cfg")] {
        sqlx::query(
            "INSERT INTO path_mappings (repo, repo_root) VALUES ($1, $2) \
             ON CONFLICT (repo) DO UPDATE SET repo_root = EXCLUDED.repo_root",
        )
        .bind(repo)
        .bind(root)
        .execute(&s.rig.pool)
        .await
        .unwrap();
    }
    let fake = s.git.git();
    let c1 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[("cfg/a.yml", Some(b"1")), ("config/b.yml", Some(b"1"))],
            "1",
        )
        .unwrap();
    let c2 = fake
        .commit(
            RepoKind::Base,
            "master",
            &[("cfg/a.yml", Some(b"2")), ("config/b.yml", Some(b"2"))],
            "2",
        )
        .unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&c1), &c2).await;
    assert_eq!(
        paths(&a, "sit1").unwrap(),
        ["a.yml"],
        "the configured root, not the default one"
    );
    assert_eq!(a.unmapped, 1);
}

#[tokio::test]
async fn a_move_past_the_bound_or_over_a_lost_commit_recomputes_the_swimlane_whole() {
    let config = RegistryConfig {
        max_affected_paths: 3,
        ..RegistryConfig::default()
    };
    let Some(s) = setup(config).await else { return };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    s.rig.add_swimlane("sit2", &["sit2"], true).await;
    let fake = s.git.git();
    let c1 = fake
        .commit(RepoKind::Base, "master", &[("config/a.yml", Some(b"0"))], "1")
        .unwrap();
    let small = fake
        .commit(
            RepoKind::Base,
            "master",
            &[("config/a.yml", Some(b"1")), ("config/b.yml", Some(b"1"))],
            "2",
        )
        .unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&c1), &small).await;
    assert_eq!(
        paths(&a, "sit1").unwrap(),
        ["a.yml", "b.yml"],
        "under the bound: paths"
    );

    let many: Vec<(String, Option<Vec<u8>>)> = (0..5)
        .map(|i| (format!("config/f{i}.yml"), Some(vec![b'x', i])))
        .collect();
    let many_ref: Vec<(&str, Option<&[u8]>)> = many.iter().map(|(p, c)| (p.as_str(), c.as_deref())).collect();
    let big = fake.commit(RepoKind::Base, "master", &many_ref, "3").unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&small), &big).await;
    assert_eq!(a.lanes.get(&lane("sit1")), Some(&LaneImpact::Whole));
    assert_eq!(a.lanes.get(&lane("sit2")), Some(&LaneImpact::Whole));
    assert_eq!(a.changed, 5);

    // The mirror no longer has the old commit: nothing says what changed.
    let lost = CommitId::parse(&"ab".repeat(32)).unwrap();
    let a = moved(&s, RepoKind::Base, "master", Some(&lost), &big).await;
    assert_eq!(a.lanes.get(&lane("sit1")), Some(&LaneImpact::Whole));
    assert_eq!(s.metrics.head_move_count(MoveResult::Whole), 2);
    assert_eq!(s.metrics.head_move_count(MoveResult::Affected), 1);
}

#[tokio::test]
async fn an_unreadable_mirror_is_a_generic_error() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    let fake = s.git.git();
    let c1 = fake
        .commit(RepoKind::Base, "master", &[("config/a.yml", Some(b"1"))], "1")
        .unwrap();
    s.git.set_failing(true);
    let err = s
        .index
        .affected(RepoKind::Base, "master", None, &c1)
        .await
        .unwrap_err();
    assert_eq!(err, RegistryError::Git(GitError::Unavailable));
    assert!(
        !err.to_string().contains("master"),
        "no input in the error text (S21)"
    );
    s.git.set_failing(false);
    assert!(!moved(&s, RepoKind::Base, "master", None, &c1).await.is_empty());
}

#[tokio::test]
async fn only_a_head_moved_event_is_handled() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    s.rig.add_swimlane("sit1", &["sit1"], true).await;
    let fake = s.git.git();
    let c1 = fake
        .commit(RepoKind::Base, "master", &[("config/a.yml", Some(b"1"))], "1")
        .unwrap();
    let c2 = fake
        .commit(RepoKind::Base, "master", &[("config/a.yml", Some(b"2"))], "2")
        .unwrap();
    let event = DomainEvent::GitHeadMoved {
        repo: RepoKind::Base,
        branch: GitRef::parse("master").unwrap(),
        from: Some(c1),
        to: c2,
    };
    let a = s.index.on_event(&event).await.unwrap().expect("a head moved");
    assert_eq!(paths(&a, "sit1").unwrap(), ["a.yml"]);
    assert_eq!(s.index.on_event(&DomainEvent::Resync).await.unwrap(), None);
}

// ------------------------------------------------------------------------------------------------ the hash of a Git file

fn commit_files(fake: &FakeGit, files: &[(&str, &[u8])]) -> CommitId {
    let changes: Vec<(&str, Option<&[u8]>)> = files.iter().map(|(p, c)| (*p, Some(*c))).collect();
    fake.commit(RepoKind::Base, "master", &changes, "c").unwrap()
}

#[tokio::test]
async fn the_normalised_hash_of_a_git_file_is_read_once_per_object() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    let fake = s.git.git();
    let c1 = commit_files(
        fake,
        &[
            ("a.yml", b"a: 1\r\nb: 2\r\n"),
            ("same-bytes.yml", b"a: 1\r\nb: 2\r\n"),
            ("lf.yml", b"a: 1\nb: 2\n"),
            ("pic.bmp", b"a: 1\r\nb: 2\r\n"),
        ],
    );
    let get = |commit: &CommitId, path: &str, class: FileClass| {
        s.index
            .git_hash(RepoKind::Base, commit, &repo_path(path), class)
            .unwrap()
    };
    let h = get(&c1, "a.yml", FileClass::Structured).expect("the file is there");
    assert_eq!(
        Some(h),
        drift_hash(
            Some(&Bytes::from_static(b"a: 1\r\nb: 2\r\n")),
            FileClass::Structured
        ),
        "the engine's drift hash, not the raw one"
    );
    assert_eq!(s.git.reads(), 1);
    assert_eq!(s.metrics.git_hash_count(CacheResult::Miss), 1);

    // The same bytes under another name are the same object: no second read.
    assert_eq!(get(&c1, "same-bytes.yml", FileClass::Structured), Some(h));
    assert_eq!(s.git.reads(), 1, "one object, one read");
    assert_eq!(s.metrics.git_hash_count(CacheResult::Hit), 1);

    // LF and CRLF files hash alike for drift (a file saved with other line endings is not drift) but are two objects.
    assert_eq!(get(&c1, "lf.yml", FileClass::Structured), Some(h));
    assert_eq!(s.git.reads(), 2);

    // A later commit that leaves the object alone costs nothing.
    let c2 = commit_files(fake, &[("other.yml", b"z: 1\n")]);
    assert_eq!(get(&c2, "a.yml", FileClass::Structured), Some(h));
    assert_eq!(
        s.git.reads(),
        2,
        "the cache is keyed by the object, not the commit"
    );

    // The class changes the answer: the same object as a binary file is its raw hash. That hash is the tree index's own SHA-256, so
    // it needs no read at all (T3: a binary file is not read, and a denied one must never be).
    let raw = get(&c1, "pic.bmp", FileClass::Binary).unwrap();
    assert_ne!(raw, h, "a binary file is hashed as it is");
    assert_eq!(
        Some(raw),
        drift_hash(Some(&Bytes::from_static(b"a: 1\r\nb: 2\r\n")), FileClass::Binary)
    );
    assert_eq!(
        s.git.reads(),
        2,
        "the raw hash comes from the index, not from the blob"
    );

    // A path the commit does not have is nothing, and reads nothing.
    assert_eq!(get(&c1, "missing.yml", FileClass::Structured), None);
    assert_eq!(s.git.reads(), 2);
    assert!(
        s.index
            .git_hash(
                RepoKind::Base,
                &CommitId::parse(&"cd".repeat(32)).unwrap(),
                &repo_path("a.yml"),
                FileClass::Structured
            )
            .is_err()
    );
}

/// D79, S17: the content of a denied file is never read, not even from Git. Its hash for comparing is the raw SHA-256 that the tree
/// index already holds, so the answer is the same as reading it would give, and no key committed to a repository passes through the
/// hub's memory because a drift check asked about it. The same holds for a binary file, for the cost of it.
#[tokio::test]
async fn a_denied_or_binary_file_is_never_read_from_git() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    let secret = b"-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----\r\n";
    let c1 = commit_files(
        s.git.git(),
        &[("server.key", secret), ("pic.bmp", b"BM\x00\x01\r\n")],
    );
    let hash = |path: &str, class: FileClass| {
        s.index
            .git_hash(RepoKind::Base, &c1, &repo_path(path), class)
            .unwrap()
    };
    assert_eq!(
        hash("server.key", FileClass::Denied),
        drift_hash(Some(&Bytes::from_static(secret)), FileClass::Denied),
        "the hash is what reading the file would give"
    );
    assert_eq!(
        hash("pic.bmp", FileClass::Binary),
        drift_hash(Some(&Bytes::from_static(b"BM\x00\x01\r\n")), FileClass::Binary)
    );
    assert_eq!(s.git.reads(), 0, "no blob was read from the repository");
    assert_eq!(
        s.metrics.git_hash_count(CacheResult::Miss),
        0,
        "and the cache of read blobs was not involved"
    );
    assert_eq!(hash("absent.key", FileClass::Denied), None);
}

#[tokio::test]
async fn concurrent_callers_read_a_blob_once() {
    let Some(s) = setup(RegistryConfig::default()).await else {
        return;
    };
    let c1 = commit_files(s.git.git(), &[("a.yml", b"a: 1\n")]);
    let barrier = std::sync::Barrier::new(8);
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    s.index
                        .git_hash(RepoKind::Base, &c1, &repo_path("a.yml"), FileClass::Structured)
                        .unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(results.iter().all(|r| *r == results[0] && r.is_some()));
    assert_eq!(s.git.reads(), 1, "eight callers, one read");
}

#[tokio::test]
async fn the_hash_cache_is_bounded_in_bytes() {
    let config = RegistryConfig {
        git_hash_cache_bytes: 1024 * 1024,
        ..RegistryConfig::default()
    };
    let Some(s) = setup(config).await else { return };
    let owned: Vec<(String, Vec<u8>)> = (0..12_000)
        .map(|i| (format!("f{i}.yml"), format!("n: {i}\n").into_bytes()))
        .collect();
    let files: Vec<(&str, &[u8])> = owned.iter().map(|(p, c)| (p.as_str(), c.as_slice())).collect();
    let c1 = commit_files(s.git.git(), &files);
    for (p, _) in &owned {
        s.index
            .git_hash(RepoKind::Base, &c1, &repo_path(p), FileClass::Structured)
            .unwrap();
    }
    let (entries, weight) = s.index.cache_stats();
    assert!(
        weight <= 1024 * 1024,
        "the cache weighs {weight} bytes against a bound of 1 MiB"
    );
    assert!(
        entries < 12_000 && entries > 1_000,
        "{entries} entries: bounded, and still useful"
    );
}
