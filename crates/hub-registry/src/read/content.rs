//! One file's content (T6, P7: 30 ms cached, 80 ms cold; S11, D79).
//!
//! The four sources of `ContentSource`:
//!
//! - **nfs**: the current version. The row of `file_state` says its hash, class and format; the bytes come from the blob store (whose
//!   own cache is the "cached" of P7).
//! - **baseline**: the raw version behind the adopted baseline (`baselines.blob_hash`).
//! - **blob**: a version by hash, but only a version *this path has had* (observed, current, or behind its baseline). Without that the
//!   operation would be an oracle that returns any stored blob under any path name.
//! - **git**: the file at a ref of the base or the tenant repository: the NFS path is mapped back to its repository path with the
//!   admin's rule and the swimlane's tenants, the ref is a commit id, a branch or a tag, and the mirror is asked, in `spawn_blocking`
//!   with a timeout.
//!
//! # What is never served
//!
//! A denied file (D79: a name matched by the deny globs, or a file the agent marked) has no content in the store and none is served
//! from anywhere: the answer is `withheld`, with its hash and size, for every source. The decision is the stored `denied` flag or the
//! hub's own glob, so an agent that forgot to mark a file does not make it readable.
//!
//! # Two steps, so that a 304 reads nothing
//!
//! [`Inner::resolve_content`] finds the hash and the facts (a row, or for Git the bytes, which are needed to know the hash), and
//! [`Inner::load_content`] reads the blob. A request whose `If-None-Match` equals the hash is answered between the two.

use std::time::Duration;

use bytes::Bytes;
use domain::{
    CommitId, ContentHash, ContentSource, FileClass, FileContent, FileFormat, GitRef, NfsPath, RepoKind,
    RepoPath, SwimlaneId, TenantId, TenantSet,
};
use engine::{classify, detect_format};
use ports::{GitError, ReadError};
use sqlx::Row;
use tracing::warn;

use super::rows;
use super::service::Inner;
use crate::error::RegistryError;

pub const FILE_SQL: &str = "SELECT nfs_hash::bytea AS hash, class, size, eol_style, encoding, bom, denied \
                        FROM file_state WHERE swimlane_id = $1 AND path = $2";
pub const BASELINE_SQL: &str =
    "SELECT blob_hash::bytea AS blob FROM baselines WHERE swimlane_id = $1 AND path = $2";
pub const TENANTS_SQL: &str =
    "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT 129";
pub const MAPPING_SQL: &str = "SELECT repo, repo_root FROM path_mappings ORDER BY repo LIMIT 8";

/// A file's row in `file_state`, as far as a content read needs it.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub hash: Option<ContentHash>,
    pub class: FileClass,
    pub size: u64,
    pub format: Option<FileFormat>,
    pub denied: bool,
}

/// What a read of one file found before the blob is read.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: NfsPath,
    pub hash: ContentHash,
    pub class: FileClass,
    pub format: Option<FileFormat>,
    pub size: u64,
    /// A denied file: nothing is read, nothing is served.
    pub withheld: bool,
    /// Bytes already in hand (a Git read), or `None` to read the blob `hash`.
    bytes: Option<Bytes>,
}

