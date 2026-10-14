//! The hints beside the ranking (T5, Q19, D88): what the cluster says about which tenant a swimlane runs.
//!
//! Two sources, both **hints and never an authority** (Q19):
//!
//! - **Environment.** `SPRING_PROFILES_ACTIVE` is one of the four variables whose value the agent reports (D88). The profile list is
//!   `<tenant>,default` (D82), so a profile that is the id of a deployable branch names a tenant. Only the allowlisted value is ever
//!   stored, and what a hint keeps is the service, the variable's name and the tenant it names, not the value.
//! - **Release.** The tenant data is deployed as the Helm chart `csp-tenant-data-<branch>` (D84), and the agent reports the chart label
//!   (`csp-tenant-data-sit12-0.3.1`) and the release's instance name (`csp-tenant-data-sit12`) of the Deployments and sync Jobs that carry
//!   one (`release_hints`). A name that is the chart of a deployable branch, with or without a version after it, names that tenant.
//!
//! A tenant id can hold hyphens (`sit1-b`), so `csp-tenant-data-sit1-b-1.0.0` could be tenant `sit1` at version `b-1.0.0` or `sit1-b`
//! at `1.0.0`. The longest tenant whose name is followed by a version (a digit, or `v` and a digit) wins.

use std::collections::BTreeSet;

use domain::{SwimlaneId, TenantId};
use serde_json::{Value, json};

use super::Inner;
use crate::error::RegistryError;

/// The chart name the tenant data is deployed under (D84).
const CHART_PREFIX: &str = "csp-tenant-data-";
/// The environment variable whose value is `<tenant>,default` (D82, D88).
const PROFILES: &str = "SPRING_PROFILES_ACTIVE";
/// The most services and release hints read for one swimlane (the cluster report bounds both well below).
const MAX_SERVICES: i64 = 2_000;
const MAX_RELEASE_HINTS: i64 = 10_000;

/// Where a hint comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HintSource {
    /// A Deployment's `SPRING_PROFILES_ACTIVE`.
    Env,
    /// A Helm chart or instance name of a Deployment or Job.
    Release,
}

impl HintSource {
    const fn key(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Release => "release",
        }
    }
}

/// A fact from the cluster that names a deployable branch. Never the value it was read from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hint {
    pub source: HintSource,
    /// `namespace/name` of the Deployment or Job.
    pub service: String,
    /// The environment variable, or the Helm label, that held it.
    pub key: String,
    /// The deployable branch it names.
    pub tenant: TenantId,
}

impl Hint {
    pub(crate) fn to_json(&self) -> Value {
        json!({"service": self.service, "key": self.key, "tenant": self.tenant.as_str()})
    }

    pub(crate) fn from_json(source: &str, v: &Value) -> Option<Self> {
        let text = |key: &str| v.get(key).and_then(Value::as_str);
        Some(Self {
            source: if source == "env" {
                HintSource::Env
            } else {
                HintSource::Release
            },
            service: text("service")?.to_owned(),
            key: text("key")?.to_owned(),
            tenant: TenantId::parse(text("tenant")?).ok()?,
        })
    }
}

/// The tenants that a profile list names: the members of `a,b,c` that are deployable branches.
fn env_tenants<'a>(value: &str, candidates: &'a BTreeSet<TenantId>) -> Vec<&'a TenantId> {
    value
        .split(',')
        .map(str::trim)
        .filter_map(|profile| TenantId::parse(profile).ok())
        .filter_map(|profile| candidates.get(&profile))
        .collect()
}

/// Whether `rest` (what follows a tenant id and a hyphen in a chart name) is a version.
fn is_version(rest: &str) -> bool {
    let rest = rest.strip_prefix('v').unwrap_or(rest);
    rest.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

/// The tenant a Helm chart or instance name names, if it is the chart of a deployable branch.
fn release_tenant<'a>(value: &str, candidates: &'a BTreeSet<TenantId>) -> Option<&'a TenantId> {
    let rest = value.strip_prefix(CHART_PREFIX)?;
    candidates
        .iter()
        .filter(|tenant| {
            let name = tenant.as_str();
            rest == name
                || rest
                    .strip_prefix(name)
                    .and_then(|after| after.strip_prefix('-'))
                    .is_some_and(is_version)
        })
        .max_by_key(|tenant| tenant.as_str().len())
}

