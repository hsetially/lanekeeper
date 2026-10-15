//! What a swimlane is computed under, and the fingerprint of it (T3, plan "Pipeline").
//!
//! The engine's answer for a swimlane depends on more than its files: the tenants it deploys, the applications its Deployments run
//! (they decide which files are property sources), the environment tier and the admin's host rules (C6), the path mapping (which
//! repository path a file is, so whether Git has it), and the set of deployable branches (which suffix reads as a tenant). When any
//! of them changes, every path of the swimlane may change, so the pipeline recomputes the whole swimlane and not only the paths
//! of an event. The **fingerprint** names the combination: it is stored in `swimlane_registry.rules_fingerprint` and kept in memory
//! with the swimlane's state, and a difference between the two, or between the memory and the database now, is "recompute everything".
//!
//! Merge modes are not an input: since D82 the mode follows from a file's role, which the application names decide, so the
//! `merge_modes` table can not change an engine answer and is left out of the fingerprint (a rule that changes nothing must not cost
//! a swimlane recompute). The severity rules are an input: the defaults and the admin's rows of `severity_rules`, read with the rest of the
//! rules of each batch, through the fingerprint of the set that was in force ([`crate::severity`]).
//!
//! Everything is read for all the swimlanes of a batch at once (one statement per kind, `= ANY`), so the cost of a batch does not
//! grow with its number of swimlanes.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{AppName, ContentHash, PathMappingRule, ShortText, SwimlaneId, TenantId, TenantSet};
use engine::{DenyMatcher, HostMatcher, HostRule, HostRules, SeverityRules};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::db::Db;
use crate::error::RegistryError;
use crate::ingest::rules::rule_of;
use crate::severity::SeverityRuleSource;

/// Bump it when the pipeline's own logic changes what it stores for the same inputs, so that the next run recomputes everything.
/// 2: `file_state.severity` and the severity of the observation a row points at are written (T9); rows from before have none.
pub(crate) const RULES_VERSION: u8 = 2;

/// The most swimlanes, tenants, application names and host rules read in one batch. All of them are small; the bounds are guards.
const MAX_SWIMLANES: i64 = 10_000;
const MAX_ROWS: i64 = 200_000;

/// The rules every swimlane shares.
#[derive(Clone)]
pub(crate) struct GlobalRules {
    pub rule: PathMappingRule,
    pub hosts: Arc<HostRules>,
    pub deny: Arc<DenyMatcher>,
    /// The severity rules in force for this batch: the engine's defaults and the admin's. A file's severity and a finding's are rated with
    /// these, and the set is part of `fingerprint`, so a swimlane computed under other rules is computed again.
    pub severity: Arc<SeverityRules>,
    /// A hash of everything above and of the severity rules, so that a swimlane's fingerprint needs only its own parts.
    fingerprint: [u8; 32],
}

impl std::fmt::Debug for GlobalRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalRules").finish_non_exhaustive()
    }
}

#[cfg(test)]
impl GlobalRules {
    /// Rules for a test that does not read them from a database.
    pub(crate) fn for_tests(rule: PathMappingRule) -> Self {
        Self {
            rule,
            hosts: Arc::new(HostRules::default()),
            deny: Arc::new(DenyMatcher::default()),
            severity: Arc::new(SeverityRules::new(engine::default_rules()).unwrap()),
            fingerprint: [7; 32],
        }
    }
}

/// The rules of one swimlane.
#[derive(Debug, Clone)]
pub(crate) struct LaneRules {
    pub swimlane: SwimlaneId,
    pub global: Arc<GlobalRules>,
    pub tenants: TenantSet,
    /// Sorted and without repeats.
    pub apps: Vec<AppName>,
    pub tier: Option<ShortText>,
    /// An admin has confirmed the baseline (D2): from then on the pipeline sets a baseline when NFS and Git converge.
    pub adopted: bool,
    pub fingerprint: ContentHash,
}

/// A hash that cannot be confused by moving a byte from one field to the next: every field has a tag and a length.
struct Fingerprint(Sha256);

impl Fingerprint {
    fn new(tag: &str) -> Self {
        let mut f = Self(Sha256::new());
        f.field("tag", tag.as_bytes());
        f
    }

