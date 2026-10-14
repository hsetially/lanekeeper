//! A small world for the pipeline tests: one or more swimlanes over a real Postgres, an in-memory Git, the registry's ingestion and
//! the pipeline, with helpers to change NFS (through ingestion, as an agent would), Git (as a push would), baselines and marks (as
//! adoption would), and to read the tables the pipeline writes.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::{CommitId, ContentHash, DomainEvent, NfsPath, RepoKind, SwimlaneId};
use hub_registry::config::RegistryConfig;
use hub_registry::{Pipeline, PipelineHandle, Work};
use ports::fakes::FakeGit;
use ports::{ReportSink, content_hash};
use sqlx::Row;

use super::counting_git::CountingGit;
use super::rig::{Rig, T, delta, entry, path};

pub struct World {
    pub rig: Rig,
    /// The audit log of the pipeline, apart from ingestion's, so that a test can make one fail and not the other.
    pub pipeline_audit: Arc<super::pg_audit::PgTestAudit>,
    pub git: CountingGit,
    pub pipeline: Pipeline,
    pub handle: PipelineHandle,
    seq: u64,
    root: u64,
    at: i64,
}

fn root_of(n: u64) -> ContentHash {
    content_hash(&n.to_be_bytes())
}

/// What the pipeline writes, as one comparable value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tables {
    /// path to (state, git hash, baseline hash, logical file, kind, role, nfs hash, class)
    #[allow(clippy::type_complexity)]
    pub rows: BTreeMap<
        String,
        (
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        ),
    >,
    /// (kind, fingerprint, path, severity, message), the open ones
    pub findings: BTreeSet<(String, String, Option<String>, String, String)>,
    pub dirs: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
    pub baselines: BTreeMap<String, String>,
    pub marks: BTreeSet<String>,
}

impl World {
    /// A swimlane `sit1` that deploys `tenants`, with no files and no Git. `None` when there is no database.
    pub async fn new(tenants: &[&str]) -> Option<Self> {
        Self::with_config(tenants, RegistryConfig::default()).await
    }

    pub async fn with_config(tenants: &[&str], config: RegistryConfig) -> Option<Self> {
        let rig = Rig::with_event_bus("sit1", config.clone()).await?;
        for t in tenants {
            rig.db.seed_tenant(&rig.pool, "sit1", t).await;
        }
        let git = CountingGit::new(FakeGit::new());
        let pipeline_audit = Arc::new(super::pg_audit::PgTestAudit::default());
        let (pipeline, handle) =
            rig.pipeline_with(Arc::new(git.clone()), config, Arc::clone(&pipeline_audit) as _);
        Some(Self {
            rig,
            pipeline_audit,
            git,
            pipeline,
            handle,
            seq: 0,
            root: 0,
            at: T,
        })
    }

    pub fn lane(&self) -> SwimlaneId {
        self.rig.swimlane.clone()
    }

    // ------------------------------------------------------------------------------------------------ NFS, through ingestion

    /// Say hello and deliver `files` as the first full scan (the swimlane's inventory).
    pub async fn inventory(&mut self, files: &[(&str, &str)]) {
        self.rig.hello().await;
        self.seq = 1;
        self.root = 1;
        self.at = T;
        let entries = files
            .iter()
            .map(|(p, c)| entry(p, c.as_bytes(), self.at))
            .collect();
        self.rig.delta(delta(1, None, root_of(1), entries)).await.unwrap();
    }

    /// Another swimlane (the same Git, its own files): an inventory of `files`.
    pub async fn second_lane(&self, id: &str, tenants: &[&str], files: &[(&str, &str)]) {
        self.rig.add_swimlane(id, tenants, false).await;
        let identity = ports::AgentIdentity::new(SwimlaneId::parse(id).unwrap());
        self.rig
            .ingest
            .hello(&identity, super::rig::hello(id))
            .await
            .unwrap();
        let entries = files.iter().map(|(p, c)| entry(p, c.as_bytes(), T)).collect();
        self.rig
            .ingest
            .delta(&identity, delta(1, None, root_of(1), entries))
            .await
            .unwrap();
    }