/// The hints that name a deployable branch, from the profiles of the Deployments `(namespace, name, SPRING_PROFILES_ACTIVE)` and the
/// release hints `(namespace, service, key, value)`. Sorted, without repeats, at most `max`.
pub(crate) fn compute(
    candidates: &BTreeSet<TenantId>,
    profiles: &[(String, String, String)],
    releases: &[(String, String, String, String)],
    max: usize,
) -> Vec<Hint> {
    let mut found: BTreeSet<Hint> = BTreeSet::new();
    for (namespace, name, value) in profiles {
        for tenant in env_tenants(value, candidates) {
            found.insert(Hint {
                source: HintSource::Env,
                service: format!("{namespace}/{name}"),
                key: PROFILES.to_owned(),
                tenant: tenant.clone(),
            });
        }
    }
    for (namespace, service, key, value) in releases {
        if let Some(tenant) = release_tenant(value, candidates) {
            found.insert(Hint {
                source: HintSource::Release,
                service: format!("{namespace}/{service}"),
                key: key.clone(),
                tenant: tenant.clone(),
            });
        }
    }
    found.into_iter().take(max).collect()
}

/// Hints as the JSON a row stores: `{"env": [...], "release": [...]}`, merged into what the row has already (the review of T4 keeps
/// its own keys there).
pub(crate) fn merge_into(stored: &Value, hints: &[Hint]) -> Value {
    let mut object = stored.as_object().cloned().unwrap_or_default();
    for source in [HintSource::Env, HintSource::Release] {
        let list: Vec<Value> = hints
            .iter()
            .filter(|h| h.source == source)
            .map(Hint::to_json)
            .collect();
        object.insert(source.key().to_owned(), Value::Array(list));
    }
    Value::Object(object)
}

