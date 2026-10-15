//! `LaneLoader` (T3, plan A21): the engine's input for a swimlane, built from the database and from Git.
//!
//! For each path the engine needs what is on NFS (the hash of the content, and the content itself only when the engine will read it),
//! what is in Git (the hash for drift), the baseline, the intentional-divergence mark and the change a sync made last. They come from
//! `file_state` (the NFS columns ingestion writes), `baselines`, `intentional_divergence`, the swimlane's last observation and the
//! [`GitView`] of the batch. Which path is which kind of file comes from the engine's own `classify_path`, never from the stored
//! `kind` and `logical_file` columns: those are derived, and what the rules say now is what counts.
//!
//! **The universe of a swimlane** is every path with a file on NFS and every path Git holds for it (the forward-mapped trees of the base
//! default branch and of its tenants' branches): a file Git has and NFS lacks is "new in Git" drift. A path that is on neither is not
//! part of the swimlane, and its row (it has one when a file was deleted) is deleted with it.
//!
//! **Content is read only where the engine reads it** ([`files_to_read`]): the logical files that an update names (before and after),
//! the root `application.*` files, and, when one of those changed, every property source. The engine's own documentation says the
//! rest is not read, and the test `incremental_never_needs_the_bytes_of_clean_files` holds it to that; a clean file carries the right
//! hash and empty bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;

use bytes::Bytes;
use domain::{
    AttributionSource, ContentHash, DriftState, FileClass, FileRole, LogicalFile, NfsPath, RepoKind,
    ReverseMapping, SwimlaneId,
};
use engine::{
    Baseline, DriftInput, IntentionalMark, LaneFile, NfsContent, Overwrite, classify, classify_path,
    drift_state, logical_file_role,
};
use ports::{BlobStore, MAX_GET_MANY};

use super::git_side::GitView;
use super::lane_store::{LaneEntry, OpaqueFile, Persisted};
use super::persist::BaselineWrite;
use super::rules::LaneRules;
use crate::db::Db;
use crate::error::RegistryError;

/// The most rows read for one swimlane. The engine refuses more files than its own bound anyway.
const MAX_ROWS: i64 = 200_000 + 1;

/// The statements, as constants so that the EXPLAIN snapshot test takes exactly what runs (P12).
///
/// The observation a row points at is joined with `LATERAL ... LIMIT 1`, so that it is one primary key probe per row of the swimlane and
/// never a scan of the history: `file_observations` keeps every version of every file, and a planner that sees 2,000 rows to match
/// against a table of millions may still prefer to hash all of it (without the `LIMIT` it flattens the subquery into an ordinary join
/// and does).
pub const ROWS_OF_LANE_SQL: &str = "\
SELECT fs.path, fs.class, fs.denied, fs.nfs_hash::bytea, fs.nfs_drift_hash::bytea, fs.state, fs.git_hash::bytea, \
       fs.baseline_hash::bytea, fs.logical_file, fs.kind, fs.role, o.previous_state, o.previous_hash::bytea, o.attribution_source, \
       fs.severity \
FROM file_state fs \
LEFT JOIN LATERAL (SELECT previous_state, previous_hash, attribution_source FROM file_observations \
                   WHERE id = fs.last_observation_id AND previous_state IS NOT NULL LIMIT 1) o ON true \
WHERE fs.swimlane_id = $1 ORDER BY fs.path LIMIT $2";

pub const ROWS_OF_PATHS_SQL: &str = "\
SELECT p.sl, fs.path, fs.class, fs.denied, fs.nfs_hash::bytea, fs.nfs_drift_hash::bytea, fs.state, fs.git_hash::bytea, \
       fs.baseline_hash::bytea, fs.logical_file, fs.kind, fs.role, o.previous_state, o.previous_hash::bytea, o.attribution_source, \
       fs.severity \
FROM unnest($1::text[], $2::text[]) AS p(sl, path) \
JOIN file_state fs ON fs.swimlane_id = p.sl AND fs.path = p.path \
LEFT JOIN LATERAL (SELECT previous_state, previous_hash, attribution_source FROM file_observations \
                   WHERE id = fs.last_observation_id AND previous_state IS NOT NULL LIMIT 1) o ON true \
LIMIT $3";

pub const BASELINES_OF_LANE_SQL: &str =
    "SELECT path, hash::bytea FROM baselines WHERE swimlane_id = $1 ORDER BY path LIMIT $2";
pub const BASELINES_OF_PATHS_SQL: &str = "\
SELECT p.sl, b.path, b.hash::bytea FROM unnest($1::text[], $2::text[]) AS p(sl, path) \
JOIN baselines b ON b.swimlane_id = p.sl AND b.path = p.path LIMIT $3";

pub const MARKS_OF_LANE_SQL: &str = "\
SELECT path, nfs_hash::bytea, git_hash::bytea FROM intentional_divergence WHERE swimlane_id = $1 ORDER BY path LIMIT $2";
pub const MARKS_OF_PATHS_SQL: &str = "\
SELECT p.sl, d.path, d.nfs_hash::bytea, d.git_hash::bytea FROM unnest($1::text[], $2::text[]) AS p(sl, path) \
JOIN intentional_divergence d ON d.swimlane_id = p.sl AND d.path = p.path LIMIT $3";

