//! The agent's picture of the cluster, and the reports made from it (T6).
//!
//! A [`Projection`] holds the trimmed Deployments, Pods and Jobs of the watched namespaces. Watcher events change it
//! ([`Projection::apply_deployment`] and its siblings), and each change returns the keys of the Deployments and Jobs
//! whose report entry is now different. A full report lists everything; a delta lists only the keys it is given.
//!
//! The pure state lives here and nothing in this file waits or does I/O, so every rule can be tested with plain values.

use std::collections::{BTreeMap, BTreeSet};

use domain::{ClusterReport, DeploymentInfo, EnvValue, JobRef, PodInfo, ReleaseHint, ServiceRef, ShortText};
use tracing::warn;

use super::env::EnvGuard;
use super::helm::{CHART_LABEL, HelmHints, INSTANCE_LABEL};
use super::jobs::{JobPhase, JobRec};
use super::trim::{DeploymentRec, PodRec};

/// The most objects of one kind that are tracked. Past this the extra objects are ignored (and logged), so that memory
/// stays bounded whatever the cluster holds.
pub const MAX_TRACKED: usize = 50_000;
/// The most pods listed under one Deployment in a report.
pub const MAX_PODS_PER_DEPLOYMENT: usize = 1_000;
/// The most list entries in one report, which is what the hub accepts (`proto::limits::MAX_ENTRIES`).
pub const MAX_REPORT_ENTRIES: usize = proto::limits::MAX_ENTRIES;

/// `(namespace, name)`.
pub type Key = (String, String);

/// An object a report may need to mention again.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DirtyKey {
    Deployment(Key),
    Job(Key),
}

/// A watcher event, after the object in it has been trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change<R> {
    /// The watcher is about to list again.
    Init,
    /// One object of the list.
    InitApply(R),
    /// The list is complete: it replaces what the namespace held.
    InitDone,
    Apply(R),
    Delete(Key),
}

pub trait Keyed {
    fn key(&self) -> Key;
}

impl Keyed for DeploymentRec {
    fn key(&self) -> Key {
        (self.namespace.clone(), self.name.clone())
    }
}

impl Keyed for PodRec {
    fn key(&self) -> Key {
        (self.namespace.clone(), self.name.clone())
    }
}

impl Keyed for JobRec {
    fn key(&self) -> Key {
        (self.namespace.clone(), self.name.clone())
    }
}

/// An object that was added, changed or removed by an event.
struct Changed<R> {
    old: Option<R>,
    new: Option<R>,
}

/// The trimmed objects of one kind.
#[derive(Debug)]
struct ObjectStore<R> {
    kind: &'static str,
    current: BTreeMap<Key, R>,
    /// A list in progress, per namespace. It replaces `current` for that namespace when it completes.
    staging: BTreeMap<String, BTreeMap<Key, R>>,
    /// The namespaces whose first list has completed.
    synced: BTreeSet<String>,
}

