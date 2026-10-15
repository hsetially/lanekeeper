//! Where a pull request goes (T6, D43): the reverse mapping of NFS paths to repository paths, and the explicit target.
//!
//! Pure functions: no I/O, and nothing here reads a file. The mapping rule is the contract's (`domain::PathMappingRule`), and
//! the reverse mapping is the engine's (rule 7), so a name is split on an exact `-<tenant>` before the extension and never on
//! its first hyphen.
//!
//! - **No target:** a tenant file (a name with the suffix of one of the swimlane's tenants) goes to the branch of that tenant,
//!   and any other file to the base repository's default branch.
//! - **Tenant target:** every path goes to `data/config/<logical path>` on that tenant's branch, which has to be one of the
//!   swimlane's tenants. A base file becomes that tenant's file; the file of another tenant is copied over.
//! - **Base target:** every path goes to `config/<logical path>` on the default branch. A tenant file loses its suffix and is
//!   copied into base.
//!
//! One pull request has one destination: paths that would go to different branches are refused, never split.

use std::collections::BTreeSet;

use domain::{NfsPath, PathMappingRule, PrRequest, PrTarget, RepoPath, ReverseMapping, TenantId, TenantSet};
use engine::reverse_map;

use crate::github::BranchName;

/// Which of the two repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// `configuration-base-saas`.
    Base,
    /// `csp-tenant-data`.
    Tenant,
}

impl Side {
    /// The value of `pr_links.repo`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Tenant => "tenant",
        }
    }
}

/// The repository and the branch a pull request is opened against.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Destination {
    pub side: Side,
    pub branch: BranchName,
}

/// Where one NFS path lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    pub nfs: NfsPath,
    /// The path in the destination repository.
    pub repo_path: RepoPath,
    /// The tenant whose suffix the NFS name carries, when it carries the suffix of one of the swimlane's tenants.
    pub suffix: Option<TenantId>,
    pub dest: Destination,
}

/// Why paths cannot be routed. Each is a fixed word: none holds a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    #[error("a pull request needs at least one path")]
    NoPaths,
    #[error("a pull request takes at most 100 paths")]
    TooManyPaths,
    #[error("a path is given twice")]
    DuplicatePath,
    #[error("two paths are the same file in the repository")]
    SameRepoFile,
    #[error("the tenant branch is not one of the swimlane's tenants")]
    UnknownTenant,
    #[error("the paths go to different branches")]
    MixedDestinations,
    #[error("a path cannot be mapped to the repository")]
    Unmappable,
}

fn under(root: &RepoPath, logical: &NfsPath) -> Result<RepoPath, TargetError> {
    RepoPath::parse(&format!("{root}/{logical}")).map_err(|_| TargetError::Unmappable)
}

fn route_one(
    rule: &PathMappingRule,
    tenants: &TenantSet,
    base_branch: &BranchName,
    nfs: &NfsPath,
    target: Option<&PrTarget>,
) -> Result<Routed, TargetError> {
    let (suffix, logical) = match reverse_map(rule, nfs, tenants).map_err(|_| TargetError::Unmappable)? {
        ReverseMapping::Base { logical, .. } => (None, logical),
        ReverseMapping::Tenant { tenant, logical, .. } => (Some(tenant), logical),
    };
    let base = || Destination {
        side: Side::Base,
        branch: base_branch.clone(),
    };
    let tenant = |t: &TenantId| Destination {
        side: Side::Tenant,
        branch: BranchName::of_tenant(t),
    };
    let dest = match (target, &suffix) {
        (Some(PrTarget::Base), _) | (None, None) => base(),
        (Some(PrTarget::Tenant { branch }), _) => tenant(branch),
        (None, Some(own)) => tenant(own),
    };
    let root = match dest.side {
        Side::Base => rule.base_root(),
        Side::Tenant => rule.tenant_root(),
    };
    Ok(Routed {
        repo_path: under(root, logical.as_path())?,
        nfs: nfs.clone(),
        suffix,
        dest,
    })
}

