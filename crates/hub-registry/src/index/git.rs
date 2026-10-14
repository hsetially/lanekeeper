//! The Git side of drift (T2, D65, P2): which swimlane paths a moved head changes, and the normalised hash of a file in Git.
//!
//! # What this does not do
//!
//! It never writes `git_tree_index`. That table has one writer, hub-platform's `TreeIndexPersist` (lease `git-tree-index`, plan A22);
//! this reads the index the mirror keeps (`GitReader::tree_index`, cached per commit) and diffs trees (`GitReader::diff_trees`).
//!
//! # A moved head (`GitHeadMoved`)
//!
//! [`GitIndex::affected`] turns "branch B of repo R went from commit F to commit T" into the `(swimlane, NFS path)` pairs whose Git
//! side changed, so the pipeline recomputes those and nothing else (P2):
//!
//! | Branch | Swimlanes concerned |
//! |---|---|
//! | the base repo's default branch | every swimlane that has had its first inventory |
//! | a deployable tenant branch `B` | the swimlanes that have `B` in their tenant set (D84, Q34) |
//! | a template, the docs branch, any other branch, a branch the mirror does not have | none |
//!
//! A swimlane that has had no inventory yet has nothing to compare, and the first inventory recomputes it whole. The repository
//! paths of the diff become NFS paths with the engine's mapping (`config/<rel>` to `<rel>`, `data/config/<rel>` on branch `B` to
//! `<name>-B.<ext>`); a path below no root (a README, a workflow file) is not configuration and is counted and dropped. A new
//! branch (`from` is `None`) is every file of its tree. When the diff names more than `max_affected_paths`, or the mirror no longer
//! has the old commit, the swimlane is recomputed whole ([`LaneImpact::Whole`]): that is slower and never wrong.
//!
//! # The hash of a Git file
//!
//! Drift compares normalised hashes (line endings, `drift_hash`), but the tree index holds the raw SHA-256. [`GitIndex::git_hash`] reads
//! the blob once and caches the normalised hash by `(repo, object id, class)`, in a cache bounded in bytes. A nightly run therefore
//! reads each distinct Git blob once, however many swimlanes and commits name it. The class is part of the key because the same bytes
//! under a `.yml` and a `.png` name hash differently. Only a `structured` or `text` file needs the blob, to normalise its line endings:
//! a `binary` or `denied` file is compared as it is, so its hash is the index's own SHA-256 and the blob is never read (for a denied
//! file that is a rule: its content is not read for anyone, D79).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::{
    BranchKind, CommitId, ContentHash, DomainEvent, FileClass, NfsPath, PathChange, PathChangeKind,
    PathMappingRule, RepoKind, RepoPath, SwimlaneId, TenantId,
};
use engine::drift_hash;
use moka::sync::Cache;
use ports::{GitError, GitReader};
use sqlx::PgPool;
use tracing::debug;

use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::ingest::rules::rule_of;
use crate::metrics::{CacheResult, MoveResult, RegistryMetrics};

/// The most swimlanes one move can concern.
const MAX_SWIMLANES: i64 = 10_000;

/// What a cache key weighs besides its text: the repo, the class, the value and the cache's own bookkeeping.
const KEY_OVERHEAD: usize = 96;

/// What a moved head means for one swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneImpact {
    /// Recompute these paths. The set is shared by every swimlane the move concerns.
    Paths(Arc<BTreeSet<NfsPath>>),
    /// Recompute the whole swimlane: the move is too large to list, or its old side is gone.
    Whole,
}

/// The swimlanes and paths a moved head changes. Empty when it changes none.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Affected {
    pub lanes: BTreeMap<SwimlaneId, LaneImpact>,
    /// Repository paths the diff named.
    pub changed: usize,
    /// Of those, paths below no repository root: not configuration.
    pub unmapped: usize,
}

impl Affected {
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }
}

/// What the Git index needs from the rest of the hub.
pub struct GitIndexDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub git: Arc<dyn GitReader>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for GitIndexDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitIndexDeps").finish_non_exhaustive()
    }
}

type HashKey = (RepoKind, CommitId, FileClass);

struct Inner {
    db: Db,
    git: Arc<dyn GitReader>,
    config: RegistryConfig,
    metrics: RegistryMetrics,
    hashes: Cache<HashKey, ContentHash>,
}

#[derive(Clone)]
pub struct GitIndex {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GitIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitIndex").finish_non_exhaustive()
    }
}