impl<R: Keyed + Clone + PartialEq> ObjectStore<R> {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            current: BTreeMap::new(),
            staging: BTreeMap::new(),
            synced: BTreeSet::new(),
        }
    }

    fn in_namespace<'a>(&'a self, namespace: &'a str) -> impl Iterator<Item = (&'a Key, &'a R)> {
        self.current
            .range((namespace.to_owned(), String::new())..)
            .take_while(move |((ns, _), _)| ns == namespace)
    }

    fn apply(&mut self, namespace: &str, change: Change<R>) -> Vec<Changed<R>> {
        match change {
            Change::Init => {
                self.staging.insert(namespace.to_owned(), BTreeMap::new());
                Vec::new()
            }
            Change::InitApply(record) => {
                let staged = self.staging.entry(namespace.to_owned()).or_default();
                if staged.len() < MAX_TRACKED {
                    staged.insert(record.key(), record);
                } else {
                    warn!(kind = self.kind, namespace, "too many objects; ignoring one");
                }
                Vec::new()
            }
            Change::InitDone => {
                let fresh = self.staging.remove(namespace).unwrap_or_default();
                let old_keys: Vec<Key> = self.in_namespace(namespace).map(|(k, _)| k.clone()).collect();
                let mut old: BTreeMap<Key, R> = old_keys
                    .into_iter()
                    .filter_map(|k| self.current.remove(&k).map(|r| (k, r)))
                    .collect();
                let mut changes = Vec::new();
                for (key, record) in fresh {
                    let before = old.remove(&key);
                    if before.as_ref() != Some(&record) {
                        changes.push(Changed {
                            old: before,
                            new: Some(record.clone()),
                        });
                    }
                    self.current.insert(key, record);
                }
                changes.extend(old.into_values().map(|gone| Changed {
                    old: Some(gone),
                    new: None,
                }));
                self.synced.insert(namespace.to_owned());
                changes
            }
            Change::Apply(record) => {
                let key = record.key();
                if !self.current.contains_key(&key) && self.current.len() >= MAX_TRACKED {
                    warn!(kind = self.kind, namespace = %key.0, "too many objects; ignoring one");
                    return Vec::new();
                }
                let before = self.current.insert(key, record.clone());
                if before.as_ref() == Some(&record) {
                    return Vec::new();
                }
                vec![Changed {
                    old: before,
                    new: Some(record),
                }]
            }
            Change::Delete(key) => match self.current.remove(&key) {
                Some(gone) => vec![Changed {
                    old: Some(gone),
                    new: None,
                }],
                None => Vec::new(),
            },
        }
    }
}

/// The trimmed cluster.
#[derive(Debug)]
pub struct Projection {
    namespaces: BTreeSet<String>,
    deployments: ObjectStore<DeploymentRec>,
    pods: ObjectStore<PodRec>,
    jobs: ObjectStore<JobRec>,
}

impl Projection {
    pub fn new(namespaces: &[String]) -> Self {
        Self {
            namespaces: namespaces.iter().cloned().collect(),
            deployments: ObjectStore::new("deployments"),
            pods: ObjectStore::new("pods"),
            jobs: ObjectStore::new("jobs"),
        }
    }

    /// Every watched namespace has listed its Deployments and Pods. A report made earlier would say that a Deployment
    /// has no pods when the agent just has not heard of them yet. Jobs are not waited for: they only add hints and
    /// sync windows.
    pub fn is_synced(&self) -> bool {
        self.namespaces
            .iter()
            .all(|ns| self.deployments.synced.contains(ns) && self.pods.synced.contains(ns))
    }

    pub fn apply_deployment(&mut self, namespace: &str, change: Change<DeploymentRec>) -> Vec<DirtyKey> {
        self.deployments
            .apply(namespace, change)
            .into_iter()
            .flat_map(|c| {
                c.old
                    .iter()
                    .chain(c.new.iter())
                    .map(Keyed::key)
                    .collect::<BTreeSet<_>>()
            })
            .map(DirtyKey::Deployment)
            .collect()
    }

    /// A pod's change is a change of the Deployment it belongs to, before the change and after it (its labels may have
    /// moved it from one Deployment to another).
    pub fn apply_pod(&mut self, namespace: &str, change: Change<PodRec>) -> Vec<DirtyKey> {
        let changes = self.pods.apply(namespace, change);
        let mut dirty = BTreeSet::new();
        for pod in changes.iter().flat_map(|c| c.old.iter().chain(c.new.iter())) {
            for ((_, name), deployment) in self.deployments.in_namespace(&pod.namespace) {
                if deployment.selector.matches(&pod.labels) {
                    dirty.insert(DirtyKey::Deployment((pod.namespace.clone(), name.clone())));
                }
            }
        }
        dirty.into_iter().collect()
    }

    /// A Job is mentioned again when it gains or changes a Helm hint: the hint is all a report says about it.
    pub fn apply_job(&mut self, namespace: &str, change: Change<JobRec>) -> Vec<DirtyKey> {
        self.jobs
            .apply(namespace, change)
            .into_iter()
            .filter_map(|c| {
                let new = c.new?;
                (new.helm.is_some() && c.old.as_ref().map(|o| &o.helm) != Some(&new.helm))
                    .then(|| DirtyKey::Job(new.key()))
            })
            .collect()
    }

