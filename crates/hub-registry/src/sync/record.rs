//! Baselines from a sync report (T4, D19, D21): a file whose NFS content equals Git **at the commits the report names** gets that as
//! its baseline.
//!
//! A baseline is "the Git commit and file hash at the last known sync" (D19). It is the commit the sync deployed, not today's head:
//! when Git has moved on since, the file is `git_ahead` (nobody has synced the new commit yet), which only a baseline at the deployed
//! commit can say. The pipeline sets a baseline on its own when NFS and Git agree at the heads (T3); this is the same decision made
//! against the commits of the report, and it is what makes the states right after a sync of an older commit.
//!
//! - A path is compared with the Git side that the path mapping names: a base file with the base commit, a tenant file of the reported
//!   tenant with the tenant commit. A tenant file of another tenant, and a path Git does not have, are left alone.
//! - A baseline that already says the same is not written (no event). One that changes is written with the raw version behind it
//!   (`blob_hash`, and a `blob_refs` row that keeps retention from deleting it, D78); the row of the version it replaces is dropped.
//!   A denied file has no blob: its content is never stored.
//! - Each chunk is one transaction under the swimlane's lock. The file is read again inside it: a newer version that ingestion applied
//!   since is not the one that equals Git, and the pipeline's own rule will converge it.

use std::time::Duration;

use domain::{ContentHash, FileClass, NfsPath, RepoKind, RepoPath, ReverseMapping, SwimlaneId, Timestamp};
use ports::{AuditAction, AuditEvent, AuditVia, Tx};

use super::{Inner, Report};
use crate::baselines::{self, RefChange};
use crate::db::Txn;
use crate::error::RegistryError;
use crate::ingest::apply::LOCK_SQL;
use crate::ingest::rules::LaneRules;
use crate::pipeline::blocking;
use crate::pipeline::persist::UPSERT_BASELINES_SQL;

/// The most rows of one swimlane read.
const MAX_ROWS: i64 = 200_001;

/// A `file_state` row of a file that is on NFS.
#[derive(Debug, Clone)]
pub(crate) struct NfsRow {
    pub path: NfsPath,
    /// `Denied` when the row says so, whatever the extension: its hash is compared as it is.
    pub class: FileClass,
    pub denied: bool,
    /// The raw hash of the content.
    pub nfs: ContentHash,
    /// N: the hash for comparing.
    pub drift: ContentHash,
    /// The observation of the current version.
    pub observation: Option<i64>,
}

/// Where Git has the file a path stands for, at the commits of the report.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    repo: RepoKind,
    repo_path: RepoPath,
    /// The tenant branch, or `None` for the base repository's default branch (as the pipeline's baselines have it).
    branch: Option<String>,
    commit: domain::CommitId,
}

/// A file with a Git side at the reported commits, and the hash of it for comparing (G).
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub row: NfsRow,
    pub target: Target,
    pub git: ContentHash,
}

type RowTuple = (String, String, bool, Vec<u8>, Option<Vec<u8>>, Option<i64>);

fn hash_of(bytes: Vec<u8>) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

/// A row, or `None` for one that has no hash for comparing yet (a row written before the column existed).
fn row_of(t: RowTuple) -> Result<Option<NfsRow>, RegistryError> {
    let (path, class, denied, nfs, drift, observation) = t;
    let Some(drift) = drift else { return Ok(None) };
    let class = std::str::FromStr::from_str(&class).map_err(|_| RegistryError::Corrupt)?;
    Ok(Some(NfsRow {
        path: NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
        class: if denied { FileClass::Denied } else { class },
        denied,
        nfs: hash_of(nfs)?,
        drift: hash_of(drift)?,
        observation,
    }))
}

/// What a path stands for in Git at the commits of the report: a base file in the base repository, a file of the reported tenant in the
/// tenant repository. Anything else (another tenant's file, a path no mapping can place) has no Git side here.
fn target_of(rules: &LaneRules, r: &Report, path: &NfsPath) -> Option<Target> {
    match rules.rule.reverse(path, &rules.tenants).ok()? {
        ReverseMapping::Base { repo_path, .. } => Some(Target {
            repo: RepoKind::Base,
            repo_path,
            branch: None,
            commit: r.base_commit.clone(),
        }),
        ReverseMapping::Tenant {
            tenant, repo_path, ..
        } if tenant == r.tenant => Some(Target {
            repo: RepoKind::Tenant,
            repo_path,
            branch: Some(tenant.as_str().to_owned()),
            commit: r.tenant_commit.clone(),
        }),
        ReverseMapping::Tenant { .. } => None,
    }
}

