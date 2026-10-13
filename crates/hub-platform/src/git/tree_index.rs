//! The Git tree index of a commit: every regular file, with its Git blob id and the SHA-256 of its bytes (D65, P2).
//!
//! An index is built once per commit and kept in a moka cache weighted by its number of entries, so the cache is bounded by what it
//! holds and not by how many commits it holds. Commits never change, so a cached index is never stale and is never invalidated.
//!
//! [`TreeIndexPersist`] writes the indexes of the branch heads to `git_tree_index`, in batches, from the replica that holds the
//! `git-tree-index` lease (so the table has one writer at a time); the hash column is what the registry joins against.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{CommitId, RepoKind, TreeIndex};
use moka::sync::Cache;
use ports::{GitError, GitReader};
use sqlx::PgPool;
use tracing::{info, warn};

use super::metrics::{CacheResult, GitMetrics};
use crate::jobs::{Job, JobContext, JobError, JobSpec, SpecError};

/// Rows per `INSERT`. A statement binds three arrays of this many values.
pub const INSERT_BATCH: usize = 5_000;
/// The lease and the job's name.
pub const PERSIST_JOB: &str = "git-tree-index";
/// One database call.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// A bounded cache of tree indexes, keyed by repository and commit.
#[derive(Clone)]
pub struct TreeIndexCache {
    cache: Cache<(RepoKind, CommitId), Arc<TreeIndex>>,
    metrics: GitMetrics,
}

impl std::fmt::Debug for TreeIndexCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeIndexCache")
            .field("entries", &self.cache.weighted_size())
            .finish_non_exhaustive()
    }
}

/// An index weighs its number of entries (at least one, so an empty tree still counts).
fn weight(index: &Arc<TreeIndex>) -> u32 {
    u32::try_from(index.entries().len().max(1)).unwrap_or(u32::MAX)
}

impl TreeIndexCache {
    /// Holds at most `max_entries` index entries in all.
    #[must_use]
    pub fn new(max_entries: u64, metrics: GitMetrics) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_entries.max(1))
            .weigher(|_key: &(RepoKind, CommitId), index: &Arc<TreeIndex>| weight(index))
            .build();
        Self { cache, metrics }
    }

    /// The cached index, counting a hit or a miss.
    #[must_use]
    pub fn get(&self, repo: RepoKind, commit: &CommitId) -> Option<Arc<TreeIndex>> {
        let found = self.cache.get(&(repo, commit.clone()));
        self.metrics.tree_index(if found.is_some() {
            CacheResult::Hit
        } else {
            CacheResult::Miss
        });
        found
    }

    /// The cached index, or the one `build` makes. Callers that ask for the same missing index at the same time share one build.
    pub fn get_or_build<E: Send + Sync + 'static>(
        &self,
        repo: RepoKind,
        commit: &CommitId,
        build: impl FnOnce() -> Result<Arc<TreeIndex>, E>,
    ) -> Result<Arc<TreeIndex>, Arc<E>> {
        let result = self.cache.try_get_with((repo, commit.clone()), build);
        self.metrics.set_tree_index_entries(self.cache.weighted_size());
        result
    }

    /// Entries held right now (moka settles its bookkeeping lazily, so this is approximate under load).
    #[must_use]
    pub fn weighted_size(&self) -> u64 {
        self.cache.run_pending_tasks();
        self.cache.weighted_size()
    }
}

// ------------------------------------------------------------------------------------------------ persistence

/// Writes the tree index of every branch head and tag commit to `git_tree_index`, once per commit.
///
/// A commit's rows are one transaction, so a crash leaves all of them or none, and "this commit has rows" means "this commit is
/// done". Rows are written `ON CONFLICT DO NOTHING`, so a second replica that raced for the lease changes nothing. The lease is
/// checked before each commit. The indexes are built (or found) through the [`GitReader`], so the work is the cached work.
pub struct TreeIndexPersist {
    reader: Arc<dyn GitReader>,
    pool: PgPool,
    metrics: GitMetrics,
}

impl std::fmt::Debug for TreeIndexPersist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeIndexPersist").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistError {
    Database,
    Git(GitError),
}

impl TreeIndexPersist {
    #[must_use]
    pub fn new(reader: Arc<dyn GitReader>, pool: PgPool, metrics: GitMetrics) -> Self {
        Self {
            reader,
            pool,
            metrics,
        }
    }

