//! Whole-swimlane compare (T6, P7: 300 ms, D67, A29, A31).
//!
//! # Two swimlanes' effective trees: subtrees are skipped
//!
//! `effective:<a>` against `effective:<b>` walks `effective_tree_hashes` from the prefix, a round for each depth: the hashes of the
//! child directories on both sides, and a directory whose hash is equal on both sides is **not read** (it is counted in
//! `skipped_identical_subtrees`); the files of the directories that differ come from `effective_files`, and only the ones that differ
//! are listed. Two swimlanes that are almost the same cost a few statements however many files they have, and two that are the same
//! cost one.
//!
//! # Everything else: path-level hashes, no skipping (A29)
//!
//! A side that is not an effective tree (`nfs:`, `baseline:`, `git:`) has no directory hashes to skip by, so each side is read as a
//! map from logical file to hash and the maps are compared; the read is bounded by `MAX_LANE_FILES` and the answer is paged like the
//! other. `nfs` and `baseline` sides use the hash for comparing (line endings normalised); when a side is Git the hashes are the raw
//! SHA-256 of the content on both sides (the tree index has no normalised ones), so a difference in line endings alone shows as a
//! difference there. `effective:` against Git cannot be compared by path hashes and is a bad request.
//!
//! # Paging (A31)
//!
//! The differing files, sorted by path, are paged by a cursor that holds the last path shown when it fits, and otherwise an ordinal
//! with a tag of the whole answer (the same rule as the grid): a cursor whose tag no longer matches is refused. Each page walks the
//! trees again; the walk is the cheap part.

use std::collections::{BTreeMap, BTreeSet};

use domain::{
    CompareRef, CompareState, ContentHash, LogicalFile, NfsPath, Page, Paged, RepoKind, SwimlaneId, TenantId,
    TreeCompareEntry, TreeComparison,
};
use engine::MAX_LANE_FILES;
use ports::ReadError;
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::api::Named;
use super::content::resolve_ref;
use super::cursor::{self, Context, Field, Kind};
use super::service::Inner;
use super::tree::below;
use crate::error::RegistryError;

/// The longest path a cursor holds as itself; a longer one pages by ordinal.
const MAX_PATH_KEY: usize = 300;
/// The most rounds of the walk: a directory is at most 512 levels deep.
const MAX_DEPTH: usize = 520;

const DIR_HASH_SQL: &str =
    "SELECT hash::bytea FROM effective_tree_hashes WHERE swimlane_id = $1 AND dir_path = $2";

/// The child directories of the directories `$2` (the root is `''`) and their hashes.
pub const CHILD_DIRS_SQL: &str = "\
SELECT dir_path, hash::bytea AS hash FROM effective_tree_hashes \
WHERE swimlane_id = $1 AND dir_path <> '' \
  AND (CASE WHEN strpos(dir_path, '/') > 0 THEN regexp_replace(dir_path, '/[^/]*$', '') ELSE '' END) = ANY($2::text[])";

/// The files directly in the directories `$2` and their effective hashes.
pub const DIR_FILES_SQL: &str = "\
SELECT logical_file, hash::bytea AS hash FROM effective_files \
WHERE swimlane_id = $1 \
  AND (CASE WHEN strpos(logical_file, '/') > 0 THEN regexp_replace(logical_file, '/[^/]*$', '') ELSE '' END) = ANY($2::text[])";

/// Every effective file below a prefix (`$2` is a `LIKE` pattern), for the sides that are not walked.
pub const EFFECTIVE_MAP_SQL: &str = "SELECT logical_file, hash::bytea AS hash FROM effective_files WHERE swimlane_id = $1 AND logical_file LIKE $2 LIMIT 200001";

/// The files of a swimlane below a prefix with both hashes, for the maps of `nfs:` sides.
pub const NFS_MAP_SQL: &str = "\
SELECT path, logical_file, nfs_hash::bytea AS raw, nfs_drift_hash::bytea AS norm FROM file_state \
WHERE swimlane_id = $1 AND logical_file LIKE $2 AND nfs_hash IS NOT NULL AND NOT denied LIMIT 200001";

/// The baselines of a swimlane below a prefix with both hashes, for the maps of `baseline:` sides.
pub const BASELINE_MAP_SQL: &str = "\
SELECT fs.path, fs.logical_file, b.blob_hash::bytea AS raw, b.hash::bytea AS norm FROM baselines b \
JOIN file_state fs ON fs.swimlane_id = b.swimlane_id AND fs.path = b.path \
WHERE b.swimlane_id = $1 AND fs.logical_file LIKE $2 AND NOT fs.denied LIMIT 200001";