/// What the baseline row of a path holds now, as the chunk reads it.
#[derive(Debug, Clone)]
struct Current {
    path: String,
    drift: Option<Vec<u8>>,
    baseline: Option<Vec<u8>>,
    baseline_blob: Option<Vec<u8>>,
    blob_exists: bool,
}

type CurrentTuple = (String, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>, bool);

/// A baseline to write.
#[derive(Debug)]
struct Write<'a> {
    item: &'a Resolved,
    before: Option<Vec<u8>>,
    old_blob: Option<Vec<u8>>,
    blob: Option<ContentHash>,
}

/// Which of the files of a chunk get a baseline, given what the database holds for them now (read under the lock).
///
/// - A file whose hash for comparing is not the one that was resolved has a newer version since: it is not the one that equals Git.
/// - A file whose baseline already says it is left alone.
/// - The raw version behind the baseline is referenced only when it is stored, and never for a denied file.
fn decide_writes<'a>(chunk: &'a [Resolved], current: &[Current]) -> Vec<Write<'a>> {
    let mut writes = Vec::new();
    for item in chunk {
        let Some(now) = current.iter().find(|c| c.path == item.row.path.as_str()) else {
            continue;
        };
        if now.drift.as_deref() != Some(item.row.drift.as_bytes().as_slice()) {
            continue;
        }
        if now.baseline.as_deref() == Some(item.git.as_bytes().as_slice()) {
            continue;
        }
        writes.push(Write {
            item,
            before: now.baseline.clone(),
            old_blob: now.baseline_blob.clone(),
            blob: (!item.row.denied && now.blob_exists).then_some(item.row.nfs),
        });
    }
    writes
}

