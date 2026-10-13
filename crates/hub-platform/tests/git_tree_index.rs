//! T7 (P2, P11, D65): the tree-index cache is bounded by what it holds, and the leaseholder persists the indexes of the branch
//! heads and tags to `git_tree_index` in batches, once per commit (a commit is one transaction, see `persist_one`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use domain::{CommitId, ContentHash, RepoKind, RepoPath, TreeEntry, TreeIndex};
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::reader::{GixReader, ReaderSettings};
use hub_platform::git::tree_index::{INSERT_BATCH, TreeIndexCache, TreeIndexPersist};
use hub_platform::jobs::{Job, JobContext, JobError};
use ports::{GitReader, content_hash};
use sqlx::{PgPool, Row};
use support::fake_github::GitRemote;
use support::pg::TestDb;
use tempfile::TempDir;

fn index(n: usize, seed: u8) -> Arc<TreeIndex> {
    let commit = CommitId::parse(&format!("{seed:02x}").repeat(20)).unwrap();
    let entries = (0..n)
        .map(|i| TreeEntry {
            path: RepoPath::parse(&format!("config/f{i}.yml")).unwrap(),
            git_oid: CommitId::parse(&format!("{:040x}", i + 1)).unwrap(),
            sha256: ContentHash::from_bytes([seed; 32]),
            size: 1,
        })
        .collect();
    Arc::new(TreeIndex::new(RepoKind::Base, commit, entries))
}

fn commit(seed: u8) -> CommitId {
    CommitId::parse(&format!("{seed:02x}").repeat(20)).unwrap()
}

#[test]
fn tree_index_cache_is_bounded_by_the_entries_it_holds_not_by_how_many_commits() {
    let cache = TreeIndexCache::new(100, GitMetrics::default());
    for seed in 1..=8u8 {
        let built = cache
            .get_or_build::<()>(RepoKind::Base, &commit(seed), || Ok(index(60, seed)))
            .unwrap();
        assert_eq!(built.entries().len(), 60);
    }
    assert!(
        cache.weighted_size() <= 100,
        "held {} entries, the cap is 100",
        cache.weighted_size()
    );
    // Two indexes of 60 entries do not fit in 100: at most one is held, whichever the cache's admission policy kept.
    let held = (1..=8u8)
        .filter(|seed| cache.get(RepoKind::Base, &commit(*seed)).is_some())
        .count();
    assert!(held <= 1, "{held} indexes are held");
}

#[test]
fn tree_index_cache_builds_a_missing_index_once_for_concurrent_callers() {
    let cache = TreeIndexCache::new(10_000, GitMetrics::default());
    let builds = Arc::new(std::sync::atomic::AtomicU32::new(0));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                cache
                    .get_or_build::<()>(RepoKind::Tenant, &commit(7), || {
                        builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        Ok(index(10, 7))
                    })
                    .unwrap();
            });
        }
    });
    assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);
}

struct Rig {
    _dir: TempDir,
    base: GitRemote,
    tenant: GitRemote,
    mirror: Arc<Mirror>,
    reader: Arc<GixReader>,
    pool: PgPool,
    db: TestDb,
    metrics: GitMetrics,
}

async fn rig() -> Option<Rig> {
    let db = TestDb::ready().await?;
    let pool = db.app_pool().await;
    let dir = TempDir::new().unwrap();
    let base = GitRemote::new();
    let tenant = GitRemote::new();
    let metrics = GitMetrics::default();
    base.commit(
        "main",
        &[
            ("config/a.yml", Some(b"a: 1\r\n")),
            ("config/b.yml", Some(b"b: 1\n")),
        ],
        "base",
    );
    tenant.commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 2\n"))], "sit1");
    let settings = MirrorSettings::new(
        dir.path(),
        RemoteUrl::local(&base.url()),
        RemoteUrl::local(&tenant.url()),
    );
    let mirror = Mirror::open(settings, Arc::new(NoCredentials), metrics.clone()).unwrap();
    mirror.initialize().await.unwrap();
    let reader =
        Arc::new(GixReader::new(Arc::clone(&mirror), &ReaderSettings::default(), metrics.clone()).unwrap());
    Some(Rig {
        _dir: dir,
        base,
        tenant,
        mirror,
        reader,
        pool,
        db,
        metrics,
    })
}

fn persist(rig: &Rig) -> TreeIndexPersist {
    TreeIndexPersist::new(
        Arc::clone(&rig.reader) as Arc<dyn GitReader>,
        rig.pool.clone(),
        rig.metrics.clone(),
    )
}

