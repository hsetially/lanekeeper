//! The files that differ, and what the admin decided about each (T5, step 3, D19, D20, D22).
//!
//! The **plan** is the comparison of every file on NFS with Git at the commits the admin chose: the base commit for the files with no
//! tenant suffix, and the commit each chosen tenant branch was at for the files that carry its suffix. A path is mapped back to Git
//! with the swimlane's path mapping and the **chosen** tenant set (the set the pipeline will use once the pass is confirmed, so the
//! plan and the states that follow it are made from the same names). Each file is one of:
//!
//! - **equal**: the same content after line endings are normalised (D19), so its baseline is that content;
//! - **differing**: another content; it needs a resolution;
//! - **neither**: Git has no such file at the commit (untracked), or the file belongs to a tenant the swimlane does not deploy. It gets
//!   no baseline and no resolution.
//!
//! Files that Git has and NFS lacks are counted (`missing_on_nfs`) and not listed: no baseline can be stored for "absent" (A24).
//!
//! A file is compared by its SHA-256 in the tree index first: the same bytes are the same file whatever the line endings, and that is
//! most of them, so most files are decided without reading Git. Only a file whose raw hash differs is read (once for each Git object, the
//! platform's cache of normalised hashes), because only then can the difference be line endings alone.
//!
//! **A resolution is bound to what the admin looked at**: the stored row holds the hash of NFS and of Git it was made against, and is
//! *current* only while both still equal what the plan finds. A file that changed since (the agent sent a new version, or the commit
//! the pass uses is another one) is *stale*: it shows without its resolution, counts as unresolved, and a confirmation refuses it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use domain::{
    CommitId, ContentHash, FileClass, NfsPath, RepoKind, RepoPath, ReverseMapping, SwimlaneId, TenantId,
    TenantSet,
};
use ports::GitReader;

use super::store::{AdoptionStep, Row};
use super::{AdoptionError, AdoptionView, Inner, hash_of};
use crate::db::ts;
use crate::error::RegistryError;
use crate::ingest::rules::rule_of;
use crate::pipeline::blocking;

/// The most files of one swimlane read for a plan.
pub(crate) const MAX_ROWS: i64 = 200_001;
/// The most files one page of [`DifferingFiles`] lists.
pub const MAX_PAGE: usize = 1_000;

/// Why a file differs is the admin's to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason(String);

impl Reason {
    /// The longest reason, in characters (the column's bound).
    pub const MAX_CHARS: usize = 2_000;

    /// A reason as the admin wrote it, without the blanks around it.
    ///
    /// # Errors
    ///
    /// `InvalidReason` for an empty one, one over [`Reason::MAX_CHARS`] characters, or one with a NUL (the database cannot hold it).
    pub fn parse(text: &str) -> Result<Self, AdoptionError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > Self::MAX_CHARS || text.contains('\0') {
            return Err(AdoptionError::InvalidReason);
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the admin decided about a file that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The NFS content is the right one and Git is behind: the baseline is Git's.
    NfsIsRight,
    /// Git is right and NFS is behind: the baseline is NFS's.
    GitIsRight,
    /// NFS and Git differ on purpose: the baseline is Git's and the divergence is marked (D20).
    Intentional(Reason),
}

/// The kind of a resolution, as stored (`adoption_resolutions.resolution`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolutionKind {
    NfsIsRight,
    GitIsRight,
    Intentional,
}

impl ResolutionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NfsIsRight => "nfs_is_right",
            Self::GitIsRight => "git_is_right",
            Self::Intentional => "intentional",
        }
    }

    fn parse(s: &str) -> Result<Self, RegistryError> {
        match s {
            "nfs_is_right" => Ok(Self::NfsIsRight),
            "git_is_right" => Ok(Self::GitIsRight),
            "intentional" => Ok(Self::Intentional),
            _ => Err(RegistryError::Corrupt),
        }
    }
}

