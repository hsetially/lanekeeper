//! Git <-> NFS path mapping (D13, D82-D84).
//!
//! | Repo | Repo path | NFS path |
//! |---|---|---|
//! | base | `config/<rel>/<name>.<ext>` | `<rel>/<name>.<ext>` |
//! | tenant, branch B | `data/config/<rel>/<name>.<ext>` | `<rel>/<name>-B.<ext>` |
//!
//! Rules (plan Q14):
//! - the extension is the last dot segment of the final component; a leading-dot-only name such as
//!   `.env` has no extension, and a file with no extension gets the suffix at the end of its name;
//! - reverse mapping strips an exact `-<tenant>` immediately before the extension, and only for a
//!   known tenant. Names like `tx-infinity-core` contain hyphens of their own, so nothing is ever
//!   split on the first hyphen;
//! - a tenant file whose name ends in a dot is refused (`MapError::TrailingDot`). Such a name has no extension, so the
//!   suffix would go after the dot (`core-sit1.` would become `core-sit1.-sit1`), where reverse mapping reads it as the
//!   extension and cuts the name in the wrong place: two repository paths (`core-sit1.` and `core.-sit1`) would be one NFS
//!   path, and a tenant file would read back as another file or as a base file. Tenant ids hold no dot so that a suffix is
//!   never an extension; a trailing dot is the one name that would put it in one;
//! - when several known tenants match, the longest wins;
//! - a base file whose name coincidentally ends with a known tenant id is reported as a tenant file.
//!   This is a documented limit of the naming scheme, not a bug.

use std::collections::BTreeSet;

use crate::{LogicalFile, NfsPath, PathError, RepoPath, TenantId};

/// Why a mapping failed. Never carries the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MapError {
    #[error("path is not below the configured repository root")]
    OutsideRoot,
    #[error("mapped path is invalid")]
    InvalidPath(#[from] PathError),
    /// A tenant file whose name ends in a dot: the suffix would land after the dot and read back as an extension.
    #[error("a tenant file name ends with a dot, so its tenant suffix cannot be read back")]
    TrailingDot,
}

/// The tenants deployed to one swimlane (Q34: a set, never exactly one).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TenantSet(BTreeSet<TenantId>);

/// Why a [`TenantSet`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TenantSetError {
    #[error("too many tenants")]
    TooMany,
}

impl TenantSet {
    /// Upper bound on the tenants of one swimlane.
    pub const MAX_TENANTS: usize = 128;

    pub fn new(tenants: impl IntoIterator<Item = TenantId>) -> Result<Self, TenantSetError> {
        let mut set = BTreeSet::new();
        for t in tenants {
            set.insert(t);
            if set.len() > Self::MAX_TENANTS {
                return Err(TenantSetError::TooMany);
            }
        }
        Ok(Self(set))
    }

    pub fn contains(&self, t: &TenantId) -> bool {
        self.0.contains(t)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &TenantId> {
        self.0.iter()
    }
}

/// The result of mapping an NFS path back to Git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverseMapping {
    /// A file with no known tenant suffix: it lives in the base repo.
    Base {
        repo_path: RepoPath,
        logical: LogicalFile,
    },
    /// A tenant file: it lives on branch `tenant` of the tenant repo.
    Tenant {
        tenant: TenantId,
        repo_path: RepoPath,
        logical: LogicalFile,
    },
}

/// Splits a file name into stem and extension.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

fn join_name(stem: &str, ext: Option<&str>) -> String {
    ext.map_or_else(|| stem.to_owned(), |e| format!("{stem}.{e}"))
}

fn join_dir(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The configurable mapping rule for the two repos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMappingRule {
    base_root: RepoPath,
    tenant_root: RepoPath,
}

impl Default for PathMappingRule {
    /// Base config under `config/`, tenant config under `data/config/`.
    fn default() -> Self {
        Self {
            base_root: RepoPath::trusted("config"),
            tenant_root: RepoPath::trusted("data/config"),
        }
    }
}

impl PathMappingRule {
    pub fn new(base_root: RepoPath, tenant_root: RepoPath) -> Self {
        Self {
            base_root,
            tenant_root,
        }
    }

    pub fn base_root(&self) -> &RepoPath {
        &self.base_root
    }

    pub fn tenant_root(&self) -> &RepoPath {
        &self.tenant_root
    }

    /// Base repo path to NFS path: the root prefix is dropped, nothing is renamed.
    pub fn base_to_nfs(&self, repo_path: &RepoPath) -> Result<NfsPath, MapError> {
        let rel = repo_path.below(&self.base_root).ok_or(MapError::OutsideRoot)?;
        Ok(NfsPath::parse(rel)?)
    }

    /// Tenant repo path on branch `tenant` to NFS path: the root prefix is dropped and `-<tenant>` is
    /// inserted before the extension.
    ///
    /// # Errors
    ///
    /// `OutsideRoot` for a path that is not below the tenant root, `InvalidPath` for a result that is not a valid
    /// NFS path, `TrailingDot` for a name that ends in a dot.
    pub fn tenant_to_nfs(&self, repo_path: &RepoPath, tenant: &TenantId) -> Result<NfsPath, MapError> {
        let rel = NfsPath::parse(repo_path.below(&self.tenant_root).ok_or(MapError::OutsideRoot)?)?;
        // A name that ends in a dot has no extension (`split_name`), so the suffix would go after the dot, where `reverse`
        // reads it as the extension: two repository paths would be one NFS path. Refuse it rather than rename it.
        if rel.file_name().ends_with('.') {
            return Err(MapError::TrailingDot);
        }
        let (stem, ext) = split_name(rel.file_name());
        let renamed = join_name(&format!("{stem}-{tenant}"), ext);
        Ok(NfsPath::parse(&join_dir(rel.dir_str(), &renamed))?)
    }

    /// NFS path to its repo path. A name ending in `-<tenant>` for a known tenant (before the extension)
    /// is a tenant file; anything else is a base file.
    pub fn reverse(&self, nfs: &NfsPath, tenants: &TenantSet) -> Result<ReverseMapping, MapError> {
        let (stem, ext) = split_name(nfs.file_name());
        let best = tenants
            .iter()
            .filter(|t| {
                stem.strip_suffix(t.as_str())
                    .and_then(|rest| rest.strip_suffix('-'))
                    .is_some_and(|rest| !matches!(rest, "" | "." | ".."))
            })
            .max_by_key(|t| t.as_str().len());

        let Some(tenant) = best else {
            return Ok(ReverseMapping::Base {
                repo_path: RepoPath::under(&self.base_root, nfs.as_str())?,
                logical: LogicalFile::from_nfs(nfs.clone())?,
            });
        };

        // `best` matched, so the stem is `<rest>-<tenant>`.
        let rest = &stem[..stem.len() - tenant.as_str().len() - 1];
        let logical_path = NfsPath::parse(&join_dir(nfs.dir_str(), &join_name(rest, ext)))?;
        Ok(ReverseMapping::Tenant {
            tenant: tenant.clone(),
            repo_path: RepoPath::under(&self.tenant_root, logical_path.as_str())?,
            logical: LogicalFile::from_nfs(logical_path)?,
        })
    }
}