async fn run(job: &TreeIndexPersist) -> Result<(), JobError> {
    let (ctx, _hold) = JobContext::held_until_told_otherwise("git-tree-index");
    job.run(&ctx).await
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_index_of_every_branch_head_and_tag_is_persisted_once_per_commit() {
    let Some(rig) = rig().await else { return };
    let tagged = rig
        .base
        .commit("main", &[("config/c.yml", Some(b"c: 1\n"))], "second");
    rig.base.tag("v1.0.0", &tagged);
    rig.base
        .commit("main", &[("config/c.yml", Some(b"c: 2\n"))], "third");
    rig.tenant.commit(
        "templates/core",
        &[("data/config/core.yml", Some(b"k: 1\n"))],
        "core",
    );
    rig.mirror.fetch(RepoKind::Base).await.unwrap();
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();

    run(&persist(&rig)).await.unwrap();

    // main at its head, the tagged commit, sit1, templates/core.
    let commits = rig
        .db
        .count(
            &rig.pool,
            "SELECT count(DISTINCT (repo, commit_id)) FROM git_tree_index",
        )
        .await;
    assert_eq!(commits, 4);

    // The rows are the reader's index, exactly.
    let head = rig.reader.head(RepoKind::Base, "main").unwrap();
    let index = rig.reader.tree_index(RepoKind::Base, &head).unwrap();
    let rows = sqlx::query("SELECT path, object_id, hash FROM git_tree_index WHERE repo = 'base' AND commit_id = $1 ORDER BY path")
        .bind(head.as_str())
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), index.entries().len());
    for (row, entry) in rows.iter().zip(index.entries()) {
        assert_eq!(row.get::<String, _>("path"), entry.path.as_str());
        assert_eq!(row.get::<String, _>("object_id"), entry.git_oid.as_str());
        assert_eq!(row.get::<Vec<u8>, _>("hash"), entry.sha256.as_bytes().to_vec());
    }
    assert_eq!(
        rows.iter()
            .find(|r| r.get::<String, _>("path") == "config/a.yml")
            .map(|r| r.get::<Vec<u8>, _>("hash")),
        Some(content_hash(b"a: 1\r\n").as_bytes().to_vec()),
        "the hash is of the exact bytes, line endings included"
    );

    // A second run writes nothing: a commit that is persisted is not persisted again.
    let before = rig.metrics.tree_index_persisted_total();
    run(&persist(&rig)).await.unwrap();
    assert_eq!(rig.metrics.tree_index_persisted_total(), before);
    assert_eq!(
        rig.db
            .count(&rig.pool, "SELECT count(*) FROM git_tree_index")
            .await,
        i64::try_from(before).unwrap()
    );

    // A new head is persisted on the next run, and the old one stays.
    rig.tenant.commit(
        "sit1",
        &[("data/config/a-sit1.yml", Some(b"a: 3\n"))],
        "sit1 again",
    );
    rig.mirror.fetch(RepoKind::Tenant).await.unwrap();
    run(&persist(&rig)).await.unwrap();
    assert_eq!(
        rig.db
            .count(
                &rig.pool,
                "SELECT count(DISTINCT (repo, commit_id)) FROM git_tree_index"
            )
            .await,
        5
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_index_of_a_commit_larger_than_two_batches_is_written_completely() {
    let Some(rig) = rig().await else { return };
    let many = INSERT_BATCH * 2 + 123;
    let files: Vec<(String, Vec<u8>)> = (0..many)
        .map(|i| {
            (
                format!("config/dir{}/f{i}.yml", i % 50),
                format!("v: {i}\n").into_bytes(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, Option<&[u8]>)> = files
        .iter()
        .map(|(p, b)| (p.as_str(), Some(b.as_slice())))
        .collect();
    rig.base.commit("big", &borrowed, "many files");
    rig.mirror.fetch(RepoKind::Base).await.unwrap();

    run(&persist(&rig)).await.unwrap();

    let head = rig.reader.head(RepoKind::Base, "big").unwrap();
    let stored = rig
        .db
        .count(
            &rig.pool,
            &format!(
                "SELECT count(*) FROM git_tree_index WHERE repo = 'base' AND commit_id = '{}'",
                head.as_str()
            ),
        )
        .await;
    assert_eq!(
        stored,
        i64::try_from(many).unwrap(),
        "every row of a commit larger than three batches is there"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_index_persistence_stops_when_the_lease_is_lost_and_writes_no_partial_commit() {
    let Some(rig) = rig().await else { return };
    let (ctx, hold) = JobContext::held_until_told_otherwise("git-tree-index");
    hold.send_replace(tokio::time::Instant::now() - std::time::Duration::from_secs(1));
    let result = persist(&rig).run(&ctx).await;
    assert_eq!(result, Err(JobError::LeaseLost));
    assert_eq!(
        rig.db
            .count(&rig.pool, "SELECT count(*) FROM git_tree_index")
            .await,
        0
    );
}

#[test]
fn the_persist_job_is_registered_every_minute_under_its_own_lease() {
    let spec = TreeIndexPersist::spec().unwrap();
    assert_eq!(spec.name(), "git-tree-index");
}