impl GitIndex {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: GitIndexDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let hashes = Cache::builder()
            .max_capacity(deps.config.git_hash_cache_bytes)
            .weigher(|key: &HashKey, _: &ContentHash| {
                u32::try_from(key.1.as_str().len() + KEY_OVERHEAD).unwrap_or(u32::MAX)
            })
            .build();
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                git: deps.git,
                config: deps.config,
                metrics: deps.metrics,
                hashes,
            }),
        })
    }

    /// The swimlanes and paths that a `GitHeadMoved` event changes, or `None` for any other event.
    pub async fn on_event(&self, event: &DomainEvent) -> Result<Option<Affected>, RegistryError> {
        match event {
            DomainEvent::GitHeadMoved {
                repo,
                branch,
                from,
                to,
            } => self
                .affected(*repo, branch.as_str(), from.as_ref(), to)
                .await
                .map(Some),
            _ => Ok(None),
        }
    }

    /// The `(swimlane, NFS path)` pairs whose Git side changed when `branch` of `repo` went from `from` to `to`.
    pub async fn affected(
        &self,
        repo: RepoKind,
        branch: &str,
        from: Option<&CommitId>,
        to: &CommitId,
    ) -> Result<Affected, RegistryError> {
        let inner = &self.inner;
        let timeout = inner.config.git_timeout;
        // Whether the branch is deployed at all, from the mirror's own classification.
        let kind = {
            let (git, branch) = (Arc::clone(&inner.git), branch.to_owned());
            blocking(timeout, move || {
                Ok(git
                    .branches(repo)?
                    .into_iter()
                    .find(|b| b.name.as_str() == branch)
                    .map(|b| b.kind))
            })
            .await?
        };
        let tenant = match (repo, kind) {
            (RepoKind::Base, Some(BranchKind::Default)) => None,
            (RepoKind::Tenant, Some(BranchKind::Deployable)) => match TenantId::parse(branch) {
                Ok(t) => Some(t),
                Err(_) => return Ok(self.unconcerned()),
            },
            _ => return Ok(self.unconcerned()),
        };
        let swimlanes = inner.concerned(tenant.as_ref()).await?;
        if swimlanes.is_empty() {
            return Ok(self.unconcerned());
        }
        let rule = inner.rule().await?;

        let (git, from, to) = (Arc::clone(&inner.git), from.cloned(), to.clone());
        let limit = inner.config.max_affected_paths;
        let diff = blocking(timeout, move || {
            Ok(changes(git.as_ref(), repo, from.as_ref(), &to))
        })
        .await?;
        let affected = match diff {
            // The old side is gone from the mirror: nothing says what changed.
            Err(GitError::NotFound) => Affected {
                lanes: swimlanes.into_iter().map(|s| (s, LaneImpact::Whole)).collect(),
                ..Affected::default()
            },
            Err(e) => return Err(e.into()),
            Ok(diff) if diff.len() > limit => Affected {
                lanes: swimlanes.into_iter().map(|s| (s, LaneImpact::Whole)).collect(),
                changed: diff.len(),
                unmapped: 0,
            },
            Ok(diff) => {
                let (paths, unmapped) = map_changes(&rule, tenant.as_ref(), &diff);
                if paths.is_empty() {
                    Affected {
                        changed: diff.len(),
                        unmapped,
                        ..Affected::default()
                    }
                } else {
                    let shared = Arc::new(paths);
                    Affected {
                        lanes: swimlanes
                            .into_iter()
                            .map(|s| (s, LaneImpact::Paths(Arc::clone(&shared))))
                            .collect(),
                        changed: diff.len(),
                        unmapped,
                    }
                }
            }
        };
        let result = if affected.is_empty() {
            MoveResult::Unconcerned
        } else if affected.lanes.values().any(|l| *l == LaneImpact::Whole) {
            MoveResult::Whole
        } else {
            MoveResult::Affected
        };
        inner.metrics.head_move(result);
        debug!(
            swimlanes = affected.lanes.len(),
            changed = affected.changed,
            unmapped = affected.unmapped,
            "git head moved"
        );
        Ok(affected)
    }

    fn unconcerned(&self) -> Affected {
        self.inner.metrics.head_move(MoveResult::Unconcerned);
        Affected::default()
    }

    /// The hash of a file in Git for drift (`engine::drift_hash` of its bytes), or `None` when the commit has no such path. Reads the
    /// blob once per `(repo, object id, class)`. Synchronous like the reader: call it inside `spawn_blocking`.
    pub fn git_hash(
        &self,
        repo: RepoKind,
        commit: &CommitId,
        path: &RepoPath,
        class: FileClass,
    ) -> Result<Option<ContentHash>, GitError> {
        let inner = &self.inner;
        let index = inner.git.tree_index(repo, commit)?;
        let Some(entry) = index.get(path) else {
            return Ok(None);
        };
        // A binary or denied file is compared as it is, so its hash for drift is the SHA-256 the index already holds: no read, and
        // no cache entry. For a denied file this is a rule, not a saving: its content is never read for anyone (D79, S17), and a key
        // that is committed to a repository must not pass through the hub's memory because a drift check asked about it.
        if matches!(class, FileClass::Binary | FileClass::Denied) {
            return Ok(Some(entry.sha256));
        }
        let key = (repo, entry.git_oid.clone(), class);
        if let Some(hash) = inner.hashes.get(&key) {
            inner.metrics.git_hash(CacheResult::Hit);
            return Ok(Some(hash));
        }
        inner.metrics.git_hash(CacheResult::Miss);
        // One reader per key: a second caller for the same blob waits for the first and takes its answer.
        let hash = inner
            .hashes
            .try_get_with(key, || -> Result<ContentHash, GitError> {
                let bytes = inner.git.read(repo, commit, path)?.ok_or(GitError::NotFound)?;
                drift_hash(Some(&bytes), class).ok_or(GitError::Unavailable)
            })
            .map_err(|e| *e);
        match hash {
            Ok(hash) => Ok(Some(hash)),
            // The commit has the path in its index but not the blob (the repository changed under us): nothing to say.
            Err(GitError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Hashes in the cache and the weight they take, for tests.
    pub fn cache_stats(&self) -> (u64, u64) {
        self.inner.hashes.run_pending_tasks();
        (self.inner.hashes.entry_count(), self.inner.hashes.weighted_size())
    }
}

impl Inner {
    /// The swimlanes a move concerns: every one with an inventory (the base repo), or those that carry the tenant.
    async fn concerned(&self, tenant: Option<&TenantId>) -> Result<Vec<SwimlaneId>, RegistryError> {
        let ids: Vec<String> = match tenant {
            None => {
                self.db
                    .run(
                        "git_concerned_base",
                        sqlx::query_scalar(
                            "SELECT swimlane_id FROM swimlane_registry WHERE first_inventory_at IS NOT NULL \
                             ORDER BY swimlane_id LIMIT $1",
                        )
                        .bind(MAX_SWIMLANES)
                        .fetch_all(self.db.pool()),
                    )
                    .await?
            }
            Some(tenant) => {
                self.db
                    .run(
                        "git_concerned_tenant",
                        sqlx::query_scalar(
                            "SELECT t.swimlane_id FROM swimlane_tenants t \
                             JOIN swimlane_registry r ON r.swimlane_id = t.swimlane_id \
                             WHERE t.tenant_id = $1 AND r.first_inventory_at IS NOT NULL \
                             ORDER BY t.swimlane_id LIMIT $2",
                        )
                        .bind(tenant.as_str())
                        .bind(MAX_SWIMLANES)
                        .fetch_all(self.db.pool()),
                    )
                    .await?
            }
        };
        ids.iter()
            .map(|id| SwimlaneId::parse(id).map_err(|_| RegistryError::Corrupt))
            .collect()
    }

    async fn rule(&self) -> Result<PathMappingRule, RegistryError> {
        let roots: Vec<(String, String)> = self
            .db
            .run(
                "git_mapping",
                sqlx::query_as("SELECT repo, repo_root FROM path_mappings LIMIT 8").fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rule_of(&roots))
    }
}

/// Run a blocking Git call on the blocking pool, with a timeout for the caller (rule 4, rule 5). The call cannot be cancelled: a
/// timeout frees the caller, not the thread.
async fn blocking<T, F>(timeout: std::time::Duration, call: F) -> Result<T, RegistryError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, GitError> + Send + 'static,
{
    match tokio::time::timeout(timeout, tokio::task::spawn_blocking(call)).await {
        Ok(Ok(result)) => result.map_err(RegistryError::from),
        Ok(Err(_)) => Err(RegistryError::Git(GitError::Unavailable)),
        Err(_) => Err(RegistryError::Timeout),
    }
}

/// The paths that differ between two commits, or every path of `to` for a new branch.
fn changes(
    git: &dyn GitReader,
    repo: RepoKind,
    from: Option<&CommitId>,
    to: &CommitId,
) -> Result<Vec<PathChange>, GitError> {
    match from {
        Some(from) => git.diff_trees(repo, from, to),
        None => Ok(git
            .tree_index(repo, to)?
            .entries()
            .iter()
            .map(|e| PathChange {
                path: e.path.clone(),
                kind: PathChangeKind::Added,
            })
            .collect()),
    }
}

/// The NFS paths of a diff, and how many paths are below no root.
fn map_changes(
    rule: &PathMappingRule,
    tenant: Option<&TenantId>,
    diff: &[PathChange],
) -> (BTreeSet<NfsPath>, usize) {
    let mut paths = BTreeSet::new();
    let mut unmapped = 0;
    for change in diff {
        let mapped = match tenant {
            None => rule.base_to_nfs(&change.path),
            Some(tenant) => rule.tenant_to_nfs(&change.path, tenant),
        };
        match mapped {
            Ok(path) => {
                paths.insert(path);
            }
            Err(_) => unmapped += 1,
        }
    }
    (paths, unmapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(path: &str, kind: PathChangeKind) -> PathChange {
        PathChange {
            path: RepoPath::parse(path).expect("a valid repository path"),
            kind,
        }
    }

    #[test]
    fn base_paths_lose_their_root_and_nothing_else() {
        let rule = PathMappingRule::default();
        let diff = [
            change("config/app/core.yml", PathChangeKind::Modified),
            change("config/application.yml", PathChangeKind::Added),
            change("README.md", PathChangeKind::Modified),
            change(".github/workflows/ci.yml", PathChangeKind::Modified),
        ];
        let (paths, unmapped) = map_changes(&rule, None, &diff);
        let names: Vec<&str> = paths.iter().map(NfsPath::as_str).collect();
        assert_eq!(names, ["app/core.yml", "application.yml"]);
        assert_eq!(unmapped, 2, "a README and a workflow are not configuration");
    }

    #[test]
    fn tenant_paths_get_the_branch_as_a_suffix_before_the_extension() {
        let rule = PathMappingRule::default();
        let sit1 = TenantId::parse("sit1").expect("a tenant id");
        let diff = [
            change("data/config/app/core.yml", PathChangeKind::Modified),
            change("data/config/app/receipt.bmp", PathChangeKind::Added),
            change("data/config/NOTICE", PathChangeKind::Added),
            change("config/app/core.yml", PathChangeKind::Modified),
        ];
        let (paths, unmapped) = map_changes(&rule, Some(&sit1), &diff);
        let names: Vec<&str> = paths.iter().map(NfsPath::as_str).collect();
        assert_eq!(
            names,
            ["NOTICE-sit1", "app/core-sit1.yml", "app/receipt-sit1.bmp"]
        );
        assert_eq!(unmapped, 1, "a base path is not a tenant path");
    }

    #[test]
    fn a_deleted_path_is_still_a_path() {
        let rule = PathMappingRule::default();
        let (paths, _) = map_changes(&rule, None, &[change("config/gone.yml", PathChangeKind::Deleted)]);
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn two_repo_paths_that_map_to_one_nfs_path_are_one_path() {
        let rule = PathMappingRule::default();
        let diff = [
            change("config/a.yml", PathChangeKind::Modified),
            change("config/a.yml", PathChangeKind::Modified),
        ];
        assert_eq!(map_changes(&rule, None, &diff).0.len(), 1);
    }

    #[tokio::test]
    async fn a_blocking_call_that_does_not_answer_times_out() {
        // The thread waits on a channel until the test frees it, so it cannot answer before the (short) timeout runs out.
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let result: Result<(), RegistryError> = blocking(std::time::Duration::from_millis(1), move || {
            let _ = wait.recv();
            Ok(())
        })
        .await;
        assert_eq!(result, Err(RegistryError::Timeout));
        drop(release);
    }

    #[tokio::test]
    async fn a_blocking_call_that_fails_is_a_git_error_and_one_that_answers_is_its_answer() {
        let ok: Result<u8, RegistryError> = blocking(std::time::Duration::from_secs(5), || Ok(7)).await;
        assert_eq!(ok, Ok(7));
        let err: Result<u8, RegistryError> =
            blocking(std::time::Duration::from_secs(5), || Err(GitError::Unavailable)).await;
        assert_eq!(err, Err(RegistryError::Git(GitError::Unavailable)));
    }
}