impl Inner {
    /// The `file_state` row of a path, if it has one.
    pub async fn file_row(
        &self,
        swimlane: &SwimlaneId,
        path: &NfsPath,
    ) -> Result<Option<FileRow>, ReadError> {
        let row = self
            .db
            .run(
                "read_file_row",
                sqlx::query(FILE_SQL)
                    .bind(swimlane.as_str())
                    .bind(path.as_str())
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        let hash: Option<Vec<u8>> = row.get("hash");
        let class: String = row.get("class");
        let size: Option<i64> = row.get("size");
        let eol: Option<String> = row.get("eol_style");
        let encoding: Option<String> = row.get("encoding");
        let bom: Option<bool> = row.get("bom");
        let denied: bool = row.get("denied");
        Ok(Some(FileRow {
            hash: rows::opt_hash(hash.as_deref())?,
            class: rows::class(&class)?,
            size: u64::try_from(size.unwrap_or(0)).unwrap_or(0),
            format: rows::file_format(eol.as_deref(), encoding.as_deref(), bom)?,
            denied,
        }))
    }

    /// Whether a path's content must never be served: the stored flag or class, or the hub's own glob (S17 defence in depth), by
    /// [`crate::denied::is_denied`].
    fn is_denied(&self, path: &NfsPath, row: Option<&FileRow>) -> bool {
        crate::denied::is_denied(
            &self.deny,
            path,
            row.is_some_and(|r| r.denied),
            row.map_or(FileClass::Text, |r| r.class),
        )
    }

    /// Find the hash and facts of the content `src` names for `path`.
    pub async fn resolve_content(
        &self,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        src: &ContentSource,
    ) -> Result<Resolved, ReadError> {
        self.require_swimlane(swimlane).await?;
        let row = self.file_row(swimlane, path).await?;
        let denied = self.is_denied(path, row.as_ref());
        match src {
            ContentSource::Nfs => {
                let row = row.ok_or(ReadError::NotFound)?;
                let hash = row.hash.ok_or(ReadError::NotFound)?;
                Ok(Resolved {
                    path: path.clone(),
                    hash,
                    class: if denied { FileClass::Denied } else { row.class },
                    format: row.format,
                    size: row.size,
                    withheld: denied,
                    bytes: None,
                })
            }
            ContentSource::Baseline => {
                let blob: Option<Option<Vec<u8>>> = self
                    .db
                    .run(
                        "read_baseline_blob",
                        sqlx::query_scalar(BASELINE_SQL)
                            .bind(swimlane.as_str())
                            .bind(path.as_str())
                            .fetch_optional(self.db.pool()),
                    )
                    .await?;
                let hash = rows::opt_hash(blob.flatten().as_deref())?.ok_or(ReadError::NotFound)?;
                Ok(self.resolved_blob(path, hash, row.as_ref(), denied))
            }
            ContentSource::Blob { hash } => {
                if !self.is_version_of(swimlane, path, hash).await? {
                    return Err(ReadError::NotFound);
                }
                Ok(self.resolved_blob(path, *hash, row.as_ref(), denied))
            }
            ContentSource::Git { repo, git_ref } => {
                if denied {
                    // A denied file is not read from Git either; its hash is not known without reading it.
                    return Err(ReadError::NotFound);
                }
                let bytes = self.git_bytes(swimlane, path, *repo, git_ref).await?;
                let class = classify(path, Some(&bytes), &self.deny);
                Ok(Resolved {
                    path: path.clone(),
                    hash: ports::content_hash(&bytes),
                    class,
                    format: Some(detect_format(&bytes)),
                    size: bytes.len() as u64,
                    withheld: class == FileClass::Denied,
                    bytes: Some(bytes),
                })
            }
        }
    }

    fn resolved_blob(
        &self,
        path: &NfsPath,
        hash: ContentHash,
        row: Option<&FileRow>,
        denied: bool,
    ) -> Resolved {
        Resolved {
            path: path.clone(),
            hash,
            class: if denied {
                FileClass::Denied
            } else {
                row.map_or_else(|| classify(path, None, &self.deny), |r| r.class)
            },
            // The row's format is the current version's; another version's is read from its bytes.
            format: None,
            size: 0,
            withheld: denied,
            bytes: None,
        }
    }

    /// Read the blob of a resolved content, and complete the facts that only the bytes tell.
    pub async fn load_content(&self, resolved: Resolved) -> Result<FileContent, ReadError> {
        if resolved.withheld {
            return Ok(FileContent {
                path: resolved.path,
                hash: resolved.hash,
                class: FileClass::Denied,
                format: resolved.format.unwrap_or(FileFormat {
                    eol: domain::EolStyle::Lf,
                    encoding: domain::TextEncoding::Utf8,
                    bom: false,
                }),
                size: resolved.size,
                bytes: None,
                withheld: true,
            });
        }
        let bytes = if let Some(b) = resolved.bytes {
            b
        } else if let Some(b) = self.get_blob(&resolved.hash).await? {
            b
        } else {
            // The row says the content exists and the store does not have it: the hash goes to the log, not to the caller.
            warn!(hash = %resolved.hash, "a file's blob is missing from the store");
            return Err(ReadError::NotFound);
        };
        let format = resolved.format.unwrap_or_else(|| detect_format(&bytes));
        Ok(FileContent {
            path: resolved.path,
            hash: resolved.hash,
            class: resolved.class,
            format,
            size: bytes.len() as u64,
            bytes: Some(bytes),
            withheld: false,
        })
    }

    // ------------------------------------------------------------------------------------------------ Git

    /// The tenants of a swimlane, as a set.
    pub async fn tenants_of(&self, swimlane: &SwimlaneId) -> Result<Vec<TenantId>, ReadError> {
        let names: Vec<String> = self
            .db
            .run(
                "read_tenants",
                sqlx::query_scalar(TENANTS_SQL)
                    .bind(swimlane.as_str())
                    .fetch_all(self.db.pool()),
            )
            .await?;
        names
            .iter()
            .map(|t| TenantId::parse(t).map_err(|_| ReadError::from(RegistryError::Corrupt)))
            .collect()
    }

    /// The admin's path mapping (the base and tenant roots).
    pub async fn mapping_rule(&self) -> Result<domain::PathMappingRule, ReadError> {
        let roots: Vec<(String, String)> = self
            .db
            .run(
                "read_mapping",
                sqlx::query_as(MAPPING_SQL).fetch_all(self.db.pool()),
            )
            .await?;
        Ok(crate::ingest::rules::rule_of(&roots))
    }

    /// The file at a ref of the base or tenant repository, for the NFS path `path` of `swimlane`.
    pub async fn git_bytes(
        &self,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        repo: RepoKind,
        git_ref: &GitRef,
    ) -> Result<Bytes, ReadError> {
        let tenants = TenantSet::new(self.tenants_of(swimlane).await?)
            .map_err(|_| ReadError::from(RegistryError::Corrupt))?;
        let rule = self.mapping_rule().await?;
        let logical = match rule.reverse(path, &tenants).map_err(|_| ReadError::BadRequest)? {
            domain::ReverseMapping::Base { logical, .. } | domain::ReverseMapping::Tenant { logical, .. } => {
                logical
            }
        };
        let root = match repo {
            RepoKind::Base => rule.base_root(),
            RepoKind::Tenant => rule.tenant_root(),
        };
        let repo_path = RepoPath::parse(&format!("{}/{}", root.as_str(), logical.as_str()))
            .map_err(|_| ReadError::BadRequest)?;
        self.git_read(repo, git_ref, &repo_path).await
    }

    /// A repository file at a ref (a commit id, a branch or a tag), on the blocking pool with the Git timeout. A ref that names nothing
    /// is `NotFound`, and so is a file that is not in the tree at that ref.
    pub async fn git_read(
        &self,
        repo: RepoKind,
        git_ref: &GitRef,
        repo_path: &RepoPath,
    ) -> Result<Bytes, ReadError> {
        self.git_read_opt(repo, git_ref, repo_path)
            .await?
            .ok_or(ReadError::NotFound)
    }

    /// As [`Self::git_read`], but a file that is not in the tree at a ref that exists is `None`: the caller can tell a mistyped ref from a
    /// file that is not there.
    pub async fn git_read_opt(
        &self,
        repo: RepoKind,
        git_ref: &GitRef,
        repo_path: &RepoPath,
    ) -> Result<Option<Bytes>, ReadError> {
        let (git, git_ref, repo_path) = (
            std::sync::Arc::clone(&self.git),
            git_ref.clone(),
            repo_path.clone(),
        );
        let job = tokio::task::spawn_blocking(move || -> Result<Option<Bytes>, GitError> {
            let commit = resolve_ref(git.as_ref(), repo, &git_ref)?;
            git.read(repo, &commit, &repo_path)
        });
        let result = tokio::time::timeout(self.config.git_timeout, job)
            .await
            .map_err(|_| {
                warn!(op = "read_git", why = "timed_out", "git read failed");
                ReadError::Unavailable
            })?;
        match result {
            Ok(Ok(found)) => Ok(found),
            Ok(Err(GitError::NotFound | GitError::InvalidRef)) => Err(ReadError::NotFound),
            Ok(Err(GitError::Unavailable)) | Err(_) => {
                warn!(op = "read_git", "git read failed");
                Err(ReadError::Unavailable)
            }
        }
    }

    /// How long a Git read may take.
    pub fn git_timeout(&self) -> Duration {
        self.config.git_timeout
    }
}

/// A ref as the commit it names: a 40 or 64 character id is itself, a branch is its head, a tag is its commit.
pub(crate) fn resolve_ref(
    git: &dyn ports::GitReader,
    repo: RepoKind,
    git_ref: &GitRef,
) -> Result<CommitId, GitError> {
    if let Ok(id) = CommitId::parse(git_ref.as_str()) {
        return Ok(id);
    }
    match git.head(repo, git_ref.as_str()) {
        Ok(commit) => return Ok(commit),
        Err(GitError::NotFound | GitError::InvalidRef) => {}
        Err(e) => return Err(e),
    }
    git.tags(repo)?
        .into_iter()
        .find(|t| t.name.as_str() == git_ref.as_str())
        .map(|t| t.commit)
        .ok_or(GitError::NotFound)
}