    fn next(&mut self) -> (u64, ContentHash, ContentHash, i64) {
        let base = root_of(self.root);
        self.seq += 1;
        self.root += 1;
        self.at += 1_000;
        (self.seq, base, root_of(self.root), self.at)
    }

    /// An increment: these files changed (path, content) on NFS.
    pub async fn nfs_change(&mut self, changes: &[(&str, &str)]) {
        let (seq, base, new, at) = self.next();
        let entries = changes.iter().map(|(p, c)| entry(p, c.as_bytes(), at)).collect();
        self.rig
            .delta(delta(seq, Some(base), new, entries))
            .await
            .unwrap();
    }

    /// An increment made of entries the caller builds, given the time of the scan (a denied file, a file with other flags).
    pub async fn nfs_raw(&mut self, build: impl FnOnce(i64) -> Vec<domain::ScanEntry>) {
        let (seq, base, new, at) = self.next();
        self.rig
            .delta(delta(seq, Some(base), new, build(at)))
            .await
            .unwrap();
    }

    /// An increment: these files were deleted on NFS.
    pub async fn nfs_delete(&mut self, paths: &[&str]) {
        let (seq, base, new, _) = self.next();
        let mut d = delta(seq, Some(base), new, Vec::new());
        d.removed = paths.iter().map(|p| path(p)).collect();
        self.rig.delta(d).await.unwrap();
    }

    // ------------------------------------------------------------------------------------------------ Git

    pub fn base_commit(&self, changes: &[(&str, Option<&str>)]) -> CommitId {
        let owned: Vec<(&str, Option<&[u8]>)> =
            changes.iter().map(|(p, c)| (*p, c.map(str::as_bytes))).collect();
        self.git
            .git()
            .commit(RepoKind::Base, "master", &owned, "base")
            .unwrap()
    }

    pub fn tenant_commit(&self, branch: &str, changes: &[(&str, Option<&str>)]) -> CommitId {
        let owned: Vec<(&str, Option<&[u8]>)> =
            changes.iter().map(|(p, c)| (*p, c.map(str::as_bytes))).collect();
        self.git
            .git()
            .commit(RepoKind::Tenant, branch, &owned, branch)
            .unwrap()
    }

    pub fn head(&self, repo: RepoKind, branch: &str) -> CommitId {
        use ports::GitReader;
        self.git.head(repo, branch).unwrap()
    }

    /// The work the move of a head means.
    pub async fn moved(
        &self,
        repo: RepoKind,
        branch: &str,
        from: Option<&CommitId>,
        to: &CommitId,
    ) -> Vec<(SwimlaneId, Work)> {
        self.pipeline
            .event_work(DomainEvent::GitHeadMoved {
                repo,
                branch: domain::GitRef::parse(branch).unwrap(),
                from: from.cloned(),
                to: to.clone(),
            })
            .await
            .unwrap()
    }

    // ------------------------------------------------------------------------------------------------ decisions

    /// An admin has confirmed the swimlane's baseline.
    pub async fn adopt(&self) {
        sqlx::query(
            "UPDATE swimlanes SET baseline_confirmed_at = now(), editing_unlocked = true WHERE id = $1",
        )
        .bind(self.rig.swimlane.as_str())
        .execute(&self.rig.pool)
        .await
        .unwrap();
    }

    /// A baseline for a path, as adoption would write it (the hash for comparing).
    pub async fn baseline(&self, p: &str, hash: ContentHash) {
        sqlx::query(
            "INSERT INTO baselines (swimlane_id, path, hash, repo, branch, commit_id) \
             VALUES ($1, $2, $3::bytea, 'base', NULL, $4) \
             ON CONFLICT (swimlane_id, path) DO UPDATE SET hash = EXCLUDED.hash",
        )
        .bind(self.rig.swimlane.as_str())
        .bind(p)
        .bind(hash.as_bytes().as_slice())
        .bind("a".repeat(40))
        .execute(&self.rig.pool)
        .await
        .unwrap();
    }

