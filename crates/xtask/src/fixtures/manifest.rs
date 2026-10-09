//! `manifest.json`: what was generated, and the planted items that engine tests use as an oracle.
//!
//! Every path in it is relative to the output directory and nothing in it depends on the machine, so two runs with the
//! same seed give the same bytes.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub schema: u32,
    pub scale: String,
    pub seed: u64,
    pub repos: Repos,
    pub paths: PathRoots,
    pub channels: Vec<String>,
    pub channel_folders: Vec<String>,
    pub tenants: Vec<String>,
    pub counts: Counts,
    pub swimlanes: Vec<Swimlane>,
    pub planted: Planted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repos {
    pub base: Repo,
    pub tenant: Repo,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repo {
    pub path: String,
    pub default_branch: String,
    pub refs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PathRoots {
    pub base_root: String,
    pub tenant_root: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    pub swimlanes: usize,
    pub tenant_branches: usize,
    pub base_files: usize,
    pub nfs_files: usize,
    pub max_file_bytes: usize,
    pub max_file_lines: usize,
    pub largest_yaml_lines: usize,
    pub image_files: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Swimlane {
    pub id: String,
    pub tenants: Vec<String>,
    pub nfs_root: String,
    pub files: usize,
    /// SHA-256 over the sorted (path, content hash) pairs of the swimlane's files.
    pub tree_sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Planted {
    /// File names that exist in more than one non-channel folder (C9).
    pub duplicate_names: Vec<DuplicateName>,
    /// Base files that repeat a key (C8).
    pub duplicate_keys: Vec<DuplicateKeys>,
    /// Tenant copies that lack entries the base has (C1).
    pub stale_forks: Vec<StaleFork>,
    /// Tenant copies identical to base (C2).
    pub redundant_copies: Vec<TenantPath>,
    /// NFS files edited by hand after the last sync.
    pub nfs_ahead: Vec<SwimlanePath>,
    /// NFS base files that hold an older version than Git's head.
    pub git_ahead: Vec<GitAhead>,
    /// NFS files with no counterpart in Git.
    pub untracked: Vec<SwimlanePath>,
    /// NFS files whose tenant suffix is not in the swimlane's tenant set (C4).
    pub orphans: Vec<Orphan>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateName {
    pub name: String,
    pub folders: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateKeys {
    pub path: String,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StaleFork {
    pub tenant: String,
    /// Logical path, below `config/` and `data/config/`.
    pub path: String,
    pub fork_version: u8,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TenantPath {
    pub tenant: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwimlanePath {
    pub swimlane: String,
    /// Path below the swimlane's NFS root.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GitAhead {
    pub swimlane: String,
    pub path: String,
    pub nfs_version: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Orphan {
    pub swimlane: String,
    pub path: String,
    pub suffix: String,
}