/// A side's files by logical file, and the tenant that stood for its swimlane where the side has one.
type SideMap = (BTreeMap<String, ContentHash>, Option<(SwimlaneId, TenantId)>);

fn hash_of(bytes: &[u8]) -> Result<ContentHash, RegistryError> {
    super::rows::hash(bytes)
}

/// Where a page resumes.
enum Resume {
    Start,
    AfterPath(String),
    Skip { rows: usize },
}

fn tag_of(entries: &[TreeCompareEntry]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/tree-tag/v1\n");
    for e in entries {
        h.update((e.file.as_str().len() as u64).to_be_bytes());
        h.update(e.file.as_str().as_bytes());
        h.update([e.state as u8]);
    }
    h.finalize()[..8].to_vec()
}

impl Inner {
    /// The hash of a directory in a swimlane's effective tree (the root is `""`).
    async fn dir_hash(&self, swimlane: &SwimlaneId, dir: &str) -> Result<Option<ContentHash>, ReadError> {
        let raw: Option<Vec<u8>> = self
            .db
            .run(
                "read_dir_hash",
                sqlx::query_scalar(DIR_HASH_SQL)
                    .bind(swimlane.as_str())
                    .bind(dir)
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(raw.as_deref().map(hash_of).transpose()?)
    }

    /// The effective walk of two swimlanes: the entries that differ, and the number of identical subtrees that were skipped.
    async fn effective_walk(
        &self,
        left: &SwimlaneId,
        right: &SwimlaneId,
        prefix: &NfsPath,
    ) -> Result<(Vec<TreeCompareEntry>, u32), ReadError> {
        self.require_swimlane(left).await?;
        self.require_swimlane(right).await?;
        let start = prefix.as_str().to_owned();
        let (hl, hr) = tokio::try_join!(self.dir_hash(left, &start), self.dir_hash(right, &start))?;
        match (hl, hr) {
            (None, None) => return Ok((Vec::new(), 0)),
            (Some(a), Some(b)) if a == b => return Ok((Vec::new(), 1)),
            _ => {}
        }
        let mut entries: Vec<TreeCompareEntry> = Vec::new();
        let mut skipped: u32 = 0;
        let mut frontier: Vec<String> = vec![start];
        for _ in 0..MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let (files_l, files_r, dirs_l, dirs_r) = tokio::try_join!(
                self.read_pairs(left, DIR_FILES_SQL, &frontier, "logical_file"),
                self.read_pairs(right, DIR_FILES_SQL, &frontier, "logical_file"),
                self.read_pairs(left, CHILD_DIRS_SQL, &frontier, "dir_path"),
                self.read_pairs(right, CHILD_DIRS_SQL, &frontier, "dir_path"),
            )?;
            diff_maps(&files_l, &files_r, &mut entries);
            frontier = Vec::new();
            let names: BTreeSet<&String> = dirs_l.keys().chain(dirs_r.keys()).collect();
            for name in names {
                match (dirs_l.get(name), dirs_r.get(name)) {
                    (Some(a), Some(b)) if a == b => skipped = skipped.saturating_add(1),
                    _ => frontier.push(name.clone()),
                }
            }
        }
        if !frontier.is_empty() {
            return Err(ReadError::BadRequest);
        }
        Ok((entries, skipped))
    }