    pub async fn remove_baseline(&self, p: &str) {
        sqlx::query("DELETE FROM baselines WHERE swimlane_id = $1 AND path = $2")
            .bind(self.rig.swimlane.as_str())
            .bind(p)
            .execute(&self.rig.pool)
            .await
            .unwrap();
    }

    /// A mark of an intentional divergence, as a person would make it.
    pub async fn mark(&self, p: &str, nfs: Option<ContentHash>, git: Option<ContentHash>) {
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) \
             VALUES ('t', 'o', 'a@example.com', 'A', 'admin', 'active') ON CONFLICT DO NOTHING",
        )
        .execute(&self.rig.pool)
        .await
        .ok();
        sqlx::query(
            "INSERT INTO intentional_divergence (swimlane_id, path, nfs_hash, git_hash, reason, marked_by_tid, marked_by_oid) \
             VALUES ($1, $2, $3::bytea, $4::bytea, 'test', 't', 'o') \
             ON CONFLICT (swimlane_id, path) DO UPDATE SET nfs_hash = EXCLUDED.nfs_hash, git_hash = EXCLUDED.git_hash",
        )
        .bind(self.rig.swimlane.as_str())
        .bind(p)
        .bind(nfs.map(|h| h.as_bytes().to_vec()))
        .bind(git.map(|h| h.as_bytes().to_vec()))
        .execute(&self.rig.pool)
        .await
        .unwrap();
    }

    pub async fn add_tenant(&self, tenant: &str) {
        self.rig.db.seed_tenant(&self.rig.pool, "sit1", tenant).await;
    }

    // ------------------------------------------------------------------------------------------------ the pipeline

    /// Bring the swimlane up to date for these paths now.
    pub async fn paths(&self, paths: &[&str]) -> hub_registry::BatchReport {
        let set: BTreeSet<NfsPath> = paths.iter().map(|p| path(p)).collect();
        self.pipeline
            .process(vec![(self.lane(), Work::Paths(set))])
            .await
            .unwrap()
    }

    pub async fn whole(&self) -> hub_registry::BatchReport {
        self.pipeline
            .process(vec![(self.lane(), Work::Whole)])
            .await
            .unwrap()
    }

    // ------------------------------------------------------------------------------------------------ the tables

    pub async fn tables(&self) -> Tables {
        let pool = &self.rig.pool;
        let lane = self.rig.swimlane.as_str();
        let hex = |b: Option<Vec<u8>>| b.map(|b| ContentHash::from_bytes(b.try_into().unwrap()).to_string());
        let rows = sqlx::query(
            "SELECT path, state, git_hash::bytea AS git, baseline_hash::bytea AS base, logical_file, kind, role, \
                    nfs_hash::bytea AS nfs, class FROM file_state WHERE swimlane_id = $1 ORDER BY path LIMIT 100000",
        )
        .bind(lane)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("path"),
                (
                    r.get::<String, _>("state"),
                    hex(r.get("git")),
                    hex(r.get("base")),
                    r.get::<String, _>("logical_file"),
                    r.get::<String, _>("kind"),
                    r.get::<Option<String>, _>("role"),
                    hex(r.get("nfs")),
                    r.get::<String, _>("class"),
                ),
            )
        })
        .collect();
        let findings = sqlx::query(
            "SELECT kind, fingerprint, path, severity, message FROM findings \
             WHERE swimlane_id = $1 AND resolved_at IS NULL ORDER BY kind, fingerprint LIMIT 100000",
        )
        .bind(lane)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.get("kind"),
                r.get("fingerprint"),
                r.get("path"),
                r.get("severity"),
                r.get("message"),
            )
        })
        .collect();
        let pairs = |sql: &'static str| {
            let lane = lane.to_owned();
            let pool = pool.clone();
            async move {
                sqlx::query(sql)
                    .bind(lane)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|r| {
                        (
                            r.get::<String, _>(0),
                            ContentHash::from_bytes(r.get::<Vec<u8>, _>(1).try_into().unwrap()).to_string(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            }
        };
        Tables {
            rows,
            findings,
            dirs: pairs(
                "SELECT dir_path, hash::bytea FROM effective_tree_hashes WHERE swimlane_id = $1 LIMIT 100000",
            )
            .await,
            files: pairs(
                "SELECT logical_file, hash::bytea FROM effective_files WHERE swimlane_id = $1 LIMIT 100000",
            )
            .await,
            baselines: pairs("SELECT path, hash::bytea FROM baselines WHERE swimlane_id = $1 LIMIT 100000")
                .await,
            marks: sqlx::query_scalar::<_, String>(
                "SELECT path FROM intentional_divergence WHERE swimlane_id = $1 LIMIT 100000",
            )
            .bind(lane)
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .collect(),
        }
    }

    /// The paths of every `DriftChanged` in the outbox, in order.
    pub async fn drift_events(&self) -> Vec<Vec<String>> {
        self.rig
            .bus
            .published()
            .await
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::DriftChanged { paths, .. } => {
                    Some(paths.iter().map(|p| p.as_str().to_owned()).collect())
                }
                _ => None,
            })
            .collect()
    }

    pub async fn findings_changed_events(&self) -> usize {
        self.rig
            .bus
            .published()
            .await
            .into_iter()
            .filter(|e| matches!(e, DomainEvent::FindingsChanged { .. }))
            .count()
    }

    pub async fn registry_row(&self) -> (i64, serde_json::Value, i32, bool) {
        let r = sqlx::query(
            "SELECT version, state_counts, finding_count, rules_fingerprint IS NOT NULL AS fp FROM swimlane_registry WHERE swimlane_id = $1",
        )
        .bind(self.rig.swimlane.as_str())
        .fetch_one(&self.rig.pool)
        .await
        .unwrap();
        (
            r.get("version"),
            r.get("state_counts"),
            r.get("finding_count"),
            r.get("fp"),
        )
    }
}