/// A `file_state` row as the pipeline reads it.
#[derive(Debug, Clone)]
pub(crate) struct StateRow {
    pub class: FileClass,
    /// The raw hash of the file on NFS, or `None` when it is not there.
    pub nfs: Option<ContentHash>,
    /// The hash for comparing that ingestion computed from the content, or the raw hash when the registry has no content.
    pub nfs_drift: Option<ContentHash>,
    pub persisted: Persisted,
    pub overwrite: Option<Overwrite>,
}

/// What the database holds for some paths of one swimlane.
#[derive(Debug, Clone, Default)]
pub(crate) struct Inputs {
    pub rows: BTreeMap<NfsPath, StateRow>,
    pub baselines: BTreeMap<NfsPath, ContentHash>,
    pub marks: BTreeMap<NfsPath, IntentionalMark>,
}

type RowTuple = (
    String,
    String,
    bool,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
);

/// A row of `ROWS_OF_PATHS_SQL`: the swimlane, then a [`RowTuple`].
type PathRowTuple = (
    String,
    String,
    String,
    bool,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
);

/// A row of `MARKS_OF_LANE_SQL`: path, NFS hash, Git hash.
type MarkRow = (String, Option<Vec<u8>>, Option<Vec<u8>>);

/// A row of `MARKS_OF_PATHS_SQL`: swimlane, path, NFS hash, Git hash.
type PathMarkRow = (String, String, Option<Vec<u8>>, Option<Vec<u8>>);

fn hash(bytes: Vec<u8>) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

fn opt_hash(bytes: Option<Vec<u8>>) -> Result<Option<ContentHash>, RegistryError> {
    bytes.map(hash).transpose()
}

fn path(text: &str) -> Result<NfsPath, RegistryError> {
    NfsPath::parse(text).map_err(|_| RegistryError::Corrupt)
}

fn row_of(t: RowTuple) -> Result<(NfsPath, StateRow), RegistryError> {
    let (
        p,
        class,
        denied,
        nfs,
        nfs_drift,
        state,
        git,
        baseline,
        logical,
        kind,
        role,
        prev_state,
        prev_hash,
        source,
        severity,
    ) = t;
    let class = FileClass::from_str(&class).map_err(|_| RegistryError::Corrupt)?;
    // The denied flag and the class say the same thing; a row that disagrees is read as denied, the safe side.
    let class = if denied { FileClass::Denied } else { class };
    let overwrite = match prev_state {
        Some(prev) => Some(Overwrite {
            previous_state: DriftState::from_str(&prev).map_err(|_| RegistryError::Corrupt)?,
            previous_nfs: opt_hash(prev_hash)?,
            source: source
                .as_deref()
                .map(AttributionSource::from_str)
                .transpose()
                .map_err(|_| RegistryError::Corrupt)?
                .unwrap_or(AttributionSource::Unknown),
        }),
        None => None,
    };
    Ok((
        path(&p)?,
        StateRow {
            class,
            nfs: opt_hash(nfs)?,
            nfs_drift: opt_hash(nfs_drift)?,
            persisted: Persisted {
                state,
                git: opt_hash(git)?,
                baseline: opt_hash(baseline)?,
                logical,
                kind,
                role,
                severity,
            },
            overwrite,
        },
    ))
}

/// Reads what the engine's input is made of.
#[derive(Clone)]
pub(crate) struct LaneLoader {
    db: Db,
    blobs: Arc<dyn BlobStore>,
    blob_timeout: std::time::Duration,
}

impl std::fmt::Debug for LaneLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaneLoader").finish_non_exhaustive()
    }
}

impl LaneLoader {
    pub(crate) fn new(db: Db, blobs: Arc<dyn BlobStore>, blob_timeout: std::time::Duration) -> Self {
        Self {
            db,
            blobs,
            blob_timeout,
        }
    }