impl Inner {
    /// Every file of the swimlane that is on NFS.
    pub(crate) async fn lane_rows(&self, swimlane: &SwimlaneId) -> Result<Vec<NfsRow>, RegistryError> {
        let rows: Vec<RowTuple> = self
            .db
            .run(
                "sync_lane_rows",
                sqlx::query_as(
                    "SELECT path, class, denied, nfs_hash::bytea, nfs_drift_hash::bytea, last_observation_id FROM file_state \
                     WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL ORDER BY path LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(MAX_ROWS)
                .fetch_all(self.db.pool()),
            )
            .await?;
        if rows.len() >= usize::try_from(MAX_ROWS).unwrap_or(usize::MAX) {
            return Err(RegistryError::Capacity);
        }
        rows.into_iter().filter_map(|t| row_of(t).transpose()).collect()
    }

    /// These files, as they are now.
    pub(crate) async fn rows_of(
        &self,
        swimlane: &SwimlaneId,
        paths: &[String],
    ) -> Result<Vec<NfsRow>, RegistryError> {
        let rows: Vec<RowTuple> = self
            .db
            .run(
                "sync_rows_of",
                sqlx::query_as(
                    "SELECT path, class, denied, nfs_hash::bytea, nfs_drift_hash::bytea, last_observation_id FROM file_state \
                     WHERE swimlane_id = $1 AND path = ANY($2::text[]) AND nfs_hash IS NOT NULL",
                )
                .bind(swimlane.as_str())
                .bind(paths)
                .fetch_all(self.db.pool()),
            )
            .await?;
        rows.into_iter().filter_map(|t| row_of(t).transpose()).collect()
    }

    /// The same, inside the transaction of a chunk.
    pub(crate) async fn rows_in(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        paths: &[String],
    ) -> Result<Vec<NfsRow>, RegistryError> {
        let rows: Vec<RowTuple> = self
            .db
            .run(
                "sync_rows_in",
                sqlx::query_as(
                    "SELECT path, class, denied, nfs_hash::bytea, nfs_drift_hash::bytea, last_observation_id FROM file_state \
                     WHERE swimlane_id = $1 AND path = ANY($2::text[]) AND nfs_hash IS NOT NULL",
                )
                .bind(swimlane.as_str())
                .bind(paths)
                .fetch_all(&mut **tx),
            )
            .await?;
        rows.into_iter().filter_map(|t| row_of(t).transpose()).collect()
    }

    /// The files that have a Git side at the reported commits, with its hash (G), on the blocking pool (rule 4): the mirror reads each
    /// distinct blob once, and the hash of a Git file is cached by the commit's blob.
    pub(crate) async fn resolve(
        &self,
        r: &Report,
        rules: &LaneRules,
        rows: Vec<NfsRow>,
    ) -> Result<Vec<Resolved>, RegistryError> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let (index, rules, report) = (self.git_index.clone(), rules.clone(), r.clone());
        let timeout = self.config.git_timeout.max(Duration::from_secs(60));
        blocking(timeout, move || {
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let Some(target) = target_of(&rules, &report, &row.path) else {
                    continue;
                };
                // `None`: the commit has no such file, so Git has nothing to say about the path.
                let Some(git) = index.git_hash(target.repo, &target.commit, &target.repo_path, row.class)?
                else {
                    continue;
                };
                out.push(Resolved { row, target, git });
            }
            Ok(out)
        })
        .await
    }

    /// Set the baselines the report calls for. Returns the paths whose baseline was written.
    pub(crate) async fn set_baselines(
        &self,
        r: &Report,
        rules: &LaneRules,
    ) -> Result<Vec<domain::NfsPath>, RegistryError> {
        let rows = self.lane_rows(&r.swimlane).await?;
        let equal: Vec<Resolved> = self
            .resolve(r, rules, rows)
            .await?
            .into_iter()
            .filter(|x| x.git == x.row.drift)
            .collect();
        let mut written = Vec::new();
        for chunk in equal.chunks(self.config.chunk_paths) {
            written.extend(self.baseline_chunk(r, chunk).await?);
        }
        Ok(written)
    }

    /// One transaction: the baselines of up to `chunk_paths` files, their `blob_refs` and their audit events.
    async fn baseline_chunk(&self, r: &Report, chunk: &[Resolved]) -> Result<Vec<NfsPath>, RegistryError> {
        let now_ms = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        self.db
            .run(
                "sync_baseline_lock",
                sqlx::query(LOCK_SQL).bind(r.swimlane.as_str()).execute(&mut *tx),
            )
            .await?;
        // Each file as it is now, with its baseline and whether the blob behind it is stored, in one read.
        let paths: Vec<&str> = chunk.iter().map(|x| x.row.path.as_str()).collect();
        let current: Vec<CurrentTuple> = self
            .db
            .run(
                "sync_baseline_current",
                sqlx::query_as(
                    "SELECT x.path, fs.nfs_drift_hash::bytea, b.hash::bytea, b.blob_hash::bytea, \
                            EXISTS (SELECT 1 FROM blobs WHERE hash = fs.nfs_hash) \
                     FROM unnest($2::text[]) AS x(path) \
                     JOIN file_state fs ON fs.swimlane_id = $1 AND fs.path = x.path AND fs.nfs_hash IS NOT NULL \
                     LEFT JOIN baselines b ON b.swimlane_id = $1 AND b.path = x.path",
                )
                .bind(r.swimlane.as_str())
                .bind(&paths)
                .fetch_all(&mut *tx),
            )
            .await?
            .into_iter()
            .collect();
        let current: Vec<Current> = current
            .into_iter()
            .map(|(path, drift, baseline, baseline_blob, blob_exists)| Current {
                path,
                drift,
                baseline,
                baseline_blob,
                blob_exists,
            })
            .collect();

        let writes = decide_writes(chunk, &current);
        if writes.is_empty() {
            return Ok(Vec::new());
        }
        self.upsert(&mut tx, &r.swimlane, &writes, now_ms).await?;
        self.blob_refs(&mut tx, &r.swimlane, &writes).await?;

        let mut audit = Vec::with_capacity(writes.len());
        for w in &writes {
            let mut event = AuditEvent::new(
                Timestamp::from_unix_millis(now_ms),
                r.audit_actor()?,
                AuditAction::BaselineAdopted,
                AuditVia::Sync,
            );
            event.swimlane = Some(r.swimlane.clone());
            event.path = Some(w.item.row.path.clone());
            event.hash_before = w.before.clone().map(hash_of).transpose()?;
            event.hash_after = Some(w.item.git);
            audit.push(event);
        }
        // The audit events last: the chain's lock is held from here to the commit.
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::record_all(&*self.audit, &mut handle, audit).await?;
        self.db.commit(tx).await?;
        Ok(writes.into_iter().map(|w| w.item.row.path.clone()).collect())
    }

    /// The baselines, with the statement the pipeline writes its own with.
    async fn upsert(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        writes: &[Write<'_>],
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        let paths: Vec<&str> = writes.iter().map(|w| w.item.row.path.as_str()).collect();
        let hashes: Vec<Vec<u8>> = writes.iter().map(|w| w.item.git.as_bytes().to_vec()).collect();
        let repos: Vec<&str> = writes
            .iter()
            .map(|w| match w.item.target.repo {
                RepoKind::Base => "base",
                RepoKind::Tenant => "tenant",
            })
            .collect();
        let branches: Vec<Option<&str>> = writes.iter().map(|w| w.item.target.branch.as_deref()).collect();
        let commits: Vec<&str> = writes.iter().map(|w| w.item.target.commit.as_str()).collect();
        let blobs: Vec<Option<Vec<u8>>> = writes
            .iter()
            .map(|w| w.blob.map(|b| b.as_bytes().to_vec()))
            .collect();
        self.db
            .run(
                "sync_baselines",
                sqlx::query(UPSERT_BASELINES_SQL)
                    .bind(swimlane.as_str())
                    .bind(&paths)
                    .bind(&hashes)
                    .bind(&repos)
                    .bind(&branches)
                    .bind(&commits)
                    .bind(&blobs)
                    .bind(now_ms)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }

    /// The reference that keeps the raw version behind each baseline from being collected (D78), and the reference of the version it
    /// replaced dropped. A reference is named by the path it belongs to. The rule is `crate::baselines`', shared with the pipeline.
    async fn blob_refs(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        writes: &[Write<'_>],
    ) -> Result<(), RegistryError> {
        let changes: Vec<RefChange> = writes
            .iter()
            .map(|w| RefChange {
                path: w.item.row.path.as_str().to_owned(),
                old: w.old_blob.clone(),
                new: w.blob.map(|b| b.as_bytes().to_vec()),
            })
            .collect();
        baselines::replace_refs(&self.db, tx, swimlane, &changes).await
    }
}

#[cfg(test)]
mod tests {
    use domain::CommitId;
    use ports::content_hash;

    use super::*;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn item(path: &str, drift: &str, git: &str, denied: bool) -> Resolved {
        Resolved {
            row: NfsRow {
                path: NfsPath::parse(path).unwrap(),
                class: if denied {
                    FileClass::Denied
                } else {
                    FileClass::Structured
                },
                denied,
                nfs: h(&format!("raw-{drift}")),
                drift: h(drift),
                observation: Some(1),
            },
            target: Target {
                repo: RepoKind::Base,
                repo_path: RepoPath::parse(&format!("config/{path}")).unwrap(),
                branch: None,
                commit: CommitId::parse(&"a".repeat(40)).unwrap(),
            },
            git: h(git),
        }
    }

    fn now(path: &str, drift: &str, baseline: Option<&str>, blob_exists: bool) -> Current {
        Current {
            path: path.to_owned(),
            drift: Some(h(drift).as_bytes().to_vec()),
            baseline: baseline.map(|b| h(b).as_bytes().to_vec()),
            baseline_blob: baseline.map(|b| h(&format!("blob-{b}")).as_bytes().to_vec()),
            blob_exists,
        }
    }

    #[test]
    fn a_file_without_a_baseline_or_with_another_one_is_written() {
        let chunk = vec![item("a.yml", "x", "x", false), item("b.yml", "y", "y", false)];
        let current = vec![
            now("a.yml", "x", None, true),
            now("b.yml", "y", Some("old"), true),
        ];
        let writes = decide_writes(&chunk, &current);
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].before, None);
        assert_eq!(writes[1].before, Some(h("old").as_bytes().to_vec()));
        assert_eq!(
            writes[1].old_blob,
            Some(h("blob-old").as_bytes().to_vec()),
            "the reference of the version it replaces is the one to drop"
        );
        assert_eq!(
            writes[0].blob,
            Some(h("raw-x")),
            "the raw version, not the hash for comparing"
        );
    }

    #[test]
    fn a_baseline_that_says_it_already_is_left_alone() {
        let chunk = vec![item("a.yml", "x", "x", false)];
        assert!(decide_writes(&chunk, &[now("a.yml", "x", Some("x"), true)]).is_empty());
    }

    #[test]
    fn a_file_that_has_a_newer_version_since_is_not_the_one_that_equals_git() {
        let chunk = vec![item("a.yml", "x", "x", false)];
        // Ingestion applied a newer version between the read and the lock: N is another hash now.
        assert!(decide_writes(&chunk, &[now("a.yml", "newer", None, true)]).is_empty());
        // The row is gone altogether (the file was removed).
        assert!(decide_writes(&chunk, &[]).is_empty());
        // N is not known (a row from before the column existed).
        let unknown = Current {
            drift: None,
            ..now("a.yml", "x", None, true)
        };
        assert!(decide_writes(&chunk, &[unknown]).is_empty());
    }

    #[test]
    fn a_denied_file_or_a_blob_that_is_not_stored_has_no_blob_behind_its_baseline() {
        let chunk = vec![item("k.pem", "k", "k", true), item("b.yml", "y", "y", false)];
        let current = vec![now("k.pem", "k", None, true), now("b.yml", "y", None, false)];
        let writes = decide_writes(&chunk, &current);
        assert_eq!(writes.len(), 2, "both get a baseline");
        assert_eq!(
            writes[0].blob, None,
            "D79: the content of a denied file is never stored or referenced"
        );
        assert_eq!(
            writes[1].blob, None,
            "a reference to a blob that is not there would break the foreign key"
        );
    }
}