    fn field(&mut self, name: &str, value: &[u8]) {
        self.0.update((name.len() as u64).to_be_bytes());
        self.0.update(name.as_bytes());
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// How the admin's `host_tiers` pattern recognises a host: a pattern that starts with a dot is a suffix (`.oktapreview.com`), any
/// other is a piece of the name (`-dev`). The same two shapes the engine's rules have.
pub(crate) fn host_rule_of(pattern: &str, tier: &str) -> Option<HostRule> {
    let tier = ShortText::parse(tier).ok()?;
    let matcher = if pattern.starts_with('.') {
        HostMatcher::Suffix(pattern.to_owned())
    } else {
        HostMatcher::Contains(pattern.to_owned())
    };
    Some(HostRule { tier, matcher })
}

/// The host rules from the rows of `host_tiers`, in order. A row the engine refuses is skipped and counted in the log, not fatal:
/// one bad pattern must not stop every swimlane from being computed.
pub(crate) fn host_rules_of(rows: &[(String, String)]) -> HostRules {
    let rules: Vec<HostRule> = rows
        .iter()
        .filter_map(|(pattern, tier)| host_rule_of(pattern, tier))
        .collect();
    let given = rules.len();
    // Try them all; if the set is refused, drop the rules one by one that the engine refuses on their own.
    if let Ok(set) = HostRules::new(rules.clone()) {
        return set;
    }
    let valid: Vec<HostRule> = rules
        .into_iter()
        .filter(|r| HostRules::new(vec![r.clone()]).is_ok())
        .collect();
    warn!(
        rows = rows.len(),
        given,
        kept = valid.len(),
        "host rules were refused by the engine and skipped"
    );
    HostRules::new(valid).unwrap_or_default()
}

fn host_fingerprint(rows: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (pattern, tier) in rows {
        out.extend_from_slice(&(pattern.len() as u64).to_be_bytes());
        out.extend_from_slice(pattern.as_bytes());
        out.extend_from_slice(&(tier.len() as u64).to_be_bytes());
        out.extend_from_slice(tier.as_bytes());
    }
    out
}

/// Reads the rules from the database.
#[derive(Clone)]
pub(crate) struct RuleSource {
    db: Db,
    deny: Arc<DenyMatcher>,
    /// The admin's deny globs, for the fingerprint (the compiled matcher cannot be hashed).
    deny_globs: Vec<String>,
    /// The severity rules: the engine's defaults and the admin's rows, read with the rest of the rules of each batch.
    severity: SeverityRuleSource,
}

impl RuleSource {
    pub(crate) fn new(
        db: Db,
        deny: Arc<DenyMatcher>,
        deny_globs: Vec<String>,
        severity: SeverityRuleSource,
    ) -> Self {
        Self {
            db,
            deny,
            deny_globs,
            severity,
        }
    }

    /// The rules every swimlane shares, with the deployable branches the Git mirror has.
    pub(crate) async fn global(&self, deployable: TenantSet) -> Result<Arc<GlobalRules>, RegistryError> {
        let roots: Vec<(String, String)> = self
            .db
            .run(
                "rules_mapping",
                sqlx::query_as("SELECT repo, repo_root FROM path_mappings ORDER BY repo LIMIT 8")
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let hosts: Vec<(String, String)> = self
            .db
            .run(
                "rules_hosts",
                sqlx::query_as("SELECT pattern, tier FROM host_tiers ORDER BY position, id LIMIT $1")
                    .bind(i64::try_from(HostRules::MAX_RULES + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let rule = rule_of(&roots);
        // The rows of `severity_rules` are read with the rest, once for the batch. A table that cannot be read fails the batch like any
        // other rule that cannot be: a swimlane must not be rated with rules nobody chose.
        let severity = self.severity.current().await?;
        let mut fp = Fingerprint::new("global");
        fp.field("version", &[RULES_VERSION]);
        fp.field("base_root", rule.base_root().as_str().as_bytes());
        fp.field("tenant_root", rule.tenant_root().as_str().as_bytes());
        fp.field("hosts", &host_fingerprint(&hosts));
        for glob in &self.deny_globs {
            fp.field("deny", glob.as_bytes());
        }
        for tenant in deployable.iter() {
            fp.field("deployable", tenant.as_str().as_bytes());
        }
        fp.field("severity", severity.fingerprint.as_bytes());
        Ok(Arc::new(GlobalRules {
            rule,
            hosts: Arc::new(host_rules_of(&hosts)),
            deny: Arc::clone(&self.deny),
            severity: Arc::clone(&severity.rules),
            fingerprint: fp.finish(),
        }))
    }

    /// The rules of these swimlanes. A swimlane that does not exist (any more) is not in the answer.
    pub(crate) async fn lanes(
        &self,
        global: &Arc<GlobalRules>,
        swimlanes: &[SwimlaneId],
    ) -> Result<BTreeMap<SwimlaneId, LaneRules>, RegistryError> {
        if swimlanes.is_empty() {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<&str> = swimlanes.iter().map(SwimlaneId::as_str).collect();
        let lanes: Vec<(String, Option<String>, bool)> = self
            .db
            .run(
                "rules_lanes",
                sqlx::query_as(
                    "SELECT id, tier, baseline_confirmed_at IS NOT NULL FROM swimlanes \
                     WHERE id = ANY($1::text[]) ORDER BY id LIMIT $2",
                )
                .bind(&ids)
                .bind(MAX_SWIMLANES)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let tenants: Vec<(String, String)> = self
            .db
            .run(
                "rules_tenants",
                sqlx::query_as(
                    "SELECT swimlane_id, tenant_id FROM swimlane_tenants \
                     WHERE swimlane_id = ANY($1::text[]) ORDER BY swimlane_id, tenant_id LIMIT $2",
                )
                .bind(&ids)
                .bind(MAX_ROWS)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let apps: Vec<(String, Option<String>)> = self
            .db
            .run(
                "rules_apps",
                sqlx::query_as(
                    "SELECT swimlane_id, env_values ->> 'SPRING_APPLICATION_NAME' FROM services \
                     WHERE swimlane_id = ANY($1::text[]) AND removed_at IS NULL ORDER BY swimlane_id, id LIMIT $2",
                )
                .bind(&ids)
                .bind(MAX_ROWS)
                .fetch_all(self.db.pool()),
            )
            .await?;

        let mut tenant_ids: BTreeMap<String, Vec<TenantId>> = BTreeMap::new();
        for (lane, tenant) in tenants {
            if let Ok(t) = TenantId::parse(&tenant) {
                tenant_ids.entry(lane).or_default().push(t);
            }
        }
        let mut app_names: BTreeMap<String, Vec<AppName>> = BTreeMap::new();
        for (lane, name) in apps {
            if let Some(name) = name.and_then(|n| AppName::parse(&n).ok()) {
                app_names.entry(lane).or_default().push(name);
            }
        }
        let mut out = BTreeMap::new();
        for (id, tier, adopted) in lanes {
            let Ok(swimlane) = SwimlaneId::parse(&id) else {
                return Err(RegistryError::Corrupt);
            };
            let tenants = TenantSet::new(tenant_ids.remove(&id).unwrap_or_default())
                .map_err(|_| RegistryError::Corrupt)?;
            let mut apps = app_names.remove(&id).unwrap_or_default();
            apps.sort();
            apps.dedup();
            let tier = tier.and_then(|t| ShortText::parse(&t).ok());
            let fingerprint = lane_fingerprint(global, &tenants, &apps, tier.as_ref(), adopted);
            out.insert(
                swimlane.clone(),
                LaneRules {
                    swimlane,
                    global: Arc::clone(global),
                    tenants,
                    apps,
                    tier,
                    adopted,
                    fingerprint,
                },
            );
        }
        Ok(out)
    }
}

/// The fingerprint of a swimlane's rules: the shared rules, its tenants, its application names and its tier.
///
/// Whether an admin has adopted the swimlane is part of it: from then on convergence sets baselines, so the paths that were already
/// converged have to be looked at again, whether or not anyone told the pipeline.
pub(crate) fn lane_fingerprint(
    global: &GlobalRules,
    tenants: &TenantSet,
    apps: &[AppName],
    tier: Option<&ShortText>,
    adopted: bool,
) -> ContentHash {
    let mut fp = Fingerprint::new("lane");
    fp.field("global", &global.fingerprint);
    for tenant in tenants.iter() {
        fp.field("tenant", tenant.as_str().as_bytes());
    }
    for app in apps {
        fp.field("app", app.as_str().as_bytes());
    }
    fp.field("tier", tier.map_or("", ShortText::as_str).as_bytes());
    fp.field("adopted", &[u8::from(adopted)]);
    ContentHash::from_bytes(fp.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> GlobalRules {
        GlobalRules::for_tests(PathMappingRule::default())
    }

    fn tenants(names: &[&str]) -> TenantSet {
        TenantSet::new(names.iter().map(|t| TenantId::parse(t).unwrap())).unwrap()
    }

    fn apps(names: &[&str]) -> Vec<AppName> {
        names.iter().map(|n| AppName::parse(n).unwrap()).collect()
    }

    #[test]
    fn the_same_rules_give_the_same_fingerprint_and_each_input_changes_it() {
        let g = global();
        let base = lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&["core"]), None, false);
        assert_eq!(
            base,
            lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&["core"]), None, false)
        );
        let tier = ShortText::parse("sit").unwrap();
        for other in [
            lane_fingerprint(&g, &tenants(&["sit1", "sit2"]), &apps(&["core"]), None, false),
            lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&["core", "api"]), None, false),
            lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&["core"]), Some(&tier), false),
            lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&["core"]), None, true),
            lane_fingerprint(
                &GlobalRules {
                    fingerprint: [8; 32],
                    ..g.clone()
                },
                &tenants(&["sit1"]),
                &apps(&["core"]),
                None,
                false,
            ),
        ] {
            assert_ne!(base, other);
        }
    }