impl Resolution {
    pub const fn kind(&self) -> ResolutionKind {
        match self {
            Self::NfsIsRight => ResolutionKind::NfsIsRight,
            Self::GitIsRight => ResolutionKind::GitIsRight,
            Self::Intentional(_) => ResolutionKind::Intentional,
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Self::Intentional(reason) => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// A resolution as it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionView {
    pub kind: ResolutionKind,
    pub reason: Option<String>,
}

/// One file that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DifferingFile {
    pub path: NfsPath,
    /// The hash for drift of the file on NFS, and of the file in Git at the chosen commit.
    pub nfs: ContentHash,
    pub git: ContentHash,
    /// The admin's decision, when there is a current one.
    pub resolution: Option<ResolutionView>,
    /// A resolution was made for another version of the file: it needs a new one.
    pub stale: bool,
}

/// A page of the files that differ, and the totals over all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DifferingFiles {
    pub files: Vec<DifferingFile>,
    /// Files that differ, with a current resolution, and without one.
    pub total: usize,
    pub resolved: usize,
    pub unresolved: usize,
    /// Files that equal Git (they get their baseline without a decision).
    pub equal: usize,
    /// Files Git has at the chosen commits that NFS lacks (not listed: no baseline can say "absent").
    pub missing_on_nfs: usize,
    /// The last path of this page when there are more: pass it as `after` for the next one.
    pub next: Option<NfsPath>,
}

/// The files a bulk resolution reaches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bulk {
    /// Only files at or below this folder (all of them when `None`).
    pub prefix: Option<NfsPath>,
    /// Leave alone the files that have a current resolution.
    pub unresolved_only: bool,
}

/// A file on NFS, as the plan reads it.
#[derive(Debug, Clone)]
pub(crate) struct NfsFile {
    pub path: NfsPath,
    /// `Denied` when the row says so, whatever the extension.
    pub class: FileClass,
    pub denied: bool,
    /// The raw SHA-256 of the content, and the hash for drift.
    pub raw: ContentHash,
    pub drift: ContentHash,
    pub size: Option<u64>,
}

/// A file with a Git side at the chosen commits.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub path: NfsPath,
    pub denied: bool,
    /// NFS: the raw hash and the hash for drift.
    pub nfs_raw: ContentHash,
    pub nfs: ContentHash,
    /// Git at the commit: the hash for drift, and the raw SHA-256 of the blob (the tree index has it).
    pub git: ContentHash,
    pub git_raw: ContentHash,
    pub repo: RepoKind,
    /// The tenant branch, or `None` for the base repository's default branch (as the pipeline's baselines have it).
    pub branch: Option<String>,
    pub commit: CommitId,
}

/// Every file on NFS compared with Git at the chosen commits.
#[derive(Debug, Clone, Default)]
pub(crate) struct Plan {
    pub equal: Vec<Entry>,
    pub differing: Vec<Entry>,
    pub missing_on_nfs: usize,
}

/// A resolution as stored.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub kind: ResolutionKind,
    pub reason: Option<String>,
    pub nfs: Option<ContentHash>,
    pub git: Option<ContentHash>,
}

impl Stored {
    /// Made against the versions the plan finds now.
    pub(crate) fn is_current(&self, entry: &Entry) -> bool {
        self.nfs == Some(entry.nfs) && self.git == Some(entry.git)
    }
}

/// How one file with a Git side compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Equal,
    Differs(ContentHash),
}

/// Compare a file with the Git side without reading anything when the raw hashes decide it (the same bytes are the same file, and a binary
/// or denied file is compared as it is). `normalised` is asked for only when it is needed.
fn compare(
    file: &NfsFile,
    git_raw: ContentHash,
    normalised: impl FnOnce() -> Result<Option<ContentHash>, RegistryError>,
) -> Result<Option<Verdict>, RegistryError> {
    if git_raw == file.raw {
        return Ok(Some(Verdict::Equal));
    }
    if matches!(file.class, FileClass::Binary | FileClass::Denied) {
        // Compared as it is, and the raw hashes differ.
        return Ok(Some(Verdict::Differs(git_raw)));
    }
    Ok(normalised()?.map(|git| {
        if git == file.drift {
            Verdict::Equal
        } else {
            Verdict::Differs(git)
        }
    }))
}