    /// Everything the database holds for one swimlane.
    pub(crate) async fn load_lane(&self, swimlane: &SwimlaneId) -> Result<Inputs, RegistryError> {
        let rows: Vec<RowTuple> = self
            .db
            .run(
                "loader_rows",
                sqlx::query_as(ROWS_OF_LANE_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_ROWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let baselines: Vec<(String, Vec<u8>)> = self
            .db
            .run(
                "loader_baselines",
                sqlx::query_as(BASELINES_OF_LANE_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_ROWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let marks: Vec<MarkRow> = self
            .db
            .run(
                "loader_marks",
                sqlx::query_as(MARKS_OF_LANE_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_ROWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut out = Inputs::default();
        for r in rows {
            let (p, row) = row_of(r)?;
            out.rows.insert(p, row);
        }
        for (p, h) in baselines {
            out.baselines.insert(path(&p)?, hash(h)?);
        }
        for (p, nfs, git) in marks {
            out.marks.insert(
                path(&p)?,
                IntentionalMark {
                    nfs: opt_hash(nfs)?,
                    git: opt_hash(git)?,
                },
            );
        }
        Ok(out)
    }

    /// What the database holds for these paths, which may be of several swimlanes: three statements whatever their number.
    pub(crate) async fn load_paths(
        &self,
        pairs: &[(SwimlaneId, NfsPath)],
    ) -> Result<BTreeMap<SwimlaneId, Inputs>, RegistryError> {
        let mut out: BTreeMap<SwimlaneId, Inputs> = BTreeMap::new();
        if pairs.is_empty() {
            return Ok(out);
        }
        let lanes: Vec<&str> = pairs.iter().map(|(l, _)| l.as_str()).collect();
        let paths: Vec<&str> = pairs.iter().map(|(_, p)| p.as_str()).collect();
        let limit = i64::try_from(pairs.len()).unwrap_or(i64::MAX);
        let rows: Vec<PathRowTuple> = self
            .db
            .run(
                "loader_path_rows",
                sqlx::query_as(ROWS_OF_PATHS_SQL)
                    .bind(&lanes)
                    .bind(&paths)
                    .bind(limit)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let baselines: Vec<(String, String, Vec<u8>)> = self
            .db
            .run(
                "loader_path_baselines",
                sqlx::query_as(BASELINES_OF_PATHS_SQL)
                    .bind(&lanes)
                    .bind(&paths)
                    .bind(limit)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let marks: Vec<PathMarkRow> = self
            .db
            .run(
                "loader_path_marks",
                sqlx::query_as(MARKS_OF_PATHS_SQL)
                    .bind(&lanes)
                    .bind(&paths)
                    .bind(limit)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let lane_of = |id: &str| SwimlaneId::parse(id).map_err(|_| RegistryError::Corrupt);
        for r in rows {
            let (lane, rest) = (r.0.clone(), r);
            let tuple: RowTuple = (
                rest.1, rest.2, rest.3, rest.4, rest.5, rest.6, rest.7, rest.8, rest.9, rest.10, rest.11,
                rest.12, rest.13, rest.14, rest.15,
            );
            let (p, row) = row_of(tuple)?;
            out.entry(lane_of(&lane)?).or_default().rows.insert(p, row);
        }
        for (lane, p, h) in baselines {
            out.entry(lane_of(&lane)?)
                .or_default()
                .baselines
                .insert(path(&p)?, hash(h)?);
        }
        for (lane, p, nfs, git) in marks {
            out.entry(lane_of(&lane)?).or_default().marks.insert(
                path(&p)?,
                IntentionalMark {
                    nfs: opt_hash(nfs)?,
                    git: opt_hash(git)?,
                },
            );
        }
        Ok(out)
    }

    /// The content of these blobs, in batches of the store's limit, each batch under the blob timeout. A blob the store does not
    /// have is not in the answer.
    pub(crate) async fn fetch_bytes(
        &self,
        hashes: &BTreeSet<ContentHash>,
    ) -> Result<HashMap<ContentHash, Bytes>, RegistryError> {
        let mut out = HashMap::with_capacity(hashes.len());
        let all: Vec<ContentHash> = hashes.iter().copied().collect();
        for batch in all.chunks(MAX_GET_MANY) {
            let got = tokio::time::timeout(self.blob_timeout, self.blobs.get_many(batch))
                .await
                .map_err(|_| RegistryError::Timeout)??;
            out.extend(got);
        }
        Ok(out)
    }
}

/// What a build decided besides the file, from the drift of the inputs it was given (D19, D20, D2):
///
/// - **a baseline**, when NFS and Git agree (or moved to the same content) and an admin has adopted the swimlane: B = N. Before
///   adoption the baselines are the adoption pass's, set with a person looking at each difference, and a baseline written behind its
///   back would turn "unknown" into "in sync" for files nobody reviewed;
/// - **a mark to clear**, when the file's N or G is no longer what it was when the mark was made.
///
/// The decision is taken *before* the engine runs and the file given to the engine already has it: its baseline is N, its stale mark
/// is gone. So the engine is called once, its state is what a full recompute of the tables would give (which read the baseline this
/// decision writes), and no instruction of its answer is left over. N for the decision is the hash ingestion stored for comparing,
/// which is what the engine computes from the content (the same function of the same bytes).
#[derive(Debug, Default, Clone)]
pub(crate) struct Decision {
    pub baseline: Option<BaselineWrite>,
    pub clear_mark: bool,
}

/// The decisions of a batch for one swimlane, in path order of the builds.
#[derive(Debug, Default)]
pub(crate) struct Decisions {
    pub baselines: Vec<BaselineWrite>,
    pub mark_clears: Vec<NfsPath>,
}

impl Decisions {
    pub(crate) fn add(&mut self, path: &NfsPath, decision: Decision) {
        self.baselines.extend(decision.baseline);
        if decision.clear_mark {
            self.mark_clears.push(path.clone());
        }
    }
}

/// What `build_path` made of a path.
#[derive(Debug)]
pub(crate) enum Built {
    /// A file the engine computes.
    Engine(Box<LaneFile>),
    /// A file computed from its stored hashes alone (denied, or without content).
    Opaque(Box<OpaqueFile>),
    /// Not part of the swimlane: not on NFS, not in Git (or not a path any mapping can place).
    Absent,
}

/// What is needed to turn a path into a [`Built`].
pub(crate) struct BuildCtx<'a> {
    pub lane: &'a LaneRules,
    pub view: &'a GitView,
}

/// The engine's view of one path, and what was decided with it. `force_opaque` is for a file whose content the store did not have: it
/// is computed from the hashes alone, like a denied file, until it is read again.
pub(crate) fn build_path(
    ctx: &BuildCtx<'_>,
    path: &NfsPath,
    row: Option<&StateRow>,
    baseline: Option<ContentHash>,
    mark: Option<IntentionalMark>,
    force_opaque: bool,
) -> Result<(Built, Decision), RegistryError> {
    let global = &ctx.lane.global;
    let lane_tenants = &ctx.lane.tenants;
    let facts = classify_path(
        &global.rule,
        path,
        ctx.view.deployable(),
        |m: &ReverseMapping| match m {
            ReverseMapping::Base { repo_path, .. } => ctx.view.is_tracked(None, repo_path, lane_tenants),
            ReverseMapping::Tenant {
                tenant, repo_path, ..
            } => ctx.view.is_tracked(Some(tenant), repo_path, lane_tenants),
        },
    );
    // A path no mapping can place (it would not fit under a repository root) is not a file of any repository: it is left out, the
    // same way every time, so that an update and a full load agree.
    let Ok(facts) = facts else {
        return Ok((Built::Absent, Decision::default()));
    };
    let class = row.map_or_else(|| classify(path, None, &global.deny), |r| r.class);
    let git = ctx
        .view
        .hash(facts.tenant.as_ref(), &facts.repo_path, class, lane_tenants)?;
    let nfs_hash = row.and_then(|r| r.nfs);
    if nfs_hash.is_none() && git.is_none() {
        return Ok((Built::Absent, Decision::default()));
    }
    let opaque = class == FileClass::Denied || force_opaque;
    // N for the drift table: the hash ingestion stored for comparing (the raw hash when the registry has no content).
    let nfs = row.and_then(|r| r.nfs.and(r.nfs_drift.or(r.nfs)));
    let mut baseline = baseline.map_or(Baseline::NotRecorded, |h| Baseline::Recorded(Some(h)));
    let mut mark = mark;
    let mut decision = Decision::default();
    if row.is_some() {
        let drift = drift_state(&DriftInput {
            baseline,
            nfs,
            git,
            intentional: mark,
        });
        if drift.mark_stale {
            mark = None;
            decision.clear_mark = true;
        }
        if drift.set_baseline_to_nfs && ctx.lane.adopted {
            if let (Some(n), Some((repo, commit))) =
                (nfs, ctx.view.source_of(facts.tenant.as_ref(), lane_tenants))
            {
                baseline = Baseline::Recorded(Some(n));
                decision.baseline = Some(BaselineWrite {
                    path: path.clone(),
                    hash: n,
                    repo,
                    branch: match repo {
                        RepoKind::Base => None,
                        RepoKind::Tenant => facts.tenant.as_ref().map(|t| t.as_str().to_owned()),
                    },
                    commit,
                    // The raw version behind the baseline; a denied file's content is never stored.
                    blob: if opaque { None } else { nfs_hash },
                });
            }
        }
    }
    let overwrite = row.and_then(|r| r.overwrite);

    if opaque {
        let drift = drift_state(&DriftInput {
            baseline,
            nfs,
            git,
            intentional: mark,
        });
        return Ok((
            Built::Opaque(Box::new(OpaqueFile {
                path: path.clone(),
                facts,
                class,
                nfs,
                git,
                baseline,
                intentional: mark,
                drift,
            })),
            decision,
        ));
    }
    Ok((
        Built::Engine(Box::new(LaneFile {
            path: path.clone(),
            facts,
            class,
            nfs: nfs_hash.map(|blob| NfsContent {
                blob,
                bytes: Bytes::new(),
            }),
            git,
            baseline,
            intentional: mark,
            overwrite,
            history: None,
        })),
        decision,
    ))
}

/// Put a built path into an entry.
pub(crate) fn place(entry: &mut LaneEntry, path: &NfsPath, built: Built) {
    match built {
        Built::Engine(file) => entry.put_file(*file),
        Built::Opaque(file) => entry.put_opaque(*file),
        Built::Absent => entry.remove(path),
    }
}

/// The root `application.*` files: a layer of every property source.
const ROOT_PREFIX: &str = "application.";

fn is_root_file(path: &NfsPath) -> bool {
    path.parent().is_root() && path.file_name().starts_with(ROOT_PREFIX)
}

/// The files of `entry` whose content the engine reads for an update, as indices into `entry.files`:
///
/// - every file of a logical file that an update names (`logicals`: the logical files of the changed paths, before and after);
/// - the root `application.*` files, always (they are a layer of every property source);
/// - when a changed path is a root file, every file of every property source, since the root is a layer of all of them.
///
/// A superset of what the engine reads is harmless (it is only read from the store); a subset would give the engine empty content
/// to parse, so this errs on the wide side.
pub(crate) fn files_to_read(
    entry: &LaneEntry,
    logicals: &BTreeSet<LogicalFile>,
    changed: &BTreeSet<NfsPath>,
) -> Vec<usize> {
    let root_changed = changed.iter().any(is_root_file);
    entry
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            if f.nfs.is_none() {
                return false;
            }
            logicals.contains(&f.facts.logical)
                || is_root_file(&f.path)
                || (root_changed
                    && logical_file_role(&f.facts.logical, &entry.apps) == FileRole::PropertySource)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Give the files at `indices` their content. Returns the paths whose blob the store did not have.
pub(crate) fn fill(
    files: &mut [LaneFile],
    indices: &[usize],
    bytes: &HashMap<ContentHash, Bytes>,
) -> Vec<NfsPath> {
    let mut missing = Vec::new();
    for &i in indices {
        let Some(file) = files.get_mut(i) else {
            continue;
        };
        let Some(content) = &mut file.nfs else {
            continue;
        };
        match bytes.get(&content.blob) {
            Some(b) => content.bytes = b.clone(),
            None => missing.push(file.path.clone()),
        }
    }
    missing
}

/// Drop the content again: at rest an entry holds hashes only.
pub(crate) fn clear(files: &mut [LaneFile]) {
    for file in files {
        if let Some(content) = &mut file.nfs {
            if !content.bytes.is_empty() {
                content.bytes = Bytes::new();
            }
        }
    }
}

/// The blobs a set of files needs, for one fetch.
pub(crate) fn blobs_of(files: &[LaneFile], indices: &[usize]) -> BTreeSet<ContentHash> {
    indices
        .iter()
        .filter_map(|&i| files.get(i))
        .filter_map(|f| f.nfs.as_ref().map(|c| c.blob))
        .collect()
}

/// Every file that has content on NFS: what a full load reads.
pub(crate) fn all_with_content(files: &[LaneFile]) -> Vec<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.nfs.is_some())
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegistryConfig;
    use crate::index::{GitIndex, GitIndexDeps};
    use crate::metrics::RegistryMetrics;
    use crate::pipeline::git_side::{LiveHeads, live_heads};
    use crate::pipeline::lane_store::{LaneEntry, LaneStore};
    use crate::pipeline::rules::{GlobalRules, lane_fingerprint};
    use domain::{FileKind, PathMappingRule, RepoKind, TenantId, TenantSet};
    use engine::HostRules;
    use ports::fakes::FakeGit;
    use sqlx::postgres::PgPoolOptions;

    fn tenant(n: &str) -> TenantId {
        TenantId::parse(n).unwrap()
    }

    fn tenants(names: &[&str]) -> TenantSet {
        TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
    }

    fn p(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    fn h(b: u8) -> ContentHash {
        ContentHash::from_bytes([b; 32])
    }

    fn git() -> FakeGit {
        let git = FakeGit::new();
        git.commit(
            RepoKind::Base,
            "master",
            &[
                ("config/app/core.yml", Some(b"a: 1\n")),
                ("config/app/only-in-git.yml", Some(b"g: 1\n")),
                ("config/server.key", Some(b"KEY")),
            ],
            "base",
        )
        .unwrap();
        git.commit(
            RepoKind::Tenant,
            "sit1",
            &[("data/config/app/core.yml", Some(b"a: 2\n"))],
            "sit1",
        )
        .unwrap();
        git.commit(
            RepoKind::Tenant,
            "sit2",
            &[("data/config/app/core.yml", Some(b"a: 3\n"))],
            "sit2",
        )
        .unwrap();
        git
    }

    struct Fixture {
        lane: LaneRules,
        view: GitView,
    }

    fn fixture(lane_tenants: &[&str]) -> Fixture {
        let git = git();
        let live: LiveHeads = live_heads(&git).unwrap();
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        let index = GitIndex::new(GitIndexDeps {
            pool,
            git: Arc::new(git.clone()),
            config: RegistryConfig::default(),
            metrics: RegistryMetrics::default(),
        })
        .unwrap();
        let needed = lane_tenants.iter().map(|t| tenant(t)).collect();
        let view = GitView::resolve(&index, &git, &live, &needed).unwrap();
        let global = Arc::new(GlobalRules::for_tests(PathMappingRule::default()));
        let tenants = tenants(lane_tenants);
        let lane = LaneRules {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            fingerprint: lane_fingerprint(&global, &tenants, &[], None, false),
            global,
            tenants,
            apps: Vec::new(),
            tier: None,
            adopted: false,
        };
        Fixture { lane, view }
    }

    fn row(class: FileClass, nfs: Option<ContentHash>) -> StateRow {
        StateRow {
            class,
            nfs,
            nfs_drift: nfs,
            persisted: Persisted {
                state: "unknown".to_owned(),
                git: None,
                baseline: None,
                logical: String::new(),
                kind: "base".to_owned(),
                role: None,
                severity: None,
            },
            overwrite: None,
        }
    }

    /// The file a build made, without what it decided.
    fn make(
        ctx: &BuildCtx<'_>,
        path: &NfsPath,
        row: Option<&StateRow>,
        baseline: Option<ContentHash>,
        mark: Option<IntentionalMark>,
        force: bool,
    ) -> Built {
        build_path(ctx, path, row, baseline, mark, force).unwrap().0
    }

    fn decided(
        f: &Fixture,
        path: &str,
        row: &StateRow,
        baseline: Option<ContentHash>,
        mark: Option<IntentionalMark>,
    ) -> (Built, Decision) {
        build_path(&ctx(f), &p(path), Some(row), baseline, mark, false).unwrap()
    }

    fn ctx(f: &Fixture) -> BuildCtx<'_> {
        BuildCtx {
            lane: &f.lane,
            view: &f.view,
        }
    }

    #[tokio::test]
    async fn a_file_on_nfs_that_git_has_is_an_engine_file_with_its_git_hash_and_no_content() {
        let f = fixture(&["sit1"]);
        let built = make(
            &ctx(&f),
            &p("app/core.yml"),
            Some(&row(FileClass::Structured, Some(h(1)))),
            None,
            None,
            false,
        );
        let Built::Engine(file) = built else {
            panic!("an engine file");
        };
        assert_eq!(file.facts.kind, FileKind::Base);
        assert_eq!(
            file.nfs.as_ref().map(|c| (c.blob, c.bytes.len())),
            Some((h(1), 0))
        );
        assert_eq!(
            file.git,
            engine::drift_hash(Some(&Bytes::from_static(b"a: 1\n")), FileClass::Structured)
        );
        assert_eq!(file.baseline, Baseline::NotRecorded);
    }

    #[tokio::test]
    async fn a_tenant_file_of_a_deployed_tenant_is_tracked_and_of_another_tenant_is_not() {
        let f = fixture(&["sit1"]);
        let mine = make(
            &ctx(&f),
            &p("app/core-sit1.yml"),
            Some(&row(FileClass::Structured, Some(h(1)))),
            None,
            None,
            false,
        );
        let Built::Engine(mine) = mine else {
            panic!("engine")
        };
        assert_eq!(mine.facts.kind, FileKind::Tenant);
        assert!(mine.git.is_some());
        // sit2 is deployable but not deployed here: its suffix is still a suffix, and Git is not asked.
        let other = make(
            &ctx(&f),
            &p("app/core-sit2.yml"),
            Some(&row(FileClass::Structured, Some(h(2)))),
            None,
            None,
            false,
        );
        let Built::Engine(other) = other else {
            panic!("engine")
        };
        assert_eq!(other.facts.kind, FileKind::Untracked);
        assert_eq!(other.facts.tenant.as_ref().map(TenantId::as_str), Some("sit2"));
        assert_eq!(other.git, None);
    }

    #[tokio::test]
    async fn a_file_only_git_has_is_in_the_swimlane_with_no_content() {
        let f = fixture(&["sit1"]);
        let built = make(&ctx(&f), &p("app/only-in-git.yml"), None, None, None, false);
        let Built::Engine(file) = built else {
            panic!("engine")
        };
        assert!(file.nfs.is_none() && file.git.is_some());
        assert_eq!(
            file.class,
            FileClass::Structured,
            "the class of a file nobody has read is its name's"
        );
    }

    #[tokio::test]
    async fn a_path_on_neither_side_is_not_part_of_the_swimlane() {
        let f = fixture(&["sit1"]);
        // A removed file (a row with no hash) that Git does not have either.
        let built = make(
            &ctx(&f),
            &p("app/gone.yml"),
            Some(&row(FileClass::Structured, None)),
            Some(h(9)),
            None,
            false,
        );
        assert!(
            matches!(built, Built::Absent),
            "its baseline row is a decision and stays; the path is not a file any more"
        );
        assert!(matches!(
            make(&ctx(&f), &p("app/never.yml"), None, None, None, false),
            Built::Absent
        ));
    }

    #[tokio::test]
    async fn a_denied_file_is_computed_from_its_hashes_and_its_git_content_is_not_read() {
        let f = fixture(&["sit1"]);
        let mut r = row(FileClass::Denied, Some(h(7)));
        r.nfs_drift = Some(h(7));
        let built = make(&ctx(&f), &p("server.key"), Some(&r), None, None, false);
        let Built::Opaque(file) = built else {
            panic!("opaque")
        };
        assert_eq!(file.nfs, Some(h(7)));
        // Git has the file: its hash for drift is the index's raw SHA-256 of "KEY".
        assert_eq!(file.git, Some(ports::content_hash(b"KEY")));
        assert_eq!(
            file.drift.state,
            DriftState::Unknown,
            "different hashes, no baseline"
        );
        // And with the same content on both sides it is in sync.
        let same = StateRow {
            nfs: Some(ports::content_hash(b"KEY")),
            nfs_drift: Some(ports::content_hash(b"KEY")),
            ..r
        };
        let Built::Opaque(same) = make(&ctx(&f), &p("server.key"), Some(&same), None, None, false) else {
            panic!("opaque")
        };
        assert_eq!(same.drift.state, DriftState::InSync);
    }

    #[tokio::test]
    async fn the_denied_flag_wins_over_the_class_and_forcing_makes_any_file_opaque() {
        let f = fixture(&["sit1"]);
        let mut r = row(FileClass::Denied, Some(h(7)));
        r.class = FileClass::Denied;
        assert!(matches!(
            make(&ctx(&f), &p("app/core.yml"), Some(&r), None, None, false),
            Built::Opaque(_)
        ));
        let plain = row(FileClass::Structured, Some(h(1)));
        assert!(matches!(
            make(&ctx(&f), &p("app/core.yml"), Some(&plain), None, None, true),
            Built::Opaque(_)
        ));
    }

    #[tokio::test]
    async fn the_baseline_the_mark_and_the_last_overwrite_reach_the_engine() {
        let f = fixture(&["sit1"]);
        let mut r = row(FileClass::Structured, Some(h(1)));
        r.overwrite = Some(Overwrite {
            previous_state: DriftState::NfsAhead,
            previous_nfs: Some(h(5)),
            source: AttributionSource::SyncJob,
        });
        // N is h(1) here and G is what Git has for the file: a conflict against the baseline h(3), and a mark that holds.
        let mark = IntentionalMark {
            nfs: Some(h(1)),
            git: engine::drift_hash(Some(&Bytes::from_static(b"a: 1\n")), FileClass::Structured),
        };
        let Built::Engine(file) = make(
            &ctx(&f),
            &p("app/core.yml"),
            Some(&r),
            Some(h(3)),
            Some(mark),
            false,
        ) else {
            panic!("engine")
        };
        assert_eq!(file.baseline, Baseline::Recorded(Some(h(3))));
        assert_eq!(file.intentional, Some(mark));
        assert_eq!(file.overwrite.map(|o| o.previous_nfs), Some(Some(h(5))));
    }

    fn converged_row(text: &str, blob: u8) -> StateRow {
        let n = engine::drift_hash(Some(&Bytes::from(text.to_owned())), FileClass::Structured);
        StateRow {
            nfs: Some(h(blob)),
            nfs_drift: n,
            ..row(FileClass::Structured, Some(h(blob)))
        }
    }

    fn drift_of(text: &str) -> ContentHash {
        engine::drift_hash(Some(&Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
    }

    #[tokio::test]
    async fn convergence_decides_a_baseline_only_for_an_adopted_swimlane_and_gives_it_to_the_engine() {
        let mut f = fixture(&["sit1"]);
        let r = converged_row("a: 1\n", 1);
        let (_, none) = decided(&f, "app/core.yml", &r, None, None);
        assert!(
            none.baseline.is_none(),
            "before adoption the baselines are the adoption pass's"
        );

        f.lane.adopted = true;
        let (built, d) = decided(&f, "app/core.yml", &r, None, None);
        let w = d.baseline.expect("NFS and Git agree on an adopted swimlane");
        assert_eq!(w.hash, drift_of("a: 1\n"), "the hash for comparing");
        assert_eq!((w.repo, w.branch.as_deref()), (RepoKind::Base, None));
        assert_eq!(Some(w.commit), f.view.heads(&f.lane.tenants).base);
        assert_eq!(w.blob, Some(h(1)), "the raw version behind it");
        let Built::Engine(file) = built else {
            panic!("engine")
        };
        assert_eq!(
            file.baseline,
            Baseline::Recorded(Some(drift_of("a: 1\n"))),
            "the engine is given the baseline, so its answer has no instruction left over"
        );

        // A tenant file is baselined from its branch.
        let (_, d) = decided(&f, "app/core-sit1.yml", &converged_row("a: 2\n", 2), None, None);
        let w = d.baseline.unwrap();
        assert_eq!((w.repo, w.branch.as_deref()), (RepoKind::Tenant, Some("sit1")));

        // Already baselined at N: nothing to decide. Baselined at something else while NFS and Git agree: moved to N.
        let (_, d) = decided(&f, "app/core.yml", &r, Some(drift_of("a: 1\n")), None);
        assert!(d.baseline.is_none());
        let (_, d) = decided(&f, "app/core.yml", &r, Some(drift_of("old: 1\n")), None);
        assert_eq!(d.baseline.map(|w| w.hash), Some(drift_of("a: 1\n")));

        // Not converged: no baseline.
        let differs = converged_row("a: 9\n", 3);
        assert!(
            decided(&f, "app/core.yml", &differs, None, None)
                .1
                .baseline
                .is_none()
        );
        // Not on NFS: nothing to take a baseline from.
        let gone = row(FileClass::Structured, None);
        assert!(
            decided(&f, "app/only-in-git.yml", &gone, None, None)
                .1
                .baseline
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_denied_file_gets_a_baseline_with_no_blob() {
        let mut f = fixture(&["sit1"]);
        f.lane.adopted = true;
        let key = ports::content_hash(b"KEY");
        let r = StateRow {
            nfs: Some(key),
            nfs_drift: Some(key),
            ..row(FileClass::Denied, Some(key))
        };
        let (built, d) = decided(&f, "server.key", &r, None, None);
        assert!(matches!(built, Built::Opaque(_)));
        let w = d.baseline.expect("converged");
        assert_eq!(
            (w.hash, w.blob),
            (key, None),
            "its content is never stored, so there is no version to keep"
        );
    }

    #[tokio::test]
    async fn a_mark_whose_hashes_no_longer_hold_is_cleared_and_not_given_to_the_engine() {
        let f = fixture(&["sit1"]);
        let r = converged_row("a: 9\n", 3);
        // Marked when N was something else: the mark is stale.
        let stale = IntentionalMark {
            nfs: Some(drift_of("a: 5\n")),
            git: Some(drift_of("a: 1\n")),
        };
        let (built, d) = decided(&f, "app/core.yml", &r, None, Some(stale));
        assert!(d.clear_mark);
        let Built::Engine(file) = built else {
            panic!("engine")
        };
        assert_eq!(file.intentional, None);
        // Marked with exactly what is there now: it holds, and stays.
        let holds = IntentionalMark {
            nfs: Some(drift_of("a: 9\n")),
            git: Some(drift_of("a: 1\n")),
        };
        let (built, d) = decided(&f, "app/core.yml", &r, None, Some(holds));
        assert!(!d.clear_mark);
        let Built::Engine(file) = built else {
            panic!("engine")
        };
        assert_eq!(file.intentional, Some(holds));
    }

    #[tokio::test]
    async fn the_content_an_update_reads_is_the_groups_it_names_the_roots_and_for_a_root_change_every_property_source()
     {
        let f = fixture(&["sit1"]);
        let apps = vec![domain::AppName::parse("core").unwrap()];
        let mut entry = test_entry(&f, &apps);
        for (path, blob) in [
            ("application.yml", 1_u8),
            ("core.yml", 2),
            ("core-sit1.yml", 3),
            ("other/a.yml", 4),
            ("other/b.yml", 5),
        ] {
            let Built::Engine(file) = make(
                &ctx(&f),
                &p(path),
                Some(&row(FileClass::Structured, Some(h(blob)))),
                None,
                None,
                false,
            ) else {
                panic!("engine")
            };
            entry.put_file(*file);
        }
        let names = |idx: Vec<usize>| -> Vec<String> {
            idx.into_iter()
                .map(|i| entry.files[i].path.as_str().to_owned())
                .collect()
        };
        // A change to a tenant file reads its group (the base file and the tenant's copy) and the root.
        let logicals: BTreeSet<LogicalFile> = [LogicalFile::parse("core.yml").unwrap()].into();
        let changed: BTreeSet<NfsPath> = [p("core-sit1.yml")].into();
        assert_eq!(
            names(files_to_read(&entry, &logicals, &changed)),
            ["application.yml", "core-sit1.yml", "core.yml"],
            "the group and the root; other/a.yml and other/b.yml are clean and are not read"
        );
        // A change to the root file reads every property source too (`core.yml` is one: the application is named core).
        let logicals: BTreeSet<LogicalFile> = [LogicalFile::parse("application.yml").unwrap()].into();
        let changed: BTreeSet<NfsPath> = [p("application.yml")].into();
        let read = names(files_to_read(&entry, &logicals, &changed));
        assert!(
            read.contains(&"core.yml".to_owned()) && read.contains(&"core-sit1.yml".to_owned()),
            "{read:?}"
        );
        assert!(
            !read.contains(&"other/a.yml".to_owned()),
            "a resource is not a layer of anything: {read:?}"
        );
    }

    fn test_entry(f: &Fixture, apps: &[domain::AppName]) -> LaneEntry {
        let id = f.lane.swimlane.clone();
        let tenants = f.lane.tenants.clone();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let hosts = HostRules::default();
        let limits = engine::Limits::default();
        let input = engine::LaneInput {
            swimlane: &id,
            tenants: &tenants,
            tier: None,
            files: &[],
        };
        let rules = engine::BatchRules {
            application_names: apps,
            host_rules: &hosts,
            limits: &limits,
        };
        let state = engine::recompute_all(&pool, &[input], &rules).unwrap().remove(0);
        LaneEntry::new(
            id,
            tenants,
            apps.to_vec(),
            None,
            f.lane.fingerprint,
            false,
            Arc::clone(&f.lane.global.severity),
            crate::pipeline::git_side::Heads::default(),
            Vec::new(),
            BTreeMap::new(),
            state,
        )
    }

    #[tokio::test]
    async fn fill_and_clear_move_content_in_and_out_and_report_a_missing_blob() {
        let f = fixture(&["sit1"]);
        let mut entry = test_entry(&f, &[]);
        for (path, blob) in [("a.yml", 1_u8), ("b.yml", 2)] {
            let Built::Engine(file) = make(
                &ctx(&f),
                &p(path),
                Some(&row(FileClass::Structured, Some(h(blob)))),
                None,
                None,
                false,
            ) else {
                panic!("engine")
            };
            entry.put_file(*file);
        }
        let indices = vec![0, 1];
        assert_eq!(blobs_of(&entry.files, &indices), [h(1), h(2)].into());
        let bytes: HashMap<ContentHash, Bytes> = [(h(1), Bytes::from_static(b"x: 1\n"))].into();
        let missing = fill(&mut entry.files, &indices, &bytes);
        assert_eq!(missing, [p("b.yml")]);
        assert_eq!(entry.files[0].nfs.as_ref().map(|c| c.bytes.len()), Some(5));
        clear(&mut entry.files);
        assert!(
            entry
                .files
                .iter()
                .all(|f| f.nfs.as_ref().is_some_and(|c| c.bytes.is_empty()))
        );
        let mut store = LaneStore::new(1);
        store.put(entry);
    }
}
