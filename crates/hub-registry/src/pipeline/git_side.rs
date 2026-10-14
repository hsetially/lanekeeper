//! The Git side of one batch (T3, D19, D20, P2): where each deployable branch is now, and what Git holds at the path of a file.
//!
//! Drift compares **N** (NFS now) with **G** (Git now). A base file's G is the file in the head of the base repository's default
//! branch; a tenant file's G is the file in the head of that tenant's branch, and only for a tenant the swimlane deploys (D84,
//! Q34). The batch asks the mirror where the heads are **once** ([`GitView::resolve`]), so that every path and every swimlane of the
//! batch is computed against the same commits, and then answers each path from the tree indexes the mirror caches per commit. The
//! normalised hash of a file is [`GitIndex::git_hash`]: read once per object, whatever the swimlanes and commits that name it.
//!
//! A path is **tracked** when Git has it at the commit the mapping names; the engine's `classify_path` takes that as a closure, and
//! an untracked file is `untracked` drift (C4). The enumeration of the paths Git holds for a swimlane ([`GitView::universe`]) is the
//! forward mapping of the trees (`config/<rel>` to `<rel>`, `data/config/<rel>` to `<name>-<tenant>.<ext>`). It only finds
//! candidates: each candidate then goes through the same reverse mapping as every NFS file, so an NFS path and a Git path are never
//! two views of one file (the documented limit of the naming scheme, a base file whose name ends with a tenant id, is one file here
//! as everywhere).
//!
//! Everything in this module is synchronous, like the reader: call it inside `spawn_blocking`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::{
    BranchKind, CommitId, ContentHash, FileClass, NfsPath, PathMappingRule, RepoKind, RepoPath, TenantId,
    TenantSet, TreeIndex,
};
use ports::{GitError, GitReader};

use crate::error::RegistryError;
use crate::index::GitIndex;

/// A head and the index of its tree.
#[derive(Clone)]
struct HeadTree {
    commit: CommitId,
    index: Arc<TreeIndex>,
}

/// The commits a swimlane's Git side was computed at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Heads {
    pub base: Option<CommitId>,
    pub tenants: BTreeMap<TenantId, CommitId>,
}

/// Where the heads were when a batch began, and the trees they hold.
#[derive(Clone)]
pub(crate) struct GitView {
    index: GitIndex,
    base: Option<HeadTree>,
    /// Only the tenants some swimlane of the batch deploys.
    tenants: BTreeMap<TenantId, HeadTree>,
    /// Every deployable branch of the tenant repository, deployed by a swimlane of the batch or not.
    deployable: TenantSet,
}

impl std::fmt::Debug for GitView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitView")
            .field("base", &self.base.as_ref().map(|b| b.commit.short()))
            .field("tenants", &self.tenants.len())
            .field("deployable", &self.deployable.len())
            .finish_non_exhaustive()
    }
}

/// The heads of the default branch of the base repository and of the deployable tenant branches, without any tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct LiveHeads {
    pub base: Option<(String, CommitId)>,
    pub tenants: BTreeMap<TenantId, CommitId>,
}

impl LiveHeads {
    /// The heads as `(repo, branch)` to commit, the key the reconcile compares.
    pub(crate) fn branches(&self) -> BTreeMap<(RepoKind, String), CommitId> {
        let mut out = BTreeMap::new();
        if let Some((name, commit)) = &self.base {
            out.insert((RepoKind::Base, name.clone()), commit.clone());
        }
        for (tenant, commit) in &self.tenants {
            out.insert((RepoKind::Tenant, tenant.as_str().to_owned()), commit.clone());
        }
        out
    }
}

