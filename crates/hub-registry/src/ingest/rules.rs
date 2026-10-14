//! What the hub knows about a swimlane's names when a file arrives: the path mapping, the tenants it carries, and the
//! application names of its Deployments. The engine turns them into a logical file, a kind and a role (D33).
//!
//! These are *derived* columns of `file_state`. Ingestion fills them on first sight of a path so that the row is complete; the
//! pipeline (T3) recomputes them whenever the rules change, so a wrong guess here (the tenant set is incomplete before the
//! adoption pass, T5) is repaired there and is never a fact anyone reads.

use domain::{
    AppName, FileKind, FileRole, LogicalFile, NfsPath, PathMappingRule, RepoPath, SwimlaneId, TenantId,
    TenantSet,
};
use engine::{classify_path, file_role};

use crate::db::{Db, Txn};
use crate::error::RegistryError;

/// The most tenants and application names read for one swimlane. Both sets are small; the bound is a guard.
const MAX_NAMES: i64 = 2_000;

#[derive(Debug, Clone)]
pub(crate) struct LaneRules {
    pub rule: PathMappingRule,
    pub tenants: TenantSet,
    pub apps: Vec<AppName>,
}

/// What `file_state` stores for a path the hub sees for the first time.
#[derive(Debug, Clone)]
pub(crate) struct Names {
    pub logical: LogicalFile,
    pub kind: FileKind,
    pub role: FileRole,
}

impl LaneRules {
    /// The names of a path. Git is not asked: whether the repository has the file is the pipeline's to find out, so a file is
    /// a base or tenant file here, by its name alone.
    ///
    /// # Errors
    ///
    /// `Invalid` for the root, which is not a file.
    pub(crate) fn names(&self, path: &NfsPath) -> Result<Names, RegistryError> {
        let (logical, kind) = match classify_path(&self.rule, path, &self.tenants, |_| true) {
            Ok(f) => (f.logical, f.kind),
            // A path the mapping cannot place (it would not fit under a repository root) is its own logical file.
            Err(_) => (
                LogicalFile::from_nfs(path.clone()).map_err(|_| RegistryError::Invalid)?,
                FileKind::Untracked,
            ),
        };
        Ok(Names {
            role: file_role(path, &self.tenants, &self.apps),
            logical,
            kind,
        })
    }
}

/// The path mapping rule from the rows of `path_mappings` (`repo`, `repo_root`): the admin's root for each repository, and the
/// default root (`config`, `data/config`) for one that has no row or a root that is not a valid path.
pub(crate) fn rule_of(roots: &[(String, String)]) -> PathMappingRule {
    let default = PathMappingRule::default();
    let root_of = |repo: &str, fallback: &RepoPath| {
        roots
            .iter()
            .find(|(r, _)| r == repo)
            .and_then(|(_, root)| RepoPath::parse(root).ok())
            .unwrap_or_else(|| fallback.clone())
    };
    PathMappingRule::new(
        root_of("base", default.base_root()),
        root_of("tenant", default.tenant_root()),
    )
}

/// Read the mapping, tenants and application names of a swimlane in the caller's transaction.
pub(crate) async fn load(db: &Db, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<LaneRules, RegistryError> {
    let roots: Vec<(String, String)> = db
        .run(
            "rules_mapping",
            sqlx::query_as("SELECT repo, repo_root FROM path_mappings LIMIT 8").fetch_all(&mut **tx),
        )
        .await?;
    let rule = rule_of(&roots);

    let tenant_ids: Vec<String> = db
        .run(
            "rules_tenants",
            sqlx::query_scalar(
                "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT $2",
            )
            .bind(swimlane.as_str())
            .bind(MAX_NAMES)
            .fetch_all(&mut **tx),
        )
        .await?;
    let tenants = TenantSet::new(tenant_ids.iter().filter_map(|t| TenantId::parse(t).ok()))
        .map_err(|_| RegistryError::Corrupt)?;

    let names: Vec<Option<String>> = db
        .run(
            "rules_apps",
            sqlx::query_scalar(
                "SELECT env_values ->> 'SPRING_APPLICATION_NAME' FROM services \
                 WHERE swimlane_id = $1 AND removed_at IS NULL ORDER BY id LIMIT $2",
            )
            .bind(swimlane.as_str())
            .bind(MAX_NAMES)
            .fetch_all(&mut **tx),
        )
        .await?;
    let mut apps: Vec<AppName> = names
        .into_iter()
        .flatten()
        .filter_map(|n| AppName::parse(&n).ok())
        .collect();
    apps.sort();
    apps.dedup();
    Ok(LaneRules { rule, tenants, apps })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(tenants: &[&str], apps: &[&str]) -> LaneRules {
        LaneRules {
            rule: PathMappingRule::default(),
            tenants: TenantSet::new(tenants.iter().map(|t| TenantId::parse(t).unwrap())).unwrap(),
            apps: apps.iter().map(|a| AppName::parse(a).unwrap()).collect(),
        }
    }

    #[test]
    fn a_tenant_file_and_its_base_file_are_one_logical_file() {
        let r = rules(&["sit1"], &["tx-infinity-api"]);
        let base = r
            .names(&NfsPath::parse("tx-infinity-api/tx-infinity-core.yml").unwrap())
            .unwrap();
        let tenant = r
            .names(&NfsPath::parse("tx-infinity-api/tx-infinity-core-sit1.yml").unwrap())
            .unwrap();
        assert_eq!(base.kind, FileKind::Base);
        assert_eq!(tenant.kind, FileKind::Tenant);
        assert_eq!(base.logical, tenant.logical);
        assert_eq!(base.logical.as_str(), "tx-infinity-api/tx-infinity-core.yml");
    }

    #[test]
    fn the_admins_roots_replace_the_defaults_and_a_bad_root_falls_back() {
        let rows = vec![
            ("base".to_owned(), "cfg".to_owned()),
            ("tenant".to_owned(), "../escape".to_owned()),
        ];
        let rule = rule_of(&rows);
        assert_eq!(rule.base_root().as_str(), "cfg");
        assert_eq!(
            rule.tenant_root().as_str(),
            "data/config",
            "a root that is not a valid path is not used"
        );
        assert_eq!(rule_of(&[]), PathMappingRule::default());
    }

    #[test]
    fn the_root_is_not_a_file() {
        assert_eq!(
            rules(&[], &[]).names(&NfsPath::root()).unwrap_err(),
            RegistryError::Invalid
        );
    }

    #[test]
    fn a_hyphen_in_a_name_is_not_a_tenant() {
        let r = rules(&["sit1"], &[]);
        let n = r
            .names(&NfsPath::parse("svc/tx-infinity-core.yml").unwrap())
            .unwrap();
        assert_eq!(n.kind, FileKind::Base);
        assert_eq!(n.logical.as_str(), "svc/tx-infinity-core.yml");
    }

    #[test]
    fn the_role_follows_the_name_and_the_application_names() {
        let r = rules(&["sit1"], &["tx-infinity-api"]);
        assert_eq!(
            r.names(&NfsPath::parse("application.yml").unwrap()).unwrap().role,
            FileRole::PropertySource
        );
        assert_eq!(
            r.names(&NfsPath::parse("tx-infinity-api/tx-infinity-api-sit1.yml").unwrap())
                .unwrap()
                .role,
            FileRole::PropertySource
        );
        assert_eq!(
            r.names(&NfsPath::parse("report/layout.xsl").unwrap())
                .unwrap()
                .role,
            FileRole::Resource
        );
    }
}