/// Where a path is in Git at the chosen commits, or `None` for a path with no Git side.
fn target_of(
    rule: &domain::PathMappingRule,
    tenants: &TenantSet,
    base: &CommitId,
    commits: &BTreeMap<TenantId, CommitId>,
    path: &NfsPath,
) -> Option<(RepoKind, RepoPath, Option<String>, CommitId)> {
    match rule.reverse(path, tenants).ok()? {
        ReverseMapping::Base { repo_path, .. } => Some((RepoKind::Base, repo_path, None, base.clone())),
        ReverseMapping::Tenant {
            tenant, repo_path, ..
        } => {
            let commit = commits.get(&tenant)?.clone();
            Some((
                RepoKind::Tenant,
                repo_path,
                Some(tenant.as_str().to_owned()),
                commit,
            ))
        }
    }
}

/// The plan, synchronously: call it inside `spawn_blocking`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_plan(
    git: &dyn GitReader,
    git_index: &crate::index::GitIndex,
    rule: &domain::PathMappingRule,
    tenants: &TenantSet,
    base: &CommitId,
    commits: &BTreeMap<TenantId, CommitId>,
    files: &[NfsFile],
) -> Result<Plan, RegistryError> {
    let mut indexes: HashMap<(RepoKind, CommitId), std::sync::Arc<domain::TreeIndex>> = HashMap::new();
    let mut plan = Plan::default();
    for file in files {
        let Some((repo, repo_path, branch, commit)) = target_of(rule, tenants, base, commits, &file.path)
        else {
            continue;
        };
        let index = if let Some(index) = indexes.get(&(repo, commit.clone())) {
            std::sync::Arc::clone(index)
        } else {
            let index = git.tree_index(repo, &commit)?;
            indexes.insert((repo, commit.clone()), std::sync::Arc::clone(&index));
            index
        };
        let Some(entry) = index.get(&repo_path) else {
            continue;
        };
        let git_raw = entry.sha256;
        let verdict = compare(file, git_raw, || {
            git_index
                .git_hash(repo, &commit, &repo_path, file.class)
                .map_err(RegistryError::from)
        })?;
        let Some(verdict) = verdict else { continue };
        let (git_hash, equal) = match verdict {
            Verdict::Equal => (file.drift, true),
            Verdict::Differs(git) => (git, false),
        };
        let planned = Entry {
            path: file.path.clone(),
            denied: file.denied,
            nfs_raw: file.raw,
            nfs: file.drift,
            git: git_hash,
            git_raw,
            repo,
            branch,
            commit,
        };
        if equal {
            plan.equal.push(planned);
        } else {
            plan.differing.push(planned);
        }
    }
    plan.missing_on_nfs = missing_on_nfs(&indexes, rule, base, commits, files);
    Ok(plan)
}

/// Files that Git has at the chosen commits and NFS lacks.
fn missing_on_nfs(
    indexes: &HashMap<(RepoKind, CommitId), std::sync::Arc<domain::TreeIndex>>,
    rule: &domain::PathMappingRule,
    base: &CommitId,
    commits: &BTreeMap<TenantId, CommitId>,
    files: &[NfsFile],
) -> usize {
    let present: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let mut missing = 0;
    let mut visit = |index: &domain::TreeIndex, map: &dyn Fn(&RepoPath) -> Option<NfsPath>| {
        for entry in index.entries() {
            if let Some(nfs) = map(&entry.path) {
                if !present.contains(nfs.as_str()) {
                    missing += 1;
                }
            }
        }
    };
    if let Some(index) = indexes.get(&(RepoKind::Base, base.clone())) {
        visit(index, &|p| rule.base_to_nfs(p).ok());
    }
    for (tenant, commit) in commits {
        if let Some(index) = indexes.get(&(RepoKind::Tenant, commit.clone())) {
            visit(index, &|p| rule.tenant_to_nfs(p, tenant).ok());
        }
    }
    missing
}

/// The step after a change in what is resolved: back to the files while one needs a decision, on to the secret scan when none does. A step
/// before the files, and one after them that is not affected, stay as they are.
pub(crate) fn settled_step(current: AdoptionStep, unresolved: usize) -> AdoptionStep {
    if current < AdoptionStep::Files {
        return current;
    }
    if unresolved > 0 {
        return AdoptionStep::Files;
    }
    if current == AdoptionStep::Files {
        AdoptionStep::Secrets
    } else {
        current
    }
}