    async fn read_pairs(
        &self,
        swimlane: &SwimlaneId,
        sql: &'static str,
        dirs: &[String],
        key: &str,
    ) -> Result<BTreeMap<String, ContentHash>, ReadError> {
        let fetched = self
            .db
            .run(
                "read_tree_level",
                sqlx::query(sql)
                    .bind(swimlane.as_str())
                    .bind(dirs)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut out = BTreeMap::new();
        for row in &fetched {
            let name: String = row.get(key);
            let hash: Vec<u8> = row.get("hash");
            out.insert(name, hash_of(&hash)?);
        }
        Ok(out)
    }

    /// A side as a map from logical file to hash, and the tenant that stood for the swimlane (A30) where the side has one. `raw`: the
    /// hash of the bytes, not the one for comparing.
    async fn side_map(
        &self,
        reference: &CompareRef,
        prefix: &NfsPath,
        raw: bool,
    ) -> Result<SideMap, ReadError> {
        match reference {
            CompareRef::Effective(s) => {
                if raw {
                    // An effective hash is not the hash of any bytes, so it has nothing to be compared with in Git.
                    return Err(ReadError::BadRequest);
                }
                Ok((self.effective_map(s, prefix).await?, None))
            }
            CompareRef::Nfs(s) => self.path_map(s, prefix, NFS_MAP_SQL, raw).await,
            CompareRef::Baseline(s) => self.path_map(s, prefix, BASELINE_MAP_SQL, raw).await,
            CompareRef::GitBase(r) | CompareRef::GitTenant(r) => {
                if !raw {
                    return Err(ReadError::BadRequest);
                }
                let repo = if matches!(reference, CompareRef::GitBase(_)) {
                    RepoKind::Base
                } else {
                    RepoKind::Tenant
                };
                Ok((self.git_map(repo, r, prefix).await?, None))
            }
        }
    }

    /// The effective files of a swimlane below a prefix.
    async fn effective_map(
        &self,
        swimlane: &SwimlaneId,
        prefix: &NfsPath,
    ) -> Result<BTreeMap<String, ContentHash>, ReadError> {
        self.require_swimlane(swimlane).await?;
        let fetched = self
            .db
            .run(
                "read_effective_map",
                sqlx::query(EFFECTIVE_MAP_SQL)
                    .bind(swimlane.as_str())
                    .bind(below(prefix))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut out = BTreeMap::new();
        for row in &fetched {
            let name: String = row.get("logical_file");
            let hash: Vec<u8> = row.get("hash");
            out.insert(name, hash_of(&hash)?);
        }
        Ok(out)
    }

    /// The files an `nfs:` or `baseline:` side has below a prefix, one copy of each logical file: the standing tenant's, else the base
    /// file's.
    async fn path_map(
        &self,
        swimlane: &SwimlaneId,
        prefix: &NfsPath,
        sql: &'static str,
        raw: bool,
    ) -> Result<SideMap, ReadError> {
        self.require_swimlane(swimlane).await?;
        let tenant = self.tenants_of(swimlane).await?.into_iter().next();
        let fetched = self
            .db
            .run(
                "read_path_map",
                sqlx::query(sql)
                    .bind(swimlane.as_str())
                    .bind(below(prefix))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let column = if raw { "raw" } else { "norm" };
        let mut chosen: BTreeMap<String, (u8, ContentHash)> = BTreeMap::new();
        for row in &fetched {
            let (path, logical): (String, String) = (row.get("path"), row.get("logical_file"));
            let Some(hash) = row.get::<Option<Vec<u8>>, _>(column) else {
                continue;
            };
            let hash = hash_of(&hash)?;
            let rank = rank_of(&path, &logical, tenant.as_ref());
            let slot = chosen.entry(logical).or_insert((u8::MAX, hash));
            if rank < slot.0 {
                *slot = (rank, hash);
            }
        }
        Ok((
            chosen.into_iter().map(|(k, (_, h))| (k, h)).collect(),
            tenant.map(|t| (swimlane.clone(), t)),
        ))
    }

    /// The files of a repository at a ref below a prefix, with the SHA-256 of their content.
    async fn git_map(
        &self,
        repo: RepoKind,
        git_ref: &domain::GitRef,
        prefix: &NfsPath,
    ) -> Result<BTreeMap<String, ContentHash>, ReadError> {
        let rule = self.mapping_rule().await?;
        let root = match repo {
            RepoKind::Base => rule.base_root().as_str().to_owned(),
            RepoKind::Tenant => rule.tenant_root().as_str().to_owned(),
        };
        let (git, git_ref) = (std::sync::Arc::clone(&self.git), git_ref.clone());
        let index = tokio::time::timeout(
            self.config.git_timeout,
            tokio::task::spawn_blocking(move || {
                let commit = resolve_ref(git.as_ref(), repo, &git_ref)?;
                git.tree_index(repo, &commit)
            }),
        )
        .await
        .map_err(|_| ReadError::Unavailable)?
        .map_err(|_| ReadError::Unavailable)?
        .map_err(|e| match e {
            ports::GitError::NotFound | ports::GitError::InvalidRef => ReadError::NotFound,
            ports::GitError::Unavailable => ReadError::Unavailable,
        })?;
        let wanted = if prefix.is_root() {
            String::new()
        } else {
            format!("{}/", prefix.as_str())
        };
        let mut out = BTreeMap::new();
        for e in index.entries() {
            let Some(rel) = e
                .path
                .as_str()
                .strip_prefix(root.as_str())
                .and_then(|r| r.strip_prefix('/'))
            else {
                continue;
            };
            if rel.starts_with(wanted.as_str()) {
                out.insert(rel.to_owned(), e.sha256);
            }
            if out.len() > MAX_LANE_FILES {
                return Err(ReadError::BadRequest);
            }
        }
        Ok(out)
    }

    /// `compareTree`.
    pub async fn compare_tree(
        &self,
        l: &CompareRef,
        r: &CompareRef,
        prefix: &NfsPath,
        page: &Page,
    ) -> Result<Named<TreeComparison>, ReadError> {
        let mut tenants: Vec<(SwimlaneId, TenantId)> = Vec::new();
        let (mut entries, skipped) = if let (CompareRef::Effective(a), CompareRef::Effective(b)) = (l, r) {
            self.effective_walk(a, b, prefix).await?
        } else {
            let raw = matches!(l, CompareRef::GitBase(_) | CompareRef::GitTenant(_))
                || matches!(r, CompareRef::GitBase(_) | CompareRef::GitTenant(_));
            let ((ml, tl), (mr, tr)) =
                tokio::try_join!(self.side_map(l, prefix, raw), self.side_map(r, prefix, raw))?;
            tenants.extend(tl);
            tenants.extend(tr);
            let mut entries = Vec::new();
            diff_maps(&ml, &mr, &mut entries);
            (entries, 0)
        };
        entries.sort_by(|a, b| a.file.as_str().cmp(b.file.as_str()));
        let tag = tag_of(&entries);

        let (ls, rs) = (l.to_string(), r.to_string());
        let context = Context::of(Kind::CompareTree, &[&ls, &rs, prefix.as_str()]);
        let resume = match cursor::resume(page, Kind::CompareTree, &context)? {
            None => Resume::Start,
            Some(fields) => match fields.as_slice() {
                [Field::Bytes(path)] => {
                    Resume::AfterPath(String::from_utf8(path.clone()).map_err(|_| ReadError::BadRequest)?)
                }
                [Field::Int(rows), Field::Bytes(given)] => {
                    if *given != tag {
                        return Err(ReadError::BadRequest);
                    }
                    Resume::Skip {
                        rows: usize::try_from(*rows).map_err(|_| ReadError::BadRequest)?,
                    }
                }
                _ => return Err(ReadError::BadRequest),
            },
        };
        let start = match &resume {
            Resume::Start => 0,
            Resume::AfterPath(p) => entries.partition_point(|e| e.file.as_str() <= p.as_str()),
            Resume::Skip { rows, .. } => (*rows).min(entries.len()),
        };
        let limit = page.limit() as usize;
        let end = (start + limit).min(entries.len());
        let more = end < entries.len();
        let next = if more {
            let last = &entries[end - 1];
            let key = if last.file.as_str().len() <= MAX_PATH_KEY {
                vec![Field::text(last.file.as_str())]
            } else {
                vec![
                    Field::Int(i64::try_from(end).unwrap_or(i64::MAX)),
                    Field::Bytes(tag),
                ]
            };
            Some(cursor::issue(Kind::CompareTree, &context, key)?)
        } else {
            None
        };
        Ok(Named {
            value: TreeComparison {
                entries: Paged {
                    items: entries[start..end].to_vec(),
                    next,
                },
                skipped_identical_subtrees: skipped,
            },
            tenants,
        })
    }
}

/// How well a row stands for its logical file: 0 the standing tenant's copy, 1 the base file, 2 anything else.
fn rank_of(path: &str, logical: &str, tenant: Option<&TenantId>) -> u8 {
    if path == logical {
        return 1;
    }
    // By name, not by the row's `kind`: the pipeline sets `kind` from what Git has.
    let is_copy = tenant.is_some_and(|t| {
        LogicalFile::parse(logical)
            .ok()
            .zip(NfsPath::parse(path).ok())
            .is_some_and(|(file, p)| super::effective::is_tenant_copy(&file, t, &p))
    });
    if is_copy { 0 } else { 2 }
}

/// The differences of two maps of logical file to hash, appended to `out`: a file on one side only, or on both with other hashes.
fn diff_maps(
    left: &BTreeMap<String, ContentHash>,
    right: &BTreeMap<String, ContentHash>,
    out: &mut Vec<TreeCompareEntry>,
) {
    let names: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    for name in names {
        let state = match (left.get(name), right.get(name)) {
            (Some(a), Some(b)) if a == b => continue,
            (Some(_), Some(_)) => CompareState::Different,
            (Some(_), None) => CompareState::LeftOnly,
            (None, Some(_)) => CompareState::RightOnly,
            (None, None) => continue,
        };
        // A name that is not a logical file cannot have been written by the registry; it is left out.
        if let Ok(file) = LogicalFile::parse(name) {
            out.push(TreeCompareEntry { file, state });
        }
    }
}