    /// Every minute, under its own lease, held for three minutes without renewal.
    pub fn spec() -> Result<JobSpec, SpecError> {
        JobSpec::new(PERSIST_JOB, Duration::from_secs(60), Duration::from_secs(180))?
            .with_max_run(Duration::from_secs(10 * 60))
    }

    /// The commits to persist for `repo`: every branch head and every tag's commit.
    async fn commits(&self, repo: RepoKind) -> Result<BTreeSet<CommitId>, GitError> {
        let reader = Arc::clone(&self.reader);
        tokio::task::spawn_blocking(move || {
            let mut out = BTreeSet::new();
            out.extend(reader.branches(repo)?.into_iter().map(|b| b.head));
            out.extend(reader.tags(repo)?.into_iter().map(|t| t.commit));
            Ok(out)
        })
        .await
        .map_err(|_| GitError::Unavailable)?
    }

    async fn persisted(&self, repo: RepoKind, commit: &CommitId) -> Result<bool, PersistError> {
        let call = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM git_tree_index WHERE repo = $1 AND commit_id = $2)",
        )
        .bind(repo.as_str())
        .bind(commit.as_str())
        .fetch_one(&self.pool);
        match tokio::time::timeout(QUERY_TIMEOUT, call).await {
            Ok(Ok(found)) => Ok(found),
            Ok(Err(_)) | Err(_) => Err(PersistError::Database),
        }
    }

    async fn persist_one(&self, repo: RepoKind, commit: &CommitId) -> Result<u64, PersistError> {
        let reader = Arc::clone(&self.reader);
        let id = commit.clone();
        let index = tokio::task::spawn_blocking(move || reader.tree_index(repo, &id))
            .await
            .map_err(|_| PersistError::Git(GitError::Unavailable))?
            .map_err(PersistError::Git)?;
        let entries = index.entries();
        if entries.is_empty() {
            return Ok(0);
        }
        let work = async {
            let mut tx = self.pool.begin().await?;
            let mut written = 0_u64;
            for chunk in entries.chunks(INSERT_BATCH) {
                let paths: Vec<&str> = chunk.iter().map(|e| e.path.as_str()).collect();
                let oids: Vec<&str> = chunk.iter().map(|e| e.git_oid.as_str()).collect();
                let hashes: Vec<&[u8]> = chunk.iter().map(|e| &e.sha256.as_bytes()[..]).collect();
                let done = sqlx::query(
                    "INSERT INTO git_tree_index (repo, commit_id, path, object_id, hash) \
                     SELECT $1, $2, p, o, h FROM UNNEST($3::text[], $4::text[], $5::bytea[]) AS t(p, o, h) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(repo.as_str())
                .bind(commit.as_str())
                .bind(&paths)
                .bind(&oids)
                .bind(&hashes)
                .execute(&mut *tx)
                .await?;
                written += done.rows_affected();
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(written)
        };
        // A transaction cut off by the timeout is rolled back when it is dropped: no partial commit.
        match tokio::time::timeout(QUERY_TIMEOUT * 4, work).await {
            Ok(Ok(written)) => Ok(written),
            Ok(Err(_)) | Err(_) => Err(PersistError::Database),
        }
    }
}

#[async_trait]
impl Job for TreeIndexPersist {
    async fn run(&self, ctx: &JobContext) -> Result<(), JobError> {
        let mut failed = false;
        for repo in [RepoKind::Base, RepoKind::Tenant] {
            ctx.ensure_held()?;
            let Ok(commits) = self.commits(repo).await else {
                failed = true;
                continue;
            };
            for commit in commits {
                ctx.ensure_held()?;
                match self.persisted(repo, &commit).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(_) => {
                        failed = true;
                        continue;
                    }
                }
                match self.persist_one(repo, &commit).await {
                    Ok(rows) => {
                        self.metrics.tree_index_persisted(rows);
                        info!(
                            code = "git_tree_index_persisted",
                            repo = repo.as_str(),
                            rows,
                            "tree index persisted"
                        );
                    }
                    Err(e) => {
                        failed = true;
                        warn!(code = "git_tree_index_persist_failed", repo = repo.as_str(), error = ?e, "tree index not persisted");
                    }
                }
            }
        }
        if failed {
            Err(JobError::Failed("git_tree_index_persist"))
        } else {
            Ok(())
        }
    }
}
