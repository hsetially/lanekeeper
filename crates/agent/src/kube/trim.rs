//! Trimming Kubernetes objects on ingest (T6, P4).
//!
//! A watcher receives whole objects: managed fields, annotations, pod templates, status. The agent keeps only what a
//! report can need and drops the object at once, so memory follows the number of Deployments and Pods and not the size
//! of their manifests (P4: 64 MiB). What is kept, and what is deliberately not:
//!
//! - **Deployment:** name, the label selector, the Helm hints if the chart matches, the names of the environment
//!   variables of its containers, and the *value* of a variable only if the hub's allowlist permits it ([`EnvGuard`]).
//!   Never annotations, never `valueFrom` (so never a reference to a Secret), never the image or the command.
//! - **Pod:** name, labels (to find its Deployment), and when it started. A pod that is terminating or has finished is
//!   not kept: it is not serving.

use std::collections::BTreeMap;

use domain::{ShortText, Timestamp};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;

use super::env::EnvGuard;
use super::helm::{HelmFilter, HelmHints};

/// The most variables kept per Deployment.
pub const MAX_ENV_ENTRIES: usize = 1_000;
/// The most labels kept per pod.
pub const MAX_POD_LABELS: usize = 100;
/// The most selector terms kept per Deployment. A selector with more cannot be matched faithfully, so it matches nothing.
pub const MAX_SELECTOR_TERMS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    In,
    NotIn,
    Exists,
    DoesNotExist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Requirement {
    key: String,
    operator: Operator,
    values: Vec<String>,
}

/// A Deployment's label selector: `matchLabels` and `matchExpressions`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selector {
    match_labels: BTreeMap<String, String>,
    requirements: Vec<Requirement>,
    /// The selector could not be kept whole (too many terms, an operator this build does not know).
    unmatchable: bool,
}

impl Selector {
    fn from_api(selector: Option<&LabelSelector>) -> Self {
        let Some(selector) = selector else {
            return Self::default();
        };
        let mut out = Self {
            match_labels: selector.match_labels.clone().unwrap_or_default(),
            ..Self::default()
        };
        let expressions = selector.match_expressions.as_deref().unwrap_or_default();
        if out.match_labels.len() + expressions.len() > MAX_SELECTOR_TERMS {
            out.unmatchable = true;
            out.match_labels.clear();
            return out;
        }
        for expression in expressions {
            let operator = match expression.operator.as_str() {
                "In" => Operator::In,
                "NotIn" => Operator::NotIn,
                "Exists" => Operator::Exists,
                "DoesNotExist" => Operator::DoesNotExist,
                _ => {
                    out.unmatchable = true;
                    continue;
                }
            };
            out.requirements.push(Requirement {
                key: expression.key.clone(),
                operator,
                values: expression.values.clone().unwrap_or_default(),
            });
        }
        out
    }

    /// Does a pod with these labels belong to the Deployment? An empty selector selects nothing: a Deployment always
    /// has one, and "everything" would attach every pod in the namespace to it.
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        if self.unmatchable || (self.match_labels.is_empty() && self.requirements.is_empty()) {
            return false;
        }
        self.match_labels
            .iter()
            .all(|(k, v)| labels.get(k).is_some_and(|have| have == v))
            && self.requirements.iter().all(|r| {
                let have = labels.get(&r.key);
                match r.operator {
                    Operator::In => have.is_some_and(|v| r.values.contains(v)),
                    Operator::NotIn => have.is_none_or(|v| !r.values.contains(v)),
                    Operator::Exists => have.is_some(),
                    Operator::DoesNotExist => have.is_none(),
                }
            })
    }
}

/// What the agent keeps of a Deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentRec {
    pub namespace: String,
    pub name: String,
    pub selector: Selector,
    /// Every variable name of the Deployment's containers. The value is there only when the allowlist permitted it.
    pub env: BTreeMap<String, Option<String>>,
    pub helm: Option<HelmHints>,
}

/// What the agent keeps of a Pod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodRec {
    pub namespace: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub started_at: Timestamp,
}

/// The `(namespace, name)` of an object, with the watched namespace standing in for a missing one.
pub fn key_of(
    meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    namespace: &str,
) -> Option<(String, String)> {
    let name = meta.name.as_deref()?;
    Some((
        meta.namespace.clone().unwrap_or_else(|| namespace.to_owned()),
        name.to_owned(),
    ))
}