/// Route every path of a request, and check that they all go to the same place.
///
/// # Errors
/// [`TargetError`], for a request that cannot be turned into one pull request.
pub fn route_all(
    rule: &PathMappingRule,
    tenants: &TenantSet,
    base_branch: &BranchName,
    paths: &[NfsPath],
    target: Option<&PrTarget>,
) -> Result<Vec<Routed>, TargetError> {
    if paths.is_empty() {
        return Err(TargetError::NoPaths);
    }
    if paths.len() > PrRequest::MAX_PATHS {
        return Err(TargetError::TooManyPaths);
    }
    if let Some(PrTarget::Tenant { branch }) = target {
        if !tenants.contains(branch) {
            return Err(TargetError::UnknownTenant);
        }
    }
    let mut seen_nfs = BTreeSet::new();
    let mut seen_repo = BTreeSet::new();
    let mut out: Vec<Routed> = Vec::with_capacity(paths.len());
    for nfs in paths {
        if !seen_nfs.insert(nfs) {
            return Err(TargetError::DuplicatePath);
        }
        let routed = route_one(rule, tenants, base_branch, nfs, target)?;
        // Judged first: files of two tenants share their repository path and differ in their branch.
        if out.first().is_some_and(|first| first.dest != routed.dest) {
            return Err(TargetError::MixedDestinations);
        }
        if !seen_repo.insert(routed.repo_path.clone()) {
            return Err(TargetError::SameRepoFile);
        }
        out.push(routed);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PathMappingRule {
        PathMappingRule::default()
    }

    fn tenants(ids: &[&str]) -> TenantSet {
        TenantSet::new(ids.iter().map(|t| TenantId::parse(t).unwrap())).unwrap()
    }

    fn nfs(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    fn master() -> BranchName {
        BranchName::parse("master").unwrap()
    }

    fn route(ids: &[&str], paths: &[&str], target: Option<&PrTarget>) -> Result<Vec<Routed>, TargetError> {
        let paths: Vec<NfsPath> = paths.iter().map(|p| nfs(p)).collect();
        route_all(&rule(), &tenants(ids), &master(), &paths, target)
    }

    /// The example of D43: `tx-infinity-api/tx-infinity-core-sit1.yml` is `data/config/tx-infinity-api/tx-infinity-core.yml` on
    /// the branch `sit1`. The name has hyphens of its own, and only the exact suffix of a tenant is split off.
    #[test]
    fn reverse_mapping_example_sit1() {
        let r = route(&["sit1"], &["tx-infinity-api/tx-infinity-core-sit1.yml"], None).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].repo_path.as_str(),
            "data/config/tx-infinity-api/tx-infinity-core.yml"
        );
        assert_eq!(r[0].dest.side, Side::Tenant);
        assert_eq!(r[0].dest.branch.as_str(), "sit1");
        assert_eq!(r[0].suffix.as_ref().map(TenantId::as_str), Some("sit1"));
    }

    #[test]
    fn a_base_file_defaults_to_the_base_branch() {
        let r = route(&["sit1"], &["tx-infinity-api/tx-infinity-core.yml"], None).unwrap();
        assert_eq!(
            r[0].repo_path.as_str(),
            "config/tx-infinity-api/tx-infinity-core.yml"
        );
        assert_eq!(r[0].dest.side, Side::Base);
        assert_eq!(r[0].dest.branch.as_str(), "master");
        assert!(r[0].suffix.is_none());
    }

    /// A suffix that is not one of the swimlane's tenants is part of the name: the file is a base file (an orphan, C4).
    #[test]
    fn the_suffix_of_a_tenant_the_swimlane_does_not_deploy_is_part_of_the_name() {
        let r = route(&["sit1"], &["a/core-sit2.yml"], None).unwrap();
        assert_eq!(r[0].repo_path.as_str(), "config/a/core-sit2.yml");
        assert_eq!(r[0].dest.side, Side::Base);
    }

    #[test]
    fn an_extensionless_tenant_file_is_mapped_too() {
        let r = route(&["sit1"], &["a/Dockerfile-sit1"], None).unwrap();
        assert_eq!(r[0].repo_path.as_str(), "data/config/a/Dockerfile");
        assert_eq!(r[0].dest.branch.as_str(), "sit1");
    }

    #[test]
    fn a_tenant_target_puts_a_base_file_on_the_tenants_branch() {
        let t = PrTarget::Tenant {
            branch: TenantId::parse("sit1").unwrap(),
        };
        let r = route(&["sit1", "sit2"], &["a/core.yml"], Some(&t)).unwrap();
        assert_eq!(r[0].repo_path.as_str(), "data/config/a/core.yml");
        assert_eq!(r[0].dest.side, Side::Tenant);
        assert_eq!(r[0].dest.branch.as_str(), "sit1");
    }

    #[test]
    fn a_tenant_target_may_be_another_tenants_file() {
        let t = PrTarget::Tenant {
            branch: TenantId::parse("sit2").unwrap(),
        };
        let r = route(&["sit1", "sit2"], &["a/core-sit1.yml"], Some(&t)).unwrap();
        assert_eq!(r[0].repo_path.as_str(), "data/config/a/core.yml");
        assert_eq!(r[0].dest.branch.as_str(), "sit2");
        assert_eq!(r[0].suffix.as_ref().map(TenantId::as_str), Some("sit1"));
    }

    #[test]
    fn a_base_target_drops_the_suffix_of_a_tenant_file() {
        let r = route(
            &["sit1"],
            &["a/core-sit1.yml", "a/other.yml"],
            Some(&PrTarget::Base),
        )
        .unwrap();
        assert_eq!(r[0].repo_path.as_str(), "config/a/core.yml");
        assert_eq!(r[1].repo_path.as_str(), "config/a/other.yml");
        assert!(r.iter().all(|x| x.dest.side == Side::Base));
    }

    #[test]
    fn the_tenant_branch_must_be_one_of_the_swimlanes_tenants() {
        let t = PrTarget::Tenant {
            branch: TenantId::parse("sit9").unwrap(),
        };
        assert_eq!(
            route(&["sit1"], &["a/core.yml"], Some(&t)),
            Err(TargetError::UnknownTenant)
        );
    }

    #[test]
    fn paths_that_go_to_different_branches_are_refused() {
        assert_eq!(
            route(&["sit1", "sit2"], &["a/x-sit1.yml", "a/x-sit2.yml"], None),
            Err(TargetError::MixedDestinations)
        );
        assert_eq!(
            route(&["sit1"], &["a/x-sit1.yml", "a/y.yml"], None),
            Err(TargetError::MixedDestinations)
        );
        // An explicit target makes them one destination.
        assert!(
            route(
                &["sit1", "sit2"],
                &["a/x-sit1.yml", "a/y.yml"],
                Some(&PrTarget::Base)
            )
            .is_ok()
        );
    }

    #[test]
    fn a_request_has_one_to_one_hundred_distinct_files() {
        assert_eq!(route(&["sit1"], &[], None), Err(TargetError::NoPaths));
        let many: Vec<String> = (0..=PrRequest::MAX_PATHS)
            .map(|i| format!("a/f{i}.yml"))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(route(&["sit1"], &many, None), Err(TargetError::TooManyPaths));
        assert!(route(&["sit1"], &many[..PrRequest::MAX_PATHS], None).is_ok());
        assert_eq!(
            route(&["sit1"], &["a/x.yml", "a/x.yml"], None),
            Err(TargetError::DuplicatePath)
        );
    }

    /// Under a base target the base file and the tenant file are one file of the repository; one commit cannot hold both.
    #[test]
    fn two_paths_that_are_one_repository_file_are_refused() {
        assert_eq!(
            route(&["sit1"], &["a/x.yml", "a/x-sit1.yml"], Some(&PrTarget::Base)),
            Err(TargetError::SameRepoFile)
        );
    }

    #[test]
    fn target_errors_hold_no_path() {
        let e = route(&["sit1"], &["secret/dir/x.yml", "secret/dir/x.yml"], None).unwrap_err();
        assert!(!format!("{e} {e:?}").contains("secret"));
    }
}