    /// The sync Jobs that are running now.
    pub fn active_sync_jobs(&self) -> Vec<JobRef> {
        self.jobs
            .current
            .values()
            .filter(|j| j.sync && j.phase == JobPhase::Running)
            .filter_map(|j| JobRef::new(&j.name, &j.uid).ok())
            .collect()
    }

    /// Everything the agent knows. The hub replaces its picture of the cluster with this.
    pub fn full_report(&self, guard: &EnvGuard) -> ClusterReport {
        let mut report = empty_report(true);
        for deployment in self.deployments.current.values() {
            self.add_deployment(&mut report, deployment, guard);
        }
        for job in self.jobs.current.values() {
            add_hints(&mut report, &job.namespace, &job.name, job.helm.as_ref());
        }
        cap(&mut report);
        report
    }

    /// Only these keys. `None` when none of them says anything (a Job without a hint, say), so nothing is sent.
    pub fn delta_report(&self, dirty: &BTreeSet<DirtyKey>, guard: &EnvGuard) -> Option<ClusterReport> {
        let mut report = empty_report(false);
        for key in dirty {
            match key {
                DirtyKey::Deployment(key) => match self.deployments.current.get(key) {
                    Some(deployment) => self.add_deployment(&mut report, deployment, guard),
                    None => {
                        if let Ok(service) = ServiceRef::new(&key.0, &key.1) {
                            report.deployments.push(DeploymentInfo {
                                service,
                                pods: Vec::new(),
                                env_values: Vec::new(),
                                env_names: Vec::new(),
                                removed: true,
                            });
                        }
                    }
                },
                DirtyKey::Job(key) => {
                    if let Some(job) = self.jobs.current.get(key) {
                        add_hints(&mut report, &job.namespace, &job.name, job.helm.as_ref());
                    }
                }
            }
        }
        cap(&mut report);
        (!report.deployments.is_empty() || !report.release_hints.is_empty()).then_some(report)
    }

    fn add_deployment(&self, report: &mut ClusterReport, deployment: &DeploymentRec, guard: &EnvGuard) {
        let Ok(service) = ServiceRef::new(&deployment.namespace, &deployment.name) else {
            return;
        };
        let pods = self
            .pods
            .in_namespace(&deployment.namespace)
            .filter(|(_, pod)| deployment.selector.matches(&pod.labels))
            .filter_map(|(_, pod)| {
                Some(PodInfo {
                    name: ShortText::parse(&pod.name).ok()?,
                    started_at: pod.started_at,
                })
            })
            .take(MAX_PODS_PER_DEPLOYMENT)
            .collect();
        let mut env_values = Vec::new();
        let mut env_names = Vec::new();
        for (name, value) in &deployment.env {
            let Ok(short_name) = ShortText::parse(name) else {
                continue;
            };
            // The allowlist is checked again here, so a list that has just narrowed takes effect before the next relist.
            let value = value.as_deref().and_then(|v| guard.value_for(name, v));
            match value {
                Some(value) => env_values.push(EnvValue {
                    name: short_name,
                    value,
                }),
                None => env_names.push(short_name),
            }
        }
        env_values.truncate(MAX_REPORT_ENTRIES);
        env_names.truncate(MAX_REPORT_ENTRIES);
        add_hints(
            report,
            &deployment.namespace,
            &deployment.name,
            deployment.helm.as_ref(),
        );
        report.deployments.push(DeploymentInfo {
            service,
            pods,
            env_values,
            env_names,
            removed: false,
        });
    }
}

fn empty_report(full: bool) -> ClusterReport {
    ClusterReport {
        full,
        deployments: Vec::new(),
        release_hints: Vec::new(),
        // The config-server's start time (C11) is T12's; sync windows (D75) are T10's.
        config_server_started_at: None,
        sync_windows: Vec::new(),
    }
}