pub fn trim_deployment(
    deployment: &Deployment,
    namespace: &str,
    guard: &EnvGuard,
    helm: &HelmFilter,
) -> Option<DeploymentRec> {
    let (namespace, name) = key_of(&deployment.metadata, namespace)?;
    let mut env: BTreeMap<String, Option<String>> = BTreeMap::new();
    let containers = deployment
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .map(|s| s.containers.as_slice())
        .unwrap_or_default();
    for var in containers
        .iter()
        .flat_map(|c| c.env.as_deref().unwrap_or_default())
    {
        if ShortText::parse(&var.name).is_err() {
            continue;
        }
        // `value` only: a `valueFrom` (a Secret or ConfigMap reference) is a name and nothing more.
        let value = var
            .value
            .as_deref()
            .and_then(|v| guard.value_for(&var.name, v))
            .map(|v| v.as_str().to_owned());
        if let Some(slot) = env.get_mut(&var.name) {
            if slot.is_none() {
                *slot = value;
            }
        } else if env.len() < MAX_ENV_ENTRIES {
            env.insert(var.name.clone(), value);
        }
    }
    Some(DeploymentRec {
        namespace,
        name,
        selector: Selector::from_api(deployment.spec.as_ref().map(|s| &s.selector)),
        env,
        helm: helm.hints(deployment.metadata.labels.as_ref()),
    })
}