impl Inner {
    /// The hints for a swimlane, against the deployable branches `candidates`.
    pub(crate) async fn hints(
        &self,
        swimlane: &SwimlaneId,
        candidates: &BTreeSet<TenantId>,
    ) -> Result<Vec<Hint>, RegistryError> {
        let profiles: Vec<(String, String, String)> = self
            .db
            .run(
                "adoption_env_hints",
                sqlx::query_as(
                    "SELECT namespace, name, env_values ->> 'SPRING_PROFILES_ACTIVE' FROM services \
                     WHERE swimlane_id = $1 AND removed_at IS NULL AND env_values ? 'SPRING_PROFILES_ACTIVE' \
                     ORDER BY namespace, name LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(MAX_SERVICES)
                .fetch_all(self.db.pool()),
            )
            .await?;
        let releases: Vec<(String, String, String, String)> = self
            .db
            .run(
                "adoption_release_hints",
                sqlx::query_as(
                    "SELECT namespace, service, key, value FROM release_hints WHERE swimlane_id = $1 \
                     ORDER BY namespace, service, key LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(MAX_RELEASE_HINTS)
                .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(compute(
            candidates,
            &profiles,
            &releases,
            self.config.adoption_hints,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenants(names: &[&str]) -> BTreeSet<TenantId> {
        names.iter().map(|n| TenantId::parse(n).unwrap()).collect()
    }

    fn profile(ns: &str, name: &str, value: &str) -> (String, String, String) {
        (ns.to_owned(), name.to_owned(), value.to_owned())
    }

    fn release(ns: &str, svc: &str, key: &str, value: &str) -> (String, String, String, String) {
        (ns.to_owned(), svc.to_owned(), key.to_owned(), value.to_owned())
    }

    #[test]
    fn a_profile_that_is_a_deployable_branch_names_a_tenant_and_default_does_not() {
        let c = tenants(&["sit1", "sit12"]);
        let hints = compute(
            &c,
            &[
                profile("csp", "api", "sit12,default"),
                profile("csp", "web", " sit1 , default "),
                profile("csp", "batch", "default"),
                profile("csp", "other", "sit99,default"),
            ],
            &[],
            100,
        );
        let named: Vec<(&str, &str)> = hints
            .iter()
            .map(|h| (h.service.as_str(), h.tenant.as_str()))
            .collect();
        assert_eq!(named, vec![("csp/api", "sit12"), ("csp/web", "sit1")]);
        assert!(
            hints
                .iter()
                .all(|h| h.source == HintSource::Env && h.key == "SPRING_PROFILES_ACTIVE")
        );
    }

    #[test]
    fn a_chart_name_names_its_tenant_whole_and_never_a_longer_one_that_starts_alike() {
        let c = tenants(&["sit1", "sit10", "sit1-b"]);
        let pick = |value: &str| release_tenant(value, &c).map(TenantId::as_str);
        assert_eq!(pick("csp-tenant-data-sit1-0.3.1"), Some("sit1"));
        assert_eq!(pick("csp-tenant-data-sit10-0.3.1"), Some("sit10"));
        assert_eq!(
            pick("csp-tenant-data-sit10"),
            Some("sit10"),
            "the instance name has no version"
        );
        assert_eq!(pick("csp-tenant-data-sit1"), Some("sit1"));
        assert_eq!(
            pick("csp-tenant-data-sit1-b-1.0.0"),
            Some("sit1-b"),
            "the longest name that is followed by a version"
        );
        assert_eq!(
            pick("csp-tenant-data-sit1-v2.0.0"),
            Some("sit1"),
            "a `v` before the version is allowed"
        );
        assert_eq!(
            pick("csp-tenant-data-sit2-0.3.1"),
            None,
            "not a deployable branch"
        );
        assert_eq!(pick("csp-tenant-data-sit1x"), None);
        assert_eq!(
            pick("tenant-data-sit1-0.3.1"),
            None,
            "not the chart of the tenant data"
        );
        assert_eq!(pick("csp-tenant-data-"), None);
        assert_eq!(pick(""), None);
    }

    #[test]
    fn release_hints_name_the_service_the_label_and_the_tenant_only() {
        let c = tenants(&["sit12"]);
        let hints = compute(
            &c,
            &[],
            &[
                release(
                    "csp",
                    "tenant-data",
                    "helm.sh/chart",
                    "csp-tenant-data-sit12-0.3.1",
                ),
                release("csp", "tenant-data", "app.kubernetes.io/instance", "inst"),
                release("csp", "svc", "helm.sh/chart", "something-else-1.0.0"),
            ],
            100,
        );
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].source, HintSource::Release);
        assert_eq!(hints[0].service, "csp/tenant-data");
        assert_eq!(hints[0].key, "helm.sh/chart");
        assert_eq!(hints[0].tenant.as_str(), "sit12");
        // What is kept does not include the value the hint was read from.
        assert!(!hints[0].to_json().to_string().contains("0.3.1"));
    }

    #[test]
    fn hints_are_sorted_deduplicated_and_bounded() {
        let c = tenants(&["sit1", "sit2"]);
        let profiles: Vec<_> = (0..50)
            .map(|i| profile("csp", &format!("svc{i:02}"), "sit1,sit2"))
            .chain([profile("csp", "svc00", "sit1,sit2")])
            .collect();
        let all = compute(&c, &profiles, &[], 10_000);
        assert_eq!(
            all.len(),
            100,
            "two for each of 50 services, the repeat counts once"
        );
        assert!(all.windows(2).all(|w| w[0] < w[1]), "sorted and without repeats");
        let cut = compute(&c, &profiles, &[], 7);
        assert_eq!(cut.len(), 7);
        assert_eq!(cut, all[..7].to_vec());
    }

    #[test]
    fn hints_merge_into_what_a_row_has_and_read_back() {
        let c = tenants(&["sit12"]);
        let hints = compute(
            &c,
            &[profile("csp", "api", "sit12,default")],
            &[release(
                "csp",
                "t",
                "helm.sh/chart",
                "csp-tenant-data-sit12-1.0.0",
            )],
            10,
        );
        let stored = json!({"foreign_suffixes": ["sit2"], "swimlane_tenants": ["sit1"]});
        let merged = merge_into(&stored, &hints);
        assert_eq!(
            merged["foreign_suffixes"],
            json!(["sit2"]),
            "the review's keys stay"
        );
        assert_eq!(merged["swimlane_tenants"], json!(["sit1"]));
        let read: Vec<Hint> = ["env", "release"]
            .iter()
            .flat_map(|key| {
                merged[*key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|h| Hint::from_json(key, h))
            })
            .collect();
        assert_eq!(read, hints);
        // A row with no hints yet (or a malformed one) takes them as well.
        assert_eq!(
            merge_into(&Value::Null, &hints)["env"].as_array().unwrap().len(),
            1
        );
    }
}