    #[test]
    fn a_byte_moved_from_one_field_to_the_next_is_another_fingerprint() {
        let g = global();
        let ab_c = lane_fingerprint(&g, &tenants(&[]), &apps(&["ab", "c"]), None, false);
        let a_bc = lane_fingerprint(&g, &tenants(&[]), &apps(&["a", "bc"]), None, false);
        assert_ne!(ab_c, a_bc, "fields are length-prefixed");
        let as_tenant = lane_fingerprint(&g, &tenants(&["sit1"]), &apps(&[]), None, false);
        let as_app = lane_fingerprint(&g, &tenants(&[]), &apps(&["sit1"]), None, false);
        assert_ne!(
            as_tenant, as_app,
            "a tenant and an application of the same name are not the same input"
        );
    }

    #[test]
    fn a_host_pattern_with_a_leading_dot_is_a_suffix_and_any_other_is_a_piece_of_the_name() {
        let rows = vec![
            (".oktapreview.com".to_owned(), "test".to_owned()),
            ("-dev".to_owned(), "dev".to_owned()),
        ];
        let rules = host_rules_of(&rows);
        let tier = |host: &str| rules.tier_of(host).map(ShortText::as_str);
        assert_eq!(tier("bank.oktapreview.com"), Some("test"), "a suffix");
        assert_eq!(
            tier("oktapreview.com.evil.org"),
            None,
            "a suffix is the end of the name"
        );
        assert_eq!(tier("api-dev.example.com"), Some("dev"), "a piece of the name");
        assert_eq!(tier("api.example.com"), None);
        assert!(matches!(
            host_rule_of(".okta.com", "prod").map(|r| r.matcher),
            Some(HostMatcher::Suffix(_))
        ));
        assert!(matches!(
            host_rule_of("-uat", "uat").map(|r| r.matcher),
            Some(HostMatcher::Contains(_))
        ));
    }

    #[test]
    fn a_host_row_the_engine_refuses_is_skipped_and_the_others_are_kept() {
        let rows = vec![
            ("-dev".to_owned(), "dev".to_owned()),
            ("not a host!".to_owned(), "dev".to_owned()),
            ("-uat".to_owned(), "uat".to_owned()),
        ];
        let rules = host_rules_of(&rows);
        let tier = |host: &str| rules.tier_of(host).map(ShortText::as_str);
        assert_eq!(tier("a-dev.example.com"), Some("dev"));
        assert_eq!(tier("a-uat.example.com"), Some("uat"));
        assert_eq!(tier("not a host!"), None);
    }

    #[test]
    fn the_host_rows_are_part_of_the_fingerprint_in_order() {
        let a = host_fingerprint(&[("-dev".into(), "dev".into()), ("-uat".into(), "uat".into())]);
        let b = host_fingerprint(&[("-uat".into(), "uat".into()), ("-dev".into(), "dev".into())]);
        assert_ne!(a, b, "the first rule that matches wins, so the order is an input");
    }
}