/// The record for a pod that is up (pending or running, not being deleted), or `None`.
pub fn trim_pod(pod: &Pod, namespace: &str) -> Option<PodRec> {
    let (namespace, name) = key_of(&pod.metadata, namespace)?;
    if pod.metadata.deletion_timestamp.is_some() {
        return None;
    }
    let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
    if matches!(phase, Some("Succeeded" | "Failed")) {
        return None;
    }
    let started = pod
        .status
        .as_ref()
        .and_then(|s| s.start_time.as_ref())
        .or(pod.metadata.creation_timestamp.as_ref())
        .map_or(0, |t| t.0.as_millisecond());
    let labels = pod
        .metadata
        .labels
        .iter()
        .flatten()
        .take(MAX_POD_LABELS)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Some(PodRec {
        namespace,
        name,
        labels,
        started_at: Timestamp::from_unix_millis(started),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn deployment(value: Value) -> Deployment {
        serde_json::from_value(value).unwrap()
    }

    fn pod(value: Value) -> Pod {
        serde_json::from_value(value).unwrap()
    }

    fn guard(names: &[&str]) -> EnvGuard {
        let names: Vec<ShortText> = names.iter().map(|n| ShortText::parse(n).unwrap()).collect();
        EnvGuard::with_allowlist(&names)
    }

    fn helm() -> HelmFilter {
        HelmFilter::new(&[ShortText::parse("csp-tenant-data-*").unwrap()]).unwrap()
    }

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    // The fixtures take their JSON literals by value, which reads best at the call sites.
    #[allow(clippy::needless_pass_by_value)]
    fn manifest(env: Value, selector: Value) -> Deployment {
        deployment(json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {
                "name": "svc", "namespace": "sit1",
                "labels": { "helm.sh/chart": "csp-tenant-data-sit1-1.0.0", "app.kubernetes.io/instance": "inst" },
                "annotations": { "ANNOTATION_MARKER": "x" },
                "managedFields": [{ "manager": "MANAGED_FIELDS_MARKER" }],
            },
            "spec": {
                "selector": selector,
                "template": { "spec": { "containers": [
                    { "name": "a", "image": "IMAGE_MARKER", "env": env },
                    { "name": "b", "image": "IMAGE_MARKER", "env": [{ "name": "ONLY_IN_B", "value": "b" }] },
                ] } },
            },
        }))
    }

    #[test]
    fn a_deployment_keeps_names_and_only_allowlisted_values() {
        let d = manifest(
            json!([
                { "name": "CONFIG_CLIENT_CACHE_TTL", "value": "20m" },
                { "name": "JAVA_OPTS", "value": "-Dsecret.in.plain=hunter2" },
                { "name": "DB_PASSWORD", "value": "hunter2" },
            ]),
            json!({ "matchLabels": { "app": "svc" } }),
        );
        let rec = trim_deployment(
            &d,
            "sit1",
            &guard(&["CONFIG_CLIENT_CACHE_TTL", "DB_PASSWORD"]),
            &helm(),
        )
        .unwrap();
        assert_eq!(
            rec.env,
            BTreeMap::from([
                ("CONFIG_CLIENT_CACHE_TTL".to_owned(), Some("20m".to_owned())),
                ("DB_PASSWORD".to_owned(), None),
                ("JAVA_OPTS".to_owned(), None),
                ("ONLY_IN_B".to_owned(), None),
            ])
        );
    }

    #[test]
    fn a_variable_from_a_secret_is_a_name_and_nothing_more() {
        let d = manifest(
            json!([{ "name": "CONFIG_CLIENT_CACHE_TTL",
                     "valueFrom": { "secretKeyRef": { "name": "SECRET_REF_MARKER", "key": "k" } } }]),
            json!({ "matchLabels": { "app": "svc" } }),
        );
        let rec = trim_deployment(&d, "sit1", &guard(&["CONFIG_CLIENT_CACHE_TTL"]), &helm()).unwrap();
        assert_eq!(rec.env["CONFIG_CLIENT_CACHE_TTL"], None);
        assert!(!format!("{rec:?}").contains("SECRET_REF_MARKER"));
    }

    #[test]
    fn what_is_kept_of_a_deployment_is_small_and_has_no_manifest_text() {
        let d = manifest(
            json!([{ "name": "A", "value": "v" }]),
            json!({ "matchLabels": { "app": "svc" } }),
        );
        let rec = trim_deployment(&d, "sit1", &guard(&[]), &helm()).unwrap();
        let shown = format!("{rec:?}");
        for marker in ["ANNOTATION_MARKER", "MANAGED_FIELDS_MARKER", "IMAGE_MARKER"] {
            assert!(!shown.contains(marker), "{marker} kept in {shown}");
        }
        assert!(shown.len() < 700, "{} bytes: {shown}", shown.len());
    }

    #[test]
    fn the_same_variable_in_two_containers_keeps_a_permitted_value() {
        let d = deployment(json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": { "name": "svc", "namespace": "sit1" },
            "spec": { "selector": {}, "template": { "spec": { "containers": [
                { "name": "a", "env": [{ "name": "V", "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } } }] },
                { "name": "b", "env": [{ "name": "V", "value": "second" }] },
            ] } } },
        }));
        let rec = trim_deployment(&d, "sit1", &guard(&["V"]), &helm()).unwrap();
        assert_eq!(rec.env["V"].as_deref(), Some("second"));
    }

    #[test]
    fn helm_hints_come_from_labels_only_on_a_matching_chart() {
        let d = manifest(json!([]), json!({ "matchLabels": { "app": "svc" } }));
        let rec = trim_deployment(&d, "sit1", &guard(&[]), &helm()).unwrap();
        assert_eq!(rec.helm.unwrap().instance.as_deref(), Some("inst"));
        let other = HelmFilter::new(&[ShortText::parse("something-else-*").unwrap()]).unwrap();
        assert_eq!(
            trim_deployment(&d, "sit1", &guard(&[]), &other).unwrap().helm,
            None
        );
    }

    #[test]
    fn the_namespace_falls_back_to_the_watched_one() {
        let d = deployment(json!({
            "apiVersion": "apps/v1", "kind": "Deployment", "metadata": { "name": "svc" },
            "spec": { "selector": {}, "template": { "spec": { "containers": [] } } },
        }));
        assert_eq!(
            trim_deployment(&d, "sit9", &guard(&[]), &helm())
                .unwrap()
                .namespace,
            "sit9"
        );
        let nameless = deployment(json!({
            "apiVersion": "apps/v1", "kind": "Deployment", "metadata": {},
            "spec": { "selector": {}, "template": { "spec": { "containers": [] } } },
        }));
        assert!(trim_deployment(&nameless, "sit9", &guard(&[]), &helm()).is_none());
    }

    #[test]
    fn env_entries_are_capped() {
        let many: Vec<Value> = (0..MAX_ENV_ENTRIES + 50)
            .map(|i| json!({ "name": format!("V{i:05}"), "value": "x" }))
            .collect();
        let d = manifest(json!(many), json!({ "matchLabels": { "app": "svc" } }));
        assert_eq!(
            trim_deployment(&d, "sit1", &guard(&[]), &helm())
                .unwrap()
                .env
                .len(),
            MAX_ENV_ENTRIES
        );
    }

    #[test]
    fn selector_match_labels_must_all_match() {
        let d = manifest(
            json!([]),
            json!({ "matchLabels": { "app": "svc", "tier": "back" } }),
        );
        let s = trim_deployment(&d, "sit1", &guard(&[]), &helm())
            .unwrap()
            .selector;
        assert!(s.matches(&labels(&[("app", "svc"), ("tier", "back"), ("extra", "x")])));
        assert!(!s.matches(&labels(&[("app", "svc")])));
        assert!(!s.matches(&labels(&[("app", "svc"), ("tier", "front")])));
    }

    #[test]
    fn selector_expressions_follow_kubernetes_semantics() {
        let d = manifest(
            json!([]),
            json!({ "matchExpressions": [
                { "key": "app", "operator": "In", "values": ["svc", "svc2"] },
                { "key": "canary", "operator": "NotIn", "values": ["true"] },
                { "key": "tier", "operator": "Exists" },
                { "key": "debug", "operator": "DoesNotExist" },
            ] }),
        );
        let s = trim_deployment(&d, "sit1", &guard(&[]), &helm())
            .unwrap()
            .selector;
        assert!(
            s.matches(&labels(&[("app", "svc2"), ("tier", "x")])),
            "NotIn matches an absent label"
        );
        assert!(s.matches(&labels(&[("app", "svc"), ("tier", "x"), ("canary", "false")])));
        assert!(!s.matches(&labels(&[("app", "other"), ("tier", "x")])), "In");
        assert!(
            !s.matches(&labels(&[("app", "svc"), ("tier", "x"), ("canary", "true")])),
            "NotIn"
        );
        assert!(!s.matches(&labels(&[("app", "svc")])), "Exists");
        assert!(
            !s.matches(&labels(&[("app", "svc"), ("tier", "x"), ("debug", "1")])),
            "DoesNotExist"
        );
    }

    #[test]
    fn an_empty_or_unreadable_selector_matches_no_pod() {
        let anything = labels(&[("app", "svc")]);
        let empty = manifest(json!([]), json!({}));
        assert!(
            !trim_deployment(&empty, "sit1", &guard(&[]), &helm())
                .unwrap()
                .selector
                .matches(&anything)
        );
        let unknown = manifest(
            json!([]),
            json!({ "matchLabels": { "app": "svc" },
                    "matchExpressions": [{ "key": "app", "operator": "Gt", "values": ["1"] }] }),
        );
        assert!(
            !trim_deployment(&unknown, "sit1", &guard(&[]), &helm())
                .unwrap()
                .selector
                .matches(&anything)
        );
        let terms: Vec<Value> = (0..=MAX_SELECTOR_TERMS)
            .map(|i| json!({ "key": format!("k{i}"), "operator": "Exists" }))
            .collect();
        let too_many = manifest(json!([]), json!({ "matchExpressions": terms }));
        let every_label: Vec<(String, &str)> =
            (0..=MAX_SELECTOR_TERMS).map(|i| (format!("k{i}"), "v")).collect();
        let every: BTreeMap<String, String> = every_label
            .iter()
            .map(|(k, v)| (k.clone(), (*v).to_owned()))
            .collect();
        assert!(
            !trim_deployment(&too_many, "sit1", &guard(&[]), &helm())
                .unwrap()
                .selector
                .matches(&every)
        );
    }

    fn pod_json(phase: Option<&str>, deleting: bool, start: Option<&str>) -> Pod {
        let mut value = json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": "svc-abc", "namespace": "sit1", "labels": { "app": "svc" },
                          "annotations": { "POD_ANNOTATION_MARKER": "x" },
                          "creationTimestamp": "2026-10-10T11:00:00Z" },
            "spec": { "containers": [{ "name": "main", "image": "IMAGE_MARKER" }] },
            "status": {},
        });
        if let Some(phase) = phase {
            value["status"]["phase"] = json!(phase);
        }
        if deleting {
            value["metadata"]["deletionTimestamp"] = json!("2026-10-10T12:00:00Z");
        }
        if let Some(start) = start {
            value["status"]["startTime"] = json!(start);
        }
        pod(value)
    }

    #[test]
    fn a_running_pod_keeps_name_labels_and_start_time_only() {
        let rec = trim_pod(
            &pod_json(Some("Running"), false, Some("2026-10-10T11:59:05Z")),
            "sit1",
        )
        .unwrap();
        assert_eq!(rec.name, "svc-abc");
        assert_eq!(rec.labels, labels(&[("app", "svc")]));
        assert_eq!(rec.started_at, Timestamp::from_unix_millis(1_791_633_545_000));
        let shown = format!("{rec:?}");
        assert!(!shown.contains("POD_ANNOTATION_MARKER") && !shown.contains("IMAGE_MARKER"));
    }

    #[test]
    fn a_pending_pod_counts_and_starts_when_it_was_created() {
        let rec = trim_pod(&pod_json(Some("Pending"), false, None), "sit1").unwrap();
        assert_eq!(rec.started_at, Timestamp::from_unix_millis(1_791_630_000_000));
        assert!(
            trim_pod(&pod_json(None, false, None), "sit1").is_some(),
            "no status yet"
        );
    }

    #[test]
    fn a_pod_that_is_terminating_or_finished_is_not_kept() {
        assert!(trim_pod(&pod_json(Some("Running"), true, None), "sit1").is_none());
        assert!(trim_pod(&pod_json(Some("Succeeded"), false, None), "sit1").is_none());
        assert!(trim_pod(&pod_json(Some("Failed"), false, None), "sit1").is_none());
    }

    #[test]
    fn pod_labels_are_capped() {
        let mut p = pod_json(Some("Running"), false, None);
        p.metadata.labels = Some(
            (0..MAX_POD_LABELS + 20)
                .map(|i| (format!("l{i:04}"), "v".to_owned()))
                .collect(),
        );
        assert_eq!(trim_pod(&p, "sit1").unwrap().labels.len(), MAX_POD_LABELS);
    }
}