fn add_hints(report: &mut ClusterReport, namespace: &str, name: &str, helm: Option<&HelmHints>) {
    let Some(helm) = helm else { return };
    let Ok(service) = ServiceRef::new(namespace, name) else {
        return;
    };
    let mut push = |key: &str, value: &str| {
        if let (Ok(key), Ok(value)) = (ShortText::parse(key), ShortText::parse(value)) {
            report.release_hints.push(ReleaseHint {
                service: service.clone(),
                key,
                value,
            });
        }
    };
    push(CHART_LABEL, &helm.chart);
    if let Some(instance) = &helm.instance {
        push(INSTANCE_LABEL, instance);
    }
}

/// Keep the lists inside what the hub accepts. A cluster that large is not expected; the report stays valid, and the
/// log says that it was cut.
fn cap(report: &mut ClusterReport) {
    if report.deployments.len() > MAX_REPORT_ENTRIES || report.release_hints.len() > MAX_REPORT_ENTRIES {
        warn!("the cluster report has more entries than the hub accepts; cutting it");
        report.deployments.truncate(MAX_REPORT_ENTRIES);
        report.release_hints.truncate(MAX_REPORT_ENTRIES);
    }
}

#[cfg(test)]
mod tests {
    use domain::Timestamp;

    use super::*;
    use crate::kube::trim::Selector;

    fn ns_list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn selector(app: &str) -> Selector {
        serde_selector(app)
    }

