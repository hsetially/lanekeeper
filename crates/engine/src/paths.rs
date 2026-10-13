//! Paths between the two repositories and the NFS tree (T4, S11, D13, D82-D84), and what an NFS path is.
//!
//! The mapping rule itself is `domain::PathMappingRule` (a contract type, shared with the agent's and the hub's other
//! code). This module is what the engine builds on it:
//!
//! - `map_path` and `reverse_map`: Git to NFS and back, typed on both sides (`RepoPath`, `NfsPath`, `TenantId`), so a
//!   hostile string cannot reach the mapping and the mapping cannot produce a path outside the roots;
//! - `classify_path`: is an NFS path a base file, a tenant file or an untracked file, which logical file is it, and which
//!   tenant;
//! - `MergeRules`: which merge mode a logical file has. It is interim (D82 supersedes D14: the mode follows the file's
//!   role in the config-server's resolution) and T10 deletes it.
//!
//! Nothing here reads a file. Whether Git has a path is the caller's knowledge, passed in as a closure.

use domain::{
    FileKind, LogicalFile, MapError, MergeMode, NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId,
    TenantSet,
};
use globset::{Candidate, Glob, GlobSet, GlobSetBuilder};

use crate::error::MergeRulesError;

/// Which repository a path is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoSource {
    /// The base repository, `config/...`.
    Base,
    /// The tenant repository, on the branch named like the tenant, `data/config/...`.
    Tenant(TenantId),
}

/// A repository path to the NFS path it is deployed at: the root is dropped, and for a tenant file `-<tenant>` goes before
/// the extension (at the end of the name when there is none).
///
/// # Errors
///
/// `OutsideRoot` for a path that is not below the rule's root for that repository (so a base path is never read as a tenant
/// path or the other way round), `InvalidPath` for a result that is not a valid NFS path.
pub fn map_path(
    rule: &PathMappingRule,
    source: &RepoSource,
    repo_path: &RepoPath,
) -> Result<NfsPath, MapError> {
    match source {
        RepoSource::Base => rule.base_to_nfs(repo_path),
        RepoSource::Tenant(tenant) => rule.tenant_to_nfs(repo_path, tenant),
    }
}

/// An NFS path to the repository file it comes from. Only an exact `-<tenant>` immediately before the extension, for a
/// tenant of `tenants`, makes a tenant file; every other name is a base file, hyphens and all.
///
/// # Errors
///
/// `InvalidPath` if the result is not a valid path (the paths are bounded in length, and the roots add to it).
pub fn reverse_map(
    rule: &PathMappingRule,
    nfs: &NfsPath,
    tenants: &TenantSet,
) -> Result<ReverseMapping, MapError> {
    rule.reverse(nfs, tenants)
}

/// What an NFS path is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    /// `Base` and `Tenant` when Git has the file, `Untracked` when it does not.
    pub kind: FileKind,
    /// The path without a tenant suffix. For a name with a suffix that is not a tenant of the swimlane it is the name as it
    /// is: the suffix is part of it.
    pub logical: LogicalFile,
    /// The tenant of the suffix, when the name carries the suffix of a known tenant. An untracked tenant-looking file keeps
    /// it, so that the file can be adopted into the right branch.
    pub tenant: Option<TenantId>,
    /// Where the file is, or would be, in Git.
    pub repo_path: RepoPath,
}

/// Classifies an NFS path.
///
/// `tracked` tells whether Git has the file. It is given what the name was read as (the repository, the branch and the
/// repository path), so that the caller can ask the right branch. The engine does no I/O: the answer is the caller's
/// knowledge of the repositories.
///
/// # Errors
///
/// As `reverse_map`.
pub fn classify_path(
    rule: &PathMappingRule,
    nfs: &NfsPath,
    tenants: &TenantSet,
    tracked: impl Fn(&ReverseMapping) -> bool,
) -> Result<FileFacts, MapError> {
    let mapping = rule.reverse(nfs, tenants)?;
    let in_git = tracked(&mapping);
    Ok(match mapping {
        ReverseMapping::Base { repo_path, logical } => FileFacts {
            kind: if in_git {
                FileKind::Base
            } else {
                FileKind::Untracked
            },
            logical,
            tenant: None,
            repo_path,
        },
        ReverseMapping::Tenant {
            tenant,
            repo_path,
            logical,
        } => FileFacts {
            kind: if in_git {
                FileKind::Tenant
            } else {
                FileKind::Untracked
            },
            logical,
            tenant: Some(tenant),
            repo_path,
        },
    })
}

/// Which merge mode a logical file has, by glob (D14). Interim: D82 supersedes D14, the mode follows the file's role, and
/// T10 deletes this type and its tests.
///
/// The first rule that matches wins; a file that matches none has the default mode. Globs match the whole logical path
/// from the root, case-sensitively (file names on NFS are), and `*` crosses `/`.
#[derive(Clone)]
pub struct MergeRules {
    default: MergeMode,
    set: Option<GlobSet>,
    modes: Vec<MergeMode>,
}

impl MergeRules {
    /// The most rules `new` takes.
    pub const MAX_RULES: usize = 1_000;
    /// The longest glob `new` takes, in bytes.
    pub const MAX_GLOB_BYTES: usize = 512;

    /// Rules in order of precedence.
    ///
    /// # Errors
    ///
    /// `TooManyRules`, `GlobTooLong`, or `InvalidGlob` for a glob that does not compile. The error does not contain the
    /// glob.
    pub fn new(default: MergeMode, rules: &[(&str, MergeMode)]) -> Result<Self, MergeRulesError> {
        if rules.len() > Self::MAX_RULES {
            return Err(MergeRulesError::TooManyRules);
        }
        let mut builder = GlobSetBuilder::new();
        let mut modes = Vec::with_capacity(rules.len());
        for (text, mode) in rules {
            if text.len() > Self::MAX_GLOB_BYTES {
                return Err(MergeRulesError::GlobTooLong);
            }
            builder.add(Glob::new(text).map_err(|_| MergeRulesError::InvalidGlob)?);
            modes.push(*mode);
        }
        let set = if modes.is_empty() {
            None
        } else {
            Some(builder.build().map_err(|_| MergeRulesError::InvalidGlob)?)
        };
        Ok(Self { default, set, modes })
    }

    /// The merge mode of a logical file.
    pub fn mode_for(&self, file: &LogicalFile) -> MergeMode {
        let Some(set) = &self.set else {
            return self.default;
        };
        let mut hits = Vec::new();
        set.matches_candidate_into(&Candidate::new(file.as_str()), &mut hits);
        // `matches_into` gives the indices of the globs that match; the first rule is the lowest index.
        hits.iter()
            .min()
            .and_then(|&i| self.modes.get(i))
            .copied()
            .unwrap_or(self.default)
    }
}

impl Default for MergeRules {
    /// Every file is `whole_file` (D14's default).
    fn default() -> Self {
        Self {
            default: MergeMode::WholeFile,
            set: None,
            modes: Vec::new(),
        }
    }
}

impl std::fmt::Debug for MergeRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The globs are admin configuration; they are not printed.
        f.debug_struct("MergeRules")
            .field("default", &self.default)
            .field("rules", &self.modes.len())
            .finish_non_exhaustive()
    }
}