/// Whether `path` is `prefix` or below it.
fn under(path: &NfsPath, prefix: &NfsPath) -> bool {
    prefix.is_root()
        || path.as_str() == prefix.as_str()
        || path
            .as_str()
            .strip_prefix(prefix.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
}

type FileRowTuple = (String, String, bool, Vec<u8>, Option<Vec<u8>>, Option<i64>);

/// The files of a swimlane that are on NFS, for a plan and for the secret scan (a swimlane's whole list, bounded by `MAX_ROWS`).
pub const NFS_FILES_SQL: &str = "\
SELECT path, class, denied, nfs_hash::bytea, nfs_drift_hash::bytea, size FROM file_state \
WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL ORDER BY path LIMIT $2";

type StoredTuple = (String, String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>);

/// The stored resolutions of a pass.
const RESOLUTIONS_SQL: &str = "\
SELECT path, resolution, reason, nfs_hash::bytea, git_hash::bytea FROM adoption_resolutions \
WHERE adoption_id = $1 ORDER BY path LIMIT $2";

fn stored_of(rows: Vec<StoredTuple>) -> Result<BTreeMap<String, Stored>, RegistryError> {
    rows.into_iter()
        .map(|(path, kind, reason, nfs, git)| {
            Ok((
                path,
                Stored {
                    kind: ResolutionKind::parse(&kind)?,
                    reason,
                    nfs: nfs.map(hash_of).transpose()?,
                    git: git.map(hash_of).transpose()?,
                },
            ))
        })
        .collect()
}

impl Inner {
    /// The files of a swimlane that are on NFS, with the hash for drift (a row without one is not compared yet and is left out).
    pub(crate) async fn nfs_files(&self, swimlane: &SwimlaneId) -> Result<Vec<NfsFile>, RegistryError> {
        let rows: Vec<FileRowTuple> = self
            .db
            .run(
                "adoption_files",
                sqlx::query_as(NFS_FILES_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_ROWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        if rows.len() >= usize::try_from(MAX_ROWS).unwrap_or(usize::MAX) {
            return Err(RegistryError::Capacity);
        }
        let mut out = Vec::with_capacity(rows.len());
        for (path, class, denied, raw, drift, size) in rows {
            let Some(drift) = drift else { continue };
            let class = FileClass::from_str(&class).map_err(|_| RegistryError::Corrupt)?;
            out.push(NfsFile {
                path: NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
                class: if denied { FileClass::Denied } else { class },
                denied,
                raw: hash_of(raw)?,
                drift: hash_of(drift)?,
                size: size.and_then(|s| u64::try_from(s).ok()),
            });
        }
        // The database's collation is not the byte order that `after` and the plan's order are: the same order everywhere, so that a page
        // never skips or repeats a file.
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// The path mapping rule of the hub.
    pub(crate) async fn mapping_rule(&self) -> Result<domain::PathMappingRule, RegistryError> {
        let roots: Vec<(String, String)> = self
            .db
            .run(
                "adoption_mapping",
                sqlx::query_as("SELECT repo, repo_root FROM path_mappings ORDER BY repo LIMIT 8")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(rule_of(&roots))
    }

    /// Compare every file of the swimlane with Git at the commits the pass has fixed.
    ///
    /// `WrongStep` before the branches are chosen and the base commit is set.
    pub(crate) async fn plan(&self, row: &Row) -> Result<Plan, AdoptionError> {
        let base = row.base()?.ok_or(AdoptionError::WrongStep)?;
        let chosen = row.chosen()?;
        if chosen.is_empty() {
            return Err(AdoptionError::WrongStep);
        }
        let commits = row.commits()?;
        let tenants = TenantSet::new(chosen).map_err(|_| RegistryError::Capacity)?;
        let rule = self.mapping_rule().await?;
        let files = self.nfs_files(&row.swimlane).await?;
        let (git, index) = (std::sync::Arc::clone(&self.git), self.git_index.clone());
        let plan = blocking(
            self.config.git_timeout.max(std::time::Duration::from_secs(60)),
            move || build_plan(&*git, &index, &rule, &tenants, &base, &commits, &files),
        )
        .await?;
        Ok(plan)
    }

    /// The resolutions stored for a pass, by path.
    pub(crate) async fn stored_resolutions(
        &self,
        id: i64,
    ) -> Result<BTreeMap<String, Stored>, RegistryError> {
        let rows: Vec<StoredTuple> = self
            .db
            .run(
                "adoption_resolutions",
                sqlx::query_as(RESOLUTIONS_SQL)
                    .bind(id)
                    .bind(MAX_ROWS)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        stored_of(rows)
    }

    /// The same, inside a transaction: what a confirmation reads after it has closed the pass, so that nothing can change it any more.
    pub(crate) async fn stored_resolutions_in(
        &self,
        tx: &mut crate::db::Txn,
        id: i64,
    ) -> Result<BTreeMap<String, Stored>, RegistryError> {
        let rows: Vec<StoredTuple> = self
            .db
            .run(
                "adoption_resolutions",
                sqlx::query_as(RESOLUTIONS_SQL)
                    .bind(id)
                    .bind(MAX_ROWS)
                    .fetch_all(&mut **tx),
            )
            .await?;
        stored_of(rows)
    }

    /// How many of the files that differ have no current resolution.
    pub(crate) fn unresolved(plan: &Plan, stored: &BTreeMap<String, Stored>) -> usize {
        plan.differing
            .iter()
            .filter(|e| !stored.get(e.path.as_str()).is_some_and(|s| s.is_current(e)))
            .count()
    }

    /// The files of a pass that differ.
    pub(crate) async fn differing_files(
        &self,
        id: i64,
        after: Option<&NfsPath>,
        limit: usize,
    ) -> Result<DifferingFiles, AdoptionError> {
        let row = self.working_row(id).await?;
        if row.step < AdoptionStep::Files {
            return Err(AdoptionError::WrongStep);
        }
        let plan = self.plan(&row).await?;
        let stored = self.stored_resolutions(id).await?;
        let limit = limit.clamp(1, MAX_PAGE);
        let current = |e: &Entry| stored.get(e.path.as_str()).filter(|s| s.is_current(e));
        let resolved = plan.differing.iter().filter(|e| current(e).is_some()).count();
        let mut files = Vec::new();
        let mut more = false;
        for entry in &plan.differing {
            if after.is_some_and(|a| entry.path.as_str() <= a.as_str()) {
                continue;
            }
            if files.len() == limit {
                more = true;
                break;
            }
            let stored_here = stored.get(entry.path.as_str());
            files.push(DifferingFile {
                path: entry.path.clone(),
                nfs: entry.nfs,
                git: entry.git,
                resolution: current(entry).map(|s| ResolutionView {
                    kind: s.kind,
                    reason: s.reason.clone(),
                }),
                stale: stored_here.is_some() && current(entry).is_none(),
            });
        }
        Ok(DifferingFiles {
            next: if more {
                files.last().map(|f| f.path.clone())
            } else {
                None
            },
            files,
            total: plan.differing.len(),
            resolved,
            unresolved: plan.differing.len() - resolved,
            equal: plan.equal.len(),
            missing_on_nfs: plan.missing_on_nfs,
        })
    }

    /// Record resolutions for files that differ.
    pub(crate) async fn resolve(
        &self,
        u: &domain::User,
        id: i64,
        items: Vec<(NfsPath, Resolution)>,
    ) -> Result<AdoptionView, AdoptionError> {
        if items.len() > self.config.adoption_resolutions_per_call {
            return Err(AdoptionError::Read(ports::ReadError::BadRequest));
        }
        let row = self.working_row(id).await?;
        if row.step < AdoptionStep::Files {
            return Err(AdoptionError::WrongStep);
        }
        let plan = self.plan(&row).await?;
        let by_path: HashMap<&str, &Entry> = plan.differing.iter().map(|e| (e.path.as_str(), e)).collect();
        // Every path first: nothing is recorded when one of them is not a file that differs. The last decision for a path wins.
        let mut chosen: BTreeMap<&str, (&Entry, &Resolution)> = BTreeMap::new();
        for (path, resolution) in &items {
            let entry = by_path.get(path.as_str()).ok_or(AdoptionError::NotDiffering)?;
            chosen.insert(path.as_str(), (entry, resolution));
        }
        self.store_resolutions(u, &row, chosen.into_values().collect())
            .await?;
        self.after_resolving(&row, &plan).await?;
        self.view(id).await
    }

    /// Record one resolution for every file of the scope.
    pub(crate) async fn resolve_all(
        &self,
        u: &domain::User,
        id: i64,
        scope: super::Bulk,
        resolution: Resolution,
    ) -> Result<AdoptionView, AdoptionError> {
        let row = self.working_row(id).await?;
        if row.step < AdoptionStep::Files {
            return Err(AdoptionError::WrongStep);
        }
        let plan = self.plan(&row).await?;
        let stored = self.stored_resolutions(id).await?;
        let reached: Vec<(&Entry, &Resolution)> = plan
            .differing
            .iter()
            .filter(|e| scope.prefix.as_ref().is_none_or(|p| under(&e.path, p)))
            .filter(|e| {
                !scope.unresolved_only || !stored.get(e.path.as_str()).is_some_and(|s| s.is_current(e))
            })
            .map(|e| (e, &resolution))
            .collect();
        self.store_resolutions(u, &row, reached).await?;
        self.after_resolving(&row, &plan).await?;
        self.view(id).await
    }

    /// Write the resolutions in one transaction, bound to the hashes of the plan. The pass must still be open: the statement that stamps
    /// it comes first, so a confirmation or a cancel that commits first makes this one `NotOpen`.
    async fn store_resolutions(
        &self,
        u: &domain::User,
        row: &Row,
        items: Vec<(&Entry, &Resolution)>,
    ) -> Result<(), AdoptionError> {
        let now = self.clock.now().unix_millis();
        let mut tx = self.db.begin().await?;
        let stamp = format!(
            "UPDATE adoptions SET updated_at = {} WHERE id = $1 AND status = 'in_progress'",
            ts("$2")
        );
        let open = self
            .db
            .run(
                "adoption_resolve_stamp",
                sqlx::query(&stamp).bind(row.id).bind(now).execute(&mut *tx),
            )
            .await?;
        if open.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        let (tid, oid) = Self::user_ids(u);
        let insert = format!(
            "INSERT INTO adoption_resolutions (adoption_id, path, resolution, reason, nfs_hash, git_hash, resolved_by_tid, resolved_by_oid, resolved_at) \
             SELECT $1, x.path, x.resolution, x.reason, x.nfs, x.git, $7, $8, {} \
             FROM unnest($2::text[], $3::text[], $4::text[], $5::bytea[], $6::bytea[]) AS x(path, resolution, reason, nfs, git) \
             ON CONFLICT (adoption_id, path) DO UPDATE SET resolution = EXCLUDED.resolution, reason = EXCLUDED.reason, \
               nfs_hash = EXCLUDED.nfs_hash, git_hash = EXCLUDED.git_hash, resolved_by_tid = EXCLUDED.resolved_by_tid, \
               resolved_by_oid = EXCLUDED.resolved_by_oid, resolved_at = EXCLUDED.resolved_at",
            ts("$9")
        );
        for chunk in items.chunks(self.config.rows_per_statement) {
            let paths: Vec<&str> = chunk.iter().map(|(e, _)| e.path.as_str()).collect();
            let kinds: Vec<&str> = chunk.iter().map(|(_, r)| r.kind().as_str()).collect();
            let reasons: Vec<Option<&str>> = chunk.iter().map(|(_, r)| r.reason()).collect();
            let nfs: Vec<Vec<u8>> = chunk.iter().map(|(e, _)| e.nfs.as_bytes().to_vec()).collect();
            let git: Vec<Vec<u8>> = chunk.iter().map(|(e, _)| e.git.as_bytes().to_vec()).collect();
            self.db
                .run(
                    "adoption_resolve",
                    sqlx::query(&insert)
                        .bind(row.id)
                        .bind(&paths)
                        .bind(&kinds)
                        .bind(&reasons)
                        .bind(&nfs)
                        .bind(&git)
                        .bind(&tid)
                        .bind(&oid)
                        .bind(now)
                        .execute(&mut *tx),
                )
                .await?;
        }
        self.db.commit(tx).await?;
        Ok(())
    }

    /// Move the pass on (or back) once the resolutions are what they are.
    pub(crate) async fn after_resolving(&self, row: &Row, plan: &Plan) -> Result<(), AdoptionError> {
        let stored = self.stored_resolutions(row.id).await?;
        let step = settled_step(row.step, Self::unresolved(plan, &stored));
        if step != row.step {
            self.set_step(row.id, step).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ports::content_hash;

    use super::*;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn file(class: FileClass, raw: &str, drift: &str) -> NfsFile {
        NfsFile {
            path: NfsPath::parse("app/core.yml").unwrap(),
            class,
            denied: class == FileClass::Denied,
            raw: h(raw),
            drift: h(drift),
            size: Some(1),
        }
    }

    #[test]
    fn a_reason_is_text_without_the_blanks_around_it() {
        assert_eq!(
            Reason::parse("  the batch job rewrites it \n").unwrap().as_str(),
            "the batch job rewrites it"
        );
        for bad in ["", "   ", "a\0b"] {
            assert_eq!(Reason::parse(bad), Err(AdoptionError::InvalidReason), "{bad:?}");
        }
        assert!(Reason::parse(&"x".repeat(Reason::MAX_CHARS)).is_ok());
        assert_eq!(
            Reason::parse(&"x".repeat(Reason::MAX_CHARS + 1)),
            Err(AdoptionError::InvalidReason)
        );
        // Characters, not bytes: 2,000 two-byte characters fit.
        assert!(Reason::parse(&"é".repeat(Reason::MAX_CHARS)).is_ok());
    }

    #[test]
    fn the_same_bytes_are_equal_without_asking_for_the_normalised_hash() {
        let f = file(FileClass::Structured, "a: 1\r\n", "a: 1\n");
        let verdict = compare(&f, h("a: 1\r\n"), || panic!("the raw hashes decide it")).unwrap();
        assert_eq!(verdict, Some(Verdict::Equal));
    }

    #[test]
    fn other_bytes_are_equal_when_only_the_line_endings_differ() {
        let f = file(FileClass::Structured, "a: 1\r\n", "a: 1\n");
        let verdict = compare(&f, h("a: 1\n"), || Ok(Some(h("a: 1\n")))).unwrap();
        assert_eq!(
            verdict,
            Some(Verdict::Equal),
            "N and G are the same after normalising (D19)"
        );
        let verdict = compare(&f, h("a: 2\n"), || Ok(Some(h("a: 2\n")))).unwrap();
        assert_eq!(verdict, Some(Verdict::Differs(h("a: 2\n"))));
        // Git no longer has the blob: nothing to say.
        assert_eq!(compare(&f, h("a: 2\n"), || Ok(None)).unwrap(), None);
        // An error is an error.
        assert_eq!(
            compare(&f, h("a: 2\n"), || Err(RegistryError::Timeout)),
            Err(RegistryError::Timeout)
        );
    }

    #[test]
    fn a_binary_or_denied_file_is_compared_as_it_is_and_never_read() {
        for class in [FileClass::Binary, FileClass::Denied] {
            let f = file(class, "BYTES", "BYTES");
            assert_eq!(
                compare(&f, h("OTHER"), || panic!("never read for {class:?}")).unwrap(),
                Some(Verdict::Differs(h("OTHER")))
            );
            assert_eq!(
                compare(&f, h("BYTES"), || panic!("never read")).unwrap(),
                Some(Verdict::Equal)
            );
        }
    }

    #[test]
    fn the_step_follows_what_is_resolved() {
        use AdoptionStep::{BaseVersion, Branch, Confirm, Files, Secrets};
        assert_eq!(
            settled_step(Files, 0),
            Secrets,
            "the last file resolved: on to the scan"
        );
        assert_eq!(settled_step(Files, 3), Files);
        assert_eq!(
            settled_step(Secrets, 2),
            Files,
            "a file that needs a new decision brings the pass back"
        );
        assert_eq!(settled_step(Confirm, 1), Files);
        assert_eq!(
            settled_step(Confirm, 0),
            Confirm,
            "nothing changed what was reviewed"
        );
        assert_eq!(settled_step(Secrets, 0), Secrets);
        assert_eq!(
            settled_step(Branch, 0),
            Branch,
            "before the files nothing is settled"
        );
        assert_eq!(settled_step(BaseVersion, 5), BaseVersion);
    }

    #[test]
    fn a_folder_is_a_prefix_by_whole_names() {
        let p = |s: &str| NfsPath::parse(s).unwrap();
        assert!(under(&p("app/core.yml"), &p("app")));
        assert!(under(&p("app/core.yml"), &p("app/core.yml")));
        assert!(!under(&p("apple/core.yml"), &p("app")), "not a string prefix");
        assert!(under(&p("app/core.yml"), &NfsPath::root()));
        assert!(!under(&p("other/a.yml"), &p("app")));
    }

    #[test]
    fn a_resolution_is_current_only_for_the_versions_it_was_made_against() {
        let entry = Entry {
            path: NfsPath::parse("a.yml").unwrap(),
            denied: false,
            nfs_raw: h("n"),
            nfs: h("n"),
            git: h("g"),
            git_raw: h("g"),
            repo: RepoKind::Base,
            branch: None,
            commit: CommitId::parse(&"a".repeat(40)).unwrap(),
        };
        let stored = |nfs: &str, git: &str| Stored {
            kind: ResolutionKind::NfsIsRight,
            reason: None,
            nfs: Some(h(nfs)),
            git: Some(h(git)),
        };
        assert!(stored("n", "g").is_current(&entry));
        assert!(!stored("n2", "g").is_current(&entry), "NFS changed since");
        assert!(
            !stored("n", "g2").is_current(&entry),
            "Git (or the commit) changed since"
        );
        let without = Stored {
            nfs: None,
            ..stored("n", "g")
        };
        assert!(!without.is_current(&entry));
    }

    #[test]
    fn resolution_kinds_are_the_checks_of_the_table() {
        for (kind, name) in [
            (ResolutionKind::NfsIsRight, "nfs_is_right"),
            (ResolutionKind::GitIsRight, "git_is_right"),
            (ResolutionKind::Intentional, "intentional"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(ResolutionKind::parse(name), Ok(kind));
        }
        assert_eq!(ResolutionKind::parse("other"), Err(RegistryError::Corrupt));
        let reason = Reason::parse("on purpose").unwrap();
        assert_eq!(
            Resolution::Intentional(reason).kind(),
            ResolutionKind::Intentional
        );
        assert_eq!(Resolution::NfsIsRight.reason(), None);
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        fn text() -> impl Strategy<Value = String> {
            prop_oneof![
                any::<String>(),
                (0_usize..2_100).prop_map(|n| "é".repeat(n)),
                (0_usize..4, 0_usize..2_100, any::<bool>()).prop_map(|(pad, n, nul)| {
                    format!(
                        "{}{}{}{}",
                        " ".repeat(pad),
                        "x".repeat(n),
                        if nul { "\0" } else { "" },
                        " ".repeat(pad)
                    )
                }),
            ]
        }

        proptest! {
            /// A reason is accepted exactly when it is text that the column holds: not blank, at most 2,000 characters, no NUL. What is
            /// kept is the text without the blanks around it, and no input makes it panic.
            #[test]
            fn a_reason_is_accepted_exactly_when_the_column_can_hold_it(input in text()) {
                let trimmed = input.trim();
                let fits = !trimmed.is_empty() && trimmed.chars().count() <= Reason::MAX_CHARS && !trimmed.contains('\0');
                match Reason::parse(&input) {
                    Ok(reason) => {
                        prop_assert!(fits);
                        prop_assert_eq!(reason.as_str(), trimmed);
                    }
                    Err(e) => {
                        prop_assert!(!fits);
                        prop_assert_eq!(e, AdoptionError::InvalidReason);
                    }
                }
            }

            /// Whatever is resolved, the step never goes before the files once the pass has reached them, never reaches the secrets while a
            /// file needs a decision, and settling twice is settling once.
            #[test]
            fn the_step_is_settled_by_what_is_unresolved(step in 0_usize..5, unresolved in 0_usize..4) {
                let steps = [
                    AdoptionStep::Branch,
                    AdoptionStep::BaseVersion,
                    AdoptionStep::Files,
                    AdoptionStep::Secrets,
                    AdoptionStep::Confirm,
                ];
                let current = steps[step];
                let settled = settled_step(current, unresolved);
                if current >= AdoptionStep::Files {
                    prop_assert!(settled >= AdoptionStep::Files);
                    if unresolved > 0 {
                        prop_assert_eq!(settled, AdoptionStep::Files);
                    } else {
                        prop_assert!(settled >= AdoptionStep::Secrets);
                    }
                } else {
                    prop_assert_eq!(settled, current);
                }
                prop_assert_eq!(settled_step(settled, unresolved), settled);
            }
        }
    }
}