/// Where the heads are now. Two calls to the mirror and no tree.
pub(crate) fn live_heads(git: &dyn GitReader) -> Result<LiveHeads, RegistryError> {
    let mut out = LiveHeads::default();
    // A repository the mirror does not have yet has no branches: nothing is tracked, and every file is untracked until it has.
    let base = match git.branches(RepoKind::Base) {
        Ok(b) => b,
        Err(GitError::NotFound) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    out.base = base
        .into_iter()
        .find(|b| b.kind == BranchKind::Default)
        .map(|b| (b.name.as_str().to_owned(), b.head));
    let tenant = match git.branches(RepoKind::Tenant) {
        Ok(b) => b,
        Err(GitError::NotFound) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    for branch in tenant {
        if branch.kind != BranchKind::Deployable {
            continue;
        }
        if let Ok(tenant) = TenantId::parse(branch.name.as_str()) {
            out.tenants.insert(tenant, branch.head);
        }
    }
    Ok(out)
}

impl GitView {
    /// The view of the heads in `live`, with the trees of the base head and of the heads of `needed` tenants.
    ///
    /// # Errors
    ///
    /// `Git` when the mirror cannot answer, `Capacity` when there are more deployable branches than a tenant set holds.
    pub(crate) fn resolve(
        index: &GitIndex,
        git: &dyn GitReader,
        live: &LiveHeads,
        needed: &BTreeSet<TenantId>,
    ) -> Result<Self, RegistryError> {
        let deployable = TenantSet::new(live.tenants.keys().cloned()).map_err(|_| RegistryError::Capacity)?;
        let base = match &live.base {
            Some((_, commit)) => Some(HeadTree {
                commit: commit.clone(),
                index: git.tree_index(RepoKind::Base, commit)?,
            }),
            None => None,
        };
        let mut tenants = BTreeMap::new();
        for tenant in needed {
            if let Some(commit) = live.tenants.get(tenant) {
                tenants.insert(
                    tenant.clone(),
                    HeadTree {
                        commit: commit.clone(),
                        index: git.tree_index(RepoKind::Tenant, commit)?,
                    },
                );
            }
        }
        Ok(Self {
            index: index.clone(),
            base,
            tenants,
            deployable,
        })
    }

    /// Every deployable branch, the set a name's suffix is read against.
    pub(crate) fn deployable(&self) -> &TenantSet {
        &self.deployable
    }

    /// The commits a swimlane that deploys `lane_tenants` is computed at.
    pub(crate) fn heads(&self, lane_tenants: &TenantSet) -> Heads {
        Heads {
            base: self.base.as_ref().map(|b| b.commit.clone()),
            tenants: lane_tenants
                .iter()
                .filter_map(|t| self.tenants.get(t).map(|h| (t.clone(), h.commit.clone())))
                .collect(),
        }
    }

    /// The tree a file of `tenant` (`None`: a base file) is looked up in. A tenant the swimlane does not deploy has none: the
    /// swimlane's services never read that branch, so Git has nothing to say about the file for this swimlane.
    fn tree(&self, tenant: Option<&TenantId>, lane_tenants: &TenantSet) -> Option<(RepoKind, &HeadTree)> {
        match tenant {
            None => self.base.as_ref().map(|b| (RepoKind::Base, b)),
            Some(t) if lane_tenants.contains(t) => self.tenants.get(t).map(|h| (RepoKind::Tenant, h)),
            Some(_) => None,
        }
    }

    /// Whether Git has the file at `repo_path`, for a swimlane that deploys `lane_tenants`.
    pub(crate) fn is_tracked(
        &self,
        tenant: Option<&TenantId>,
        repo_path: &RepoPath,
        lane_tenants: &TenantSet,
    ) -> bool {
        self.tree(tenant, lane_tenants)
            .is_some_and(|(_, head)| head.index.get(repo_path).is_some())
    }

    /// G: the hash of the file in Git for drift, or `None` when Git has no such file for this swimlane.
    pub(crate) fn hash(
        &self,
        tenant: Option<&TenantId>,
        repo_path: &RepoPath,
        class: FileClass,
        lane_tenants: &TenantSet,
    ) -> Result<Option<ContentHash>, GitError> {
        let Some((repo, head)) = self.tree(tenant, lane_tenants) else {
            return Ok(None);
        };
        self.index.git_hash(repo, &head.commit, repo_path, class)
    }

    /// The commit and the branch a baseline of a file of `tenant` is taken from, when Git has a head for it.
    pub(crate) fn source_of(
        &self,
        tenant: Option<&TenantId>,
        lane_tenants: &TenantSet,
    ) -> Option<(RepoKind, CommitId)> {
        self.tree(tenant, lane_tenants)
            .map(|(repo, head)| (repo, head.commit.clone()))
    }

    /// The NFS paths Git holds for a swimlane that deploys `lane_tenants`: the base default branch's tree and the trees of the
    /// tenants', mapped forward. A path no mapping can place is left out. Sorted, without repeats.
    pub(crate) fn universe(&self, rule: &PathMappingRule, lane_tenants: &TenantSet) -> BTreeSet<NfsPath> {
        let mut out = BTreeSet::new();
        if let Some(base) = &self.base {
            for entry in base.index.entries() {
                if let Ok(path) = rule.base_to_nfs(&entry.path) {
                    out.insert(path);
                }
            }
        }
        for tenant in lane_tenants.iter() {
            if let Some(head) = self.tenants.get(tenant) {
                for entry in head.index.entries() {
                    if let Ok(path) = rule.tenant_to_nfs(&entry.path, tenant) {
                        out.insert(path);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RegistryConfig;
    use crate::index::GitIndexDeps;
    use crate::metrics::RegistryMetrics;
    use ports::fakes::FakeGit;
    use sqlx::postgres::PgPoolOptions;

    fn tenant(name: &str) -> TenantId {
        TenantId::parse(name).unwrap()
    }

    fn tenants(names: &[&str]) -> TenantSet {
        TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
    }

    fn rp(p: &str) -> RepoPath {
        RepoPath::parse(p).unwrap()
    }

    /// A `GitIndex` that never touches its pool (the view's own calls do not), over `git`.
    fn index(git: &FakeGit) -> GitIndex {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        GitIndex::new(GitIndexDeps {
            pool,
            git: Arc::new(git.clone()),
            config: RegistryConfig::default(),
            metrics: RegistryMetrics::default(),
        })
        .unwrap()
    }

    fn repos() -> FakeGit {
        let git = FakeGit::new();
        git.commit(
            RepoKind::Base,
            "master",
            &[
                ("config/app/core.yml", Some(b"a: 1\r\n")),
                ("config/app/pic.bmp", Some(b"BM\r\n")),
                ("README.md", Some(b"x")),
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
        git.commit(
            RepoKind::Tenant,
            "templates/core",
            &[("data/config/t.yml", Some(b"t: 1\n"))],
            "t",
        )
        .unwrap();
        git
    }

    fn view(git: &FakeGit, needed: &[&str]) -> GitView {
        let live = live_heads(git).unwrap();
        let needed: BTreeSet<TenantId> = needed.iter().map(|n| tenant(n)).collect();
        GitView::resolve(&index(git), git, &live, &needed).unwrap()
    }

    #[tokio::test]
    async fn the_live_heads_are_the_default_branch_and_the_deployable_branches_only() {
        let git = repos();
        let live = live_heads(&git).unwrap();
        assert_eq!(live.base.as_ref().map(|b| b.0.as_str()), Some("master"));
        let names: Vec<&str> = live.tenants.keys().map(TenantId::as_str).collect();
        assert_eq!(names, ["sit1", "sit2"], "a template is not deployed");
        let branches = live.branches();
        assert!(branches.contains_key(&(RepoKind::Base, "master".to_owned())));
        assert!(branches.contains_key(&(RepoKind::Tenant, "sit1".to_owned())));
        assert!(!branches.contains_key(&(RepoKind::Tenant, "templates/core".to_owned())));
    }

    #[tokio::test]
    async fn a_missing_repository_has_no_heads_and_tracks_nothing() {
        let git = FakeGit::new();
        let live = live_heads(&git).unwrap();
        assert_eq!(live, LiveHeads::default());
        let v = view(&git, &["sit1"]);
        assert!(!v.is_tracked(None, &rp("config/a.yml"), &tenants(&["sit1"])));
        assert_eq!(
            v.hash(None, &rp("config/a.yml"), FileClass::Structured, &tenants(&[])),
            Ok(None)
        );
        assert!(
            v.universe(&PathMappingRule::default(), &tenants(&["sit1"]))
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_tenant_the_swimlane_does_not_deploy_is_not_looked_up() {
        let git = repos();
        let v = view(&git, &["sit1"]);
        let lane = tenants(&["sit1"]);
        let path = rp("data/config/app/core.yml");
        assert!(v.is_tracked(Some(&tenant("sit1")), &path, &lane));
        assert!(
            !v.is_tracked(Some(&tenant("sit2")), &path, &lane),
            "sit2 has the file, but this swimlane deploys sit1 only"
        );
        assert_eq!(
            v.hash(Some(&tenant("sit2")), &path, FileClass::Structured, &lane),
            Ok(None)
        );
        assert_eq!(v.deployable().len(), 2, "but its suffix is still a tenant suffix");
        assert_eq!(v.heads(&lane).tenants.len(), 1);
    }

    #[tokio::test]
    async fn the_hash_is_the_drift_hash_and_a_binary_file_is_not_read() {
        let git = repos();
        let v = view(&git, &["sit1"]);
        let lane = tenants(&["sit1"]);
        let crlf = v
            .hash(None, &rp("config/app/core.yml"), FileClass::Structured, &lane)
            .unwrap();
        let lf = engine::drift_hash(Some(&bytes::Bytes::from_static(b"a: 1\n")), FileClass::Structured);
        assert_eq!(crlf, lf, "line endings are not drift");
        let raw = v
            .hash(None, &rp("config/app/pic.bmp"), FileClass::Binary, &lane)
            .unwrap();
        assert_eq!(
            raw,
            engine::drift_hash(Some(&bytes::Bytes::from_static(b"BM\r\n")), FileClass::Binary)
        );
        assert_eq!(
            v.hash(None, &rp("config/none.yml"), FileClass::Structured, &lane),
            Ok(None)
        );
    }

    #[tokio::test]
    async fn the_universe_is_the_forward_mapped_trees_of_the_base_and_of_the_swimlanes_tenants() {
        let git = repos();
        let v = view(&git, &["sit1", "sit2"]);
        let rule = PathMappingRule::default();
        let names = |lane: &TenantSet| -> Vec<String> {
            v.universe(&rule, lane)
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect()
        };
        assert_eq!(
            names(&tenants(&["sit1"])),
            ["app/core-sit1.yml", "app/core.yml", "app/pic.bmp"],
            "the README is not configuration, sit2 is not deployed"
        );
        assert_eq!(
            names(&tenants(&["sit1", "sit2"])),
            [
                "app/core-sit1.yml",
                "app/core-sit2.yml",
                "app/core.yml",
                "app/pic.bmp"
            ]
        );
        assert_eq!(names(&tenants(&[])), ["app/core.yml", "app/pic.bmp"]);
    }

    #[tokio::test]
    async fn the_source_of_a_baseline_is_the_repository_and_commit_of_the_file() {
        let git = repos();
        let v = view(&git, &["sit1"]);
        let lane = tenants(&["sit1"]);
        let (repo, commit) = v.source_of(None, &lane).unwrap();
        assert_eq!(repo, RepoKind::Base);
        assert_eq!(Some(commit), v.heads(&lane).base);
        let (repo, _) = v.source_of(Some(&tenant("sit1")), &lane).unwrap();
        assert_eq!(repo, RepoKind::Tenant);
        assert!(v.source_of(Some(&tenant("sit2")), &lane).is_none());
    }
}
