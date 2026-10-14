//! Ranking the deployable branches against a swimlane (T4, T5, D22, P6): which branch of the tenant repository does the swimlane's NFS look
//! like?
//!
//! One implementation for both producers of a ranking: the tenant-suffix review of the pipeline (`pipeline::tenant_rematch`, T4) and the
//! adoption pass (T5), so that they cannot disagree about what a ranking is.
//!
//! The work is `engine::adoption_rank` on the registry's rayon pool over the **tree indexes** of the mirror (`GitReader::tree_index`, which
//! the platform builds once per commit and caches): the path and the SHA-256 of every file of every deployable branch, with no blob
//! read. The swimlane's side is its `file_state` rows (path and raw hash), so nothing is read from Git or the blob store at all. A
//! template, the docs branch and any branch the registry's rule does not make deployable are never ranked ([`crate::branches`]).
//!
//! Both sides are compared by the raw SHA-256: a swimlane whose files have other line endings than Git's scores lower on every branch
//! alike, and the order, which is what an admin is shown, is what matters.
//!
//! The whole ranking is returned (a branch with nothing in common last): the caller keeps the top of it, and the hints look at every
//! deployable branch.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use domain::{
    BranchKind, CommitId, ContentHash, GitRef, NfsPath, RepoKind, RepoPath, ReverseMapping, SwimlaneId,
    TenantId, TenantSet,
};
use engine::{AdoptionCandidate, BatchError, adoption_rank};
use ports::GitReader;
use serde_json::{Value, json};

use crate::branches::BranchClassifier;
use crate::db::Db;
use crate::error::RegistryError;
use crate::ingest::rules::rule_of;
use crate::pipeline::blocking;

/// The most files of a swimlane read for the ranking.
const MAX_FILES: i64 = 200_001;

/// One deployable branch and how well it fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ranked {
    pub branch: GitRef,
    pub tenant: TenantId,
    pub head: CommitId,
    pub shared: u64,
    pub union: u64,
}

impl Ranked {
    /// The four keys a stored ranking has (`adoptions.ranking`): the review of T4 and the pass of T5 write the same.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "branch": self.branch.as_str(),
            "tenant": self.tenant.as_str(),
            "shared": self.shared,
            "union": self.union,
        })
    }
}

/// What the ranking needs: the database for the swimlane's files and the rules, the mirror for the branches, the pool for the work.
#[derive(Clone)]
pub(crate) struct Ranker {
    pub db: Db,
    pub git: Arc<dyn GitReader>,
    pub pool: Arc<rayon::ThreadPool>,
    pub timeout: Duration,
}

impl Ranker {
    /// Every deployable branch, best first, ties by branch name (the engine's order).
    pub(crate) async fn rank(&self, swimlane: &SwimlaneId) -> Result<Vec<Ranked>, RegistryError> {
        let roots: Vec<(String, String)> = self
            .db
            .run(
                "rank_mapping",
                sqlx::query_as("SELECT repo, repo_root FROM path_mappings ORDER BY repo LIMIT 8")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let rule = rule_of(&roots);
        let classifier = BranchClassifier::load(&self.db).await?;
        let files: Vec<(String, Vec<u8>)> = self
            .db
            .run(
                "rank_files",
                sqlx::query_as(
                    "SELECT path, nfs_hash::bytea FROM file_state \
                     WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL ORDER BY path LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(MAX_FILES)
                .fetch_all(self.db.pool()),
            )
            .await?;
        if files.len() >= usize::try_from(MAX_FILES).unwrap_or(usize::MAX) {
            return Err(RegistryError::Capacity);
        }
        let (git, pool) = (Arc::clone(&self.git), Arc::clone(&self.pool));
        blocking(self.timeout.max(Duration::from_secs(60)), move || {
            let branches = git.branches(RepoKind::Tenant)?;
            let mut deployable: Vec<(GitRef, TenantId, CommitId)> = Vec::new();
            for branch in branches {
                if let Some(tenant) = classifier.tenant_of(branch.name.as_str()) {
                    deployable.push((branch.name, tenant, branch.head));
                }
            }
            let set = TenantSet::new(deployable.iter().map(|(_, t, _)| t.clone()))
                .map_err(|_| RegistryError::Capacity)?;
            // The swimlane's side: the files whose names carry the suffix of a deployable branch.
            let mut ours: Vec<(NfsPath, ContentHash)> = Vec::new();
            for (path, hash) in files {
                let (Ok(path), Ok(hash)) = (NfsPath::parse(&path), <[u8; 32]>::try_from(hash)) else {
                    continue;
                };
                if matches!(rule.reverse(&path, &set), Ok(ReverseMapping::Tenant { .. })) {
                    ours.push((path, ContentHash::from_bytes(hash)));
                }
            }
            // The branches' side: every file at the head, by the hash the tree index holds.
            let mut trees: Vec<Vec<(RepoPath, ContentHash)>> = Vec::with_capacity(deployable.len());
            for (_, _, head) in &deployable {
                let index = git.tree_index(RepoKind::Tenant, head)?;
                trees.push(
                    index
                        .entries()
                        .iter()
                        .map(|e| (e.path.clone(), e.sha256))
                        .collect(),
                );
            }
            let candidates: Vec<AdoptionCandidate<'_>> = deployable
                .iter()
                .zip(&trees)
                .map(|((name, tenant, _), files)| AdoptionCandidate {
                    branch: name,
                    kind: BranchKind::Deployable,
                    tenant,
                    files,
                })
                .collect();
            let scores = adoption_rank(&pool, &rule, &ours, &candidates).map_err(|e| match e {
                BatchError::TooManyCandidates | BatchError::TooManyFiles => RegistryError::Capacity,
                _ => RegistryError::Invalid,
            })?;
            let heads: BTreeMap<&GitRef, &CommitId> =
                deployable.iter().map(|(name, _, head)| (name, head)).collect();
            Ok(scores
                .iter()
                .filter_map(|s| {
                    Some(Ranked {
                        branch: s.branch.clone(),
                        tenant: s.tenant.clone(),
                        head: (*heads.get(&s.branch)?).clone(),
                        shared: s.similarity.shared(),
                        union: s.similarity.union(),
                    })
                })
                .collect())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ranked_branch_is_stored_with_the_four_keys_of_the_review() {
        let r = Ranked {
            branch: GitRef::parse("sit2").unwrap(),
            tenant: TenantId::parse("sit2").unwrap(),
            head: CommitId::parse(&"a".repeat(40)).unwrap(),
            shared: 3,
            union: 5,
        };
        let json = r.to_json();
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["branch", "shared", "tenant", "union"], "and no head");
        assert_eq!(json["shared"], 3);
        assert_eq!(json["tenant"], "sit2");
    }
}