/// The state of a path as a test reads it: the drift state's wire name.
pub async fn state_of(world: &World, p: &str) -> Option<String> {
    world.rig.state(p).await.map(|r| r.state)
}

// ---------------------------------------------------------------------------------------------------------------- the job

use std::future::Future;
use std::time::Duration;

use hub_platform::jobs::{JobContext, JobError};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// The pipeline's job running in a task with a lease that the test can take away.
pub struct RunningJob {
    task: JoinHandle<Result<(), JobError>>,
    lease: watch::Sender<Instant>,
}

impl RunningJob {
    /// Start one run of the job of `pipeline`.
    pub fn start(pipeline: &Pipeline) -> Self {
        let (ctx, lease): (JobContext, _) =
            JobContext::held_until_told_otherwise(hub_registry::pipeline::PIPELINE_JOB);
        let job = pipeline.job();
        let task = tokio::spawn(async move { job.run(&ctx).await });
        Self { task, lease }
    }

    /// Take the lease away and wait for the run to end: `LeaseLost`.
    pub async fn lose_lease(self) -> Result<(), JobError> {
        self.lease.send_replace(Instant::now() - Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("the run ends once its lease is gone")
            .expect("the run does not panic")
    }

    /// Wait for the run to end on its own.
    pub async fn finished(self, within: Duration) -> Result<(), JobError> {
        // `self.lease` is dropped at the end of this function, after the run: until then the run holds its lease.
        tokio::time::timeout(within, self.task)
            .await
            .expect("the run ends by itself")
            .expect("the run does not panic")
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Wait until `check` says yes, polling every 20 ms for at most `timeout`.
pub async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = Instant::now() + timeout;
    loop {
        if check().await {
            return;
        }
        assert!(
            Instant::now() < end,
            "timed out after {timeout:?} waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

impl World {
    /// Whether a `DriftChanged` that names `path` is in the outbox.
    pub async fn drift_named(&self, path: &str) -> bool {
        self.drift_events().await.iter().flatten().any(|p| p == path)
    }

    /// Whether a `DriftChanged` that names `path` was published after the first `seen` of them.
    pub async fn drift_named_after(&self, seen: usize, path: &str) -> bool {
        self.drift_events()
            .await
            .iter()
            .skip(seen)
            .flatten()
            .any(|p| p == path)
    }
}