    /// A selector for `app=<app>`, built the way the trimmer builds one.
    fn serde_selector(app: &str) -> Selector {
        use k8s_openapi::api::apps::v1::Deployment;
        let d: Deployment = serde_json::from_value(serde_json::json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": { "name": "x", "namespace": "n" },
            "spec": { "selector": { "matchLabels": { "app": app } }, "template": { "spec": { "containers": [] } } },
        }))
        .unwrap();
        crate::kube::trim::trim_deployment(
            &d,
            "n",
            &EnvGuard::new(),
            &crate::kube::helm::HelmFilter::new(&[]).unwrap(),
        )
        .unwrap()
        .selector
    }

    fn dep(ns: &str, name: &str, env: &[(&str, Option<&str>)]) -> DeploymentRec {
        DeploymentRec {
            namespace: ns.to_owned(),
            name: name.to_owned(),
            selector: selector(name),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
                .collect(),
            helm: None,
        }
    }

    fn pod(ns: &str, name: &str, app: &str, started: i64) -> PodRec {
        PodRec {
            namespace: ns.to_owned(),
            name: name.to_owned(),
            labels: BTreeMap::from([("app".to_owned(), app.to_owned())]),
            started_at: Timestamp::from_unix_millis(started),
        }
    }

    fn job(ns: &str, name: &str, sync: bool, phase: JobPhase, chart: Option<&str>) -> JobRec {
        JobRec {
            namespace: ns.to_owned(),
            name: name.to_owned(),
            uid: format!("uid-{name}"),
            sync,
            phase,
            started_at: None,
            finished_at: None,
            helm: chart.map(|c| HelmHints {
                chart: c.to_owned(),
                instance: Some("inst".to_owned()),
            }),
        }
    }

    fn key(ns: &str, name: &str) -> DirtyKey {
        DirtyKey::Deployment((ns.to_owned(), name.to_owned()))
    }

    fn list<R>(
        p: &mut Projection,
        ns: &str,
        items: Vec<R>,
        apply: fn(&mut Projection, &str, Change<R>) -> Vec<DirtyKey>,
    ) -> Vec<DirtyKey> {
        let mut dirty = apply(p, ns, Change::Init);
        for item in items {
            dirty.extend(apply(p, ns, Change::InitApply(item)));
        }
        dirty.extend(apply(p, ns, Change::InitDone));
        dirty
    }

    fn guard(names: &[&str]) -> EnvGuard {
        let names: Vec<ShortText> = names.iter().map(|n| ShortText::parse(n).unwrap()).collect();
        EnvGuard::with_allowlist(&names)
    }

    fn sets(keys: Vec<DirtyKey>) -> BTreeSet<DirtyKey> {
        keys.into_iter().collect()
    }

    #[test]
    fn a_list_is_only_visible_when_it_completes() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_deployment("sit1", Change::Init);
        p.apply_deployment("sit1", Change::InitApply(dep("sit1", "a", &[])));
        assert!(p.full_report(&guard(&[])).deployments.is_empty());
        let dirty = p.apply_deployment("sit1", Change::InitDone);
        assert_eq!(dirty, vec![key("sit1", "a")]);
        assert_eq!(p.full_report(&guard(&[])).deployments.len(), 1);
    }

    #[test]
    fn a_relist_marks_only_what_differs() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        let first = list(
            &mut p,
            "sit1",
            vec![
                dep("sit1", "a", &[]),
                dep("sit1", "b", &[]),
                dep("sit1", "c", &[]),
            ],
            Projection::apply_deployment,
        );
        assert_eq!(sets(first).len(), 3);
        let second = list(
            &mut p,
            "sit1",
            vec![
                dep("sit1", "a", &[]),
                dep("sit1", "b", &[("NEW", None)]),
                dep("sit1", "d", &[]),
            ],
            Projection::apply_deployment,
        );
        assert_eq!(
            sets(second),
            BTreeSet::from([key("sit1", "b"), key("sit1", "c"), key("sit1", "d")])
        );
    }

    #[test]
    fn a_relist_of_one_namespace_leaves_the_others_alone() {
        let mut p = Projection::new(&ns_list(&["sit1", "sit2"]));
        list(
            &mut p,
            "sit1",
            vec![dep("sit1", "a", &[])],
            Projection::apply_deployment,
        );
        list(
            &mut p,
            "sit2",
            vec![dep("sit2", "b", &[])],
            Projection::apply_deployment,
        );
        let dirty = list(&mut p, "sit1", vec![], Projection::apply_deployment);
        assert_eq!(dirty, vec![key("sit1", "a")]);
        let names: Vec<_> = p
            .full_report(&guard(&[]))
            .deployments
            .iter()
            .map(|d| d.service.name().to_owned())
            .collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn applying_an_unchanged_object_marks_nothing() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        assert_eq!(
            p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[]))),
            vec![key("sit1", "a")]
        );
        assert!(
            p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])))
                .is_empty()
        );
        assert_eq!(
            p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[("X", None)]))),
            vec![key("sit1", "a")]
        );
        assert!(
            p.apply_deployment("sit1", Change::Delete(("sit1".into(), "zzz".into())))
                .is_empty()
        );
    }

    #[test]
    fn a_pod_belongs_to_the_deployments_whose_selector_matches_it() {
        let mut p = Projection::new(&ns_list(&["sit1", "sit2"]));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "b", &[])));
        p.apply_deployment("sit2", Change::Apply(dep("sit2", "a", &[])));
        let dirty = p.apply_pod("sit1", Change::Apply(pod("sit1", "a-1", "a", 1_000)));
        assert_eq!(dirty, vec![key("sit1", "a")], "not b, and not sit2's a");
        let report = p.full_report(&guard(&[]));
        let a = report
            .deployments
            .iter()
            .find(|d| d.service.namespace() == "sit1" && d.service.name() == "a")
            .unwrap();
        assert_eq!(a.pods.len(), 1);
        assert_eq!(
            (a.pods[0].name.as_str(), a.pods[0].started_at.unix_millis()),
            ("a-1", 1_000)
        );
        let other = report
            .deployments
            .iter()
            .find(|d| d.service.namespace() == "sit2")
            .unwrap();
        assert!(other.pods.is_empty());
        let b = report
            .deployments
            .iter()
            .find(|d| d.service.name() == "b")
            .unwrap();
        assert!(
            b.pods.is_empty(),
            "a pod is listed under the Deployment its labels select, and no other"
        );
    }

    #[test]
    fn a_pod_that_changes_labels_dirties_both_deployments() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "b", &[])));
        p.apply_pod("sit1", Change::Apply(pod("sit1", "x", "a", 1)));
        let dirty = p.apply_pod("sit1", Change::Apply(pod("sit1", "x", "b", 1)));
        assert_eq!(sets(dirty), BTreeSet::from([key("sit1", "a"), key("sit1", "b")]));
    }

    #[test]
    fn a_pod_removed_dirties_its_deployment_and_leaves_the_report() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])));
        p.apply_pod("sit1", Change::Apply(pod("sit1", "a-1", "a", 1)));
        let dirty = p.apply_pod("sit1", Change::Delete(("sit1".into(), "a-1".into())));
        assert_eq!(dirty, vec![key("sit1", "a")]);
        assert!(p.full_report(&guard(&[])).deployments[0].pods.is_empty());
    }

    #[test]
    fn a_pod_with_no_deployment_yet_marks_nothing_and_is_picked_up_later() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        assert!(
            p.apply_pod("sit1", Change::Apply(pod("sit1", "a-1", "a", 7)))
                .is_empty()
        );
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])));
        assert_eq!(p.full_report(&guard(&[])).deployments[0].pods.len(), 1);
    }

    #[test]
    fn a_removed_deployment_is_reported_as_removed_in_a_delta() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[("V", Some("1"))])));
        let dirty = p.apply_deployment("sit1", Change::Delete(("sit1".into(), "a".into())));
        let report = p.delta_report(&sets(dirty), &guard(&["V"])).unwrap();
        assert!(!report.full);
        assert_eq!(report.deployments.len(), 1);
        let gone = &report.deployments[0];
        assert!(
            gone.removed && gone.pods.is_empty() && gone.env_values.is_empty() && gone.env_names.is_empty()
        );
        assert_eq!((gone.service.namespace(), gone.service.name()), ("sit1", "a"));
    }

    #[test]
    fn a_delta_lists_only_the_keys_it_is_given() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        for n in ["a", "b", "c"] {
            p.apply_deployment("sit1", Change::Apply(dep("sit1", n, &[])));
        }
        let report = p
            .delta_report(&BTreeSet::from([key("sit1", "b")]), &guard(&[]))
            .unwrap();
        assert_eq!(report.deployments.len(), 1);
        assert_eq!(report.deployments[0].service.name(), "b");
        assert!(p.delta_report(&BTreeSet::new(), &guard(&[])).is_none());
    }

    #[test]
    fn a_full_report_is_sorted_and_says_it_is_full() {
        let mut p = Projection::new(&ns_list(&["sit2", "sit1"]));
        for (ns, n) in [("sit2", "z"), ("sit1", "b"), ("sit1", "a")] {
            p.apply_deployment(ns, Change::Apply(dep(ns, n, &[])));
        }
        let report = p.full_report(&guard(&[]));
        assert!(report.full && report.sync_windows.is_empty() && report.config_server_started_at.is_none());
        let order: Vec<_> = report
            .deployments
            .iter()
            .map(|d| format!("{}/{}", d.service.namespace(), d.service.name()))
            .collect();
        assert_eq!(order, vec!["sit1/a", "sit1/b", "sit2/z"]);
    }

    #[test]
    fn env_values_follow_the_allowlist_at_report_time() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        // The record kept a value that the list has since stopped permitting.
        p.apply_deployment(
            "sit1",
            Change::Apply(dep(
                "sit1",
                "a",
                &[("KEPT", Some("1")), ("OLD", Some("2")), ("PLAIN", None)],
            )),
        );
        let report = p.full_report(&guard(&["KEPT"]));
        let d = &report.deployments[0];
        assert_eq!(
            d.env_values
                .iter()
                .map(|v| (v.name.as_str(), v.value.as_str()))
                .collect::<Vec<_>>(),
            vec![("KEPT", "1")]
        );
        assert_eq!(
            d.env_names.iter().map(ShortText::as_str).collect::<Vec<_>>(),
            vec!["OLD", "PLAIN"]
        );
    }

    #[test]
    fn release_hints_come_from_deployments_and_jobs_with_a_matching_chart() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        let mut with_hint = dep("sit1", "svc", &[]);
        with_hint.helm = Some(HelmHints {
            chart: "csp-tenant-data-sit1-1.0.0".into(),
            instance: None,
        });
        p.apply_deployment("sit1", Change::Apply(with_hint));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "plain", &[])));
        let dirty = p.apply_job(
            "sit1",
            Change::Apply(job(
                "sit1",
                "tenant-data",
                false,
                JobPhase::Finished,
                Some("csp-tenant-data-sit1-1.0.0"),
            )),
        );
        assert_eq!(dirty, vec![DirtyKey::Job(("sit1".into(), "tenant-data".into()))]);
        let report = p.full_report(&guard(&[]));
        let hints: Vec<_> = report
            .release_hints
            .iter()
            .map(|h| {
                (
                    h.service.name().to_owned(),
                    h.key.as_str().to_owned(),
                    h.value.as_str().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            hints,
            vec![
                (
                    "svc".into(),
                    "helm.sh/chart".into(),
                    "csp-tenant-data-sit1-1.0.0".into()
                ),
                (
                    "tenant-data".into(),
                    "helm.sh/chart".into(),
                    "csp-tenant-data-sit1-1.0.0".into()
                ),
                (
                    "tenant-data".into(),
                    "app.kubernetes.io/instance".into(),
                    "inst".into()
                ),
            ]
        );
    }

    #[test]
    fn a_job_without_a_hint_marks_nothing_and_a_delta_for_it_is_empty() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        assert!(
            p.apply_job(
                "sit1",
                Change::Apply(job("sit1", "dataload", true, JobPhase::Running, None))
            )
            .is_empty()
        );
        let only_job = BTreeSet::from([DirtyKey::Job(("sit1".into(), "dataload".into()))]);
        assert!(p.delta_report(&only_job, &guard(&[])).is_none());
    }

    #[test]
    fn active_sync_jobs_are_the_running_sync_jobs() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_job(
            "sit1",
            Change::Apply(job("sit1", "dataload-1", true, JobPhase::Running, None)),
        );
        p.apply_job(
            "sit1",
            Change::Apply(job("sit1", "dataload-0", true, JobPhase::Finished, None)),
        );
        p.apply_job(
            "sit1",
            Change::Apply(job(
                "sit1",
                "tenant-data",
                false,
                JobPhase::Running,
                Some("csp-tenant-data-x"),
            )),
        );
        let active = p.active_sync_jobs();
        assert_eq!(active.len(), 1);
        assert_eq!(
            (active[0].name(), active[0].uid()),
            ("dataload-1", "uid-dataload-1")
        );
    }

    #[test]
    fn synced_means_deployments_and_pods_listed_in_every_namespace() {
        let mut p = Projection::new(&ns_list(&["sit1", "sit2"]));
        assert!(!p.is_synced());
        list(
            &mut p,
            "sit1",
            vec![dep("sit1", "a", &[])],
            Projection::apply_deployment,
        );
        list(&mut p, "sit1", vec![], Projection::apply_pod);
        list(
            &mut p,
            "sit2",
            vec![dep("sit2", "a", &[])],
            Projection::apply_deployment,
        );
        assert!(!p.is_synced(), "sit2 has no pod list yet");
        list(&mut p, "sit2", vec![], Projection::apply_pod);
        assert!(p.is_synced(), "jobs are not waited for");
    }

    #[test]
    fn the_pods_of_a_deployment_are_capped() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        p.apply_deployment("sit1", Change::Apply(dep("sit1", "a", &[])));
        for i in 0..MAX_PODS_PER_DEPLOYMENT + 5 {
            p.apply_pod("sit1", Change::Apply(pod("sit1", &format!("a-{i:05}"), "a", 1)));
        }
        assert_eq!(
            p.full_report(&guard(&[])).deployments[0].pods.len(),
            MAX_PODS_PER_DEPLOYMENT
        );
    }

    #[test]
    fn tracking_is_bounded() {
        let mut p = Projection::new(&ns_list(&["sit1"]));
        for i in 0..MAX_TRACKED + 10 {
            p.apply_pod("sit1", Change::Apply(pod("sit1", &format!("p-{i:06}"), "a", 1)));
        }
        assert_eq!(p.pods.current.len(), MAX_TRACKED);
        // An object already tracked can still change when the store is full.
        p.apply_pod("sit1", Change::Apply(pod("sit1", "p-000000", "b", 2)));
        assert_eq!(
            p.pods.current[&("sit1".to_owned(), "p-000000".to_owned())]
                .started_at
                .unix_millis(),
            2
        );
    }
}
