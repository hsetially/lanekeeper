//! Sync Jobs (D72, D75, Q3): which Jobs are the data loads, and whether one is running.
//!
//! A Job is a sync Job when its name matches one of the configured globs (`*dataload*` by default, the name Q36 gives)
//! or it carries the configured `key=value` label. The tracker keeps a trimmed record of those Jobs, and of Jobs that
//! carry a Helm hint, and nothing of the rest. T10 builds the sync window events and the delta tagging on top of this.

use std::collections::BTreeMap;

use domain::{ShortText, Timestamp};
use globset::{Glob, GlobSet, GlobSetBuilder};
use k8s_openapi::api::batch::v1::Job;

use super::error::BuildError;
use super::helm::{HelmFilter, HelmHints};
use crate::config::LabelSelector;

/// Whether a Job still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    Running,
    /// Completed, failed, or suspended: no data is being loaded by it.
    Finished,
}

/// What the agent keeps of a Job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRec {
    pub namespace: String,
    pub name: String,
    pub uid: String,
    /// The Job is a data load (name glob or label), as opposed to one kept only for its Helm hint.
    pub sync: bool,
    pub phase: JobPhase,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub helm: Option<HelmHints>,
}

/// The rule for what a sync Job is.
#[derive(Debug, Clone)]
pub struct JobMatcher {
    names: GlobSet,
    label: Option<LabelSelector>,
}

impl JobMatcher {
    pub fn new(name_globs: &[ShortText], label: Option<&LabelSelector>) -> Result<Self, BuildError> {
        let bad = BuildError::BadGlob {
            setting: "LK_SYNC_JOB_NAME_GLOBS",
        };
        let mut builder = GlobSetBuilder::new();
        for glob in name_globs {
            builder.add(Glob::new(glob.as_str()).map_err(|_| bad)?);
        }
        Ok(Self {
            names: builder.build().map_err(|_| bad)?,
            label: label.cloned(),
        })
    }

    pub fn matches(&self, name: &str, labels: Option<&BTreeMap<String, String>>) -> bool {
        if self.names.is_match(name) {
            return true;
        }
        match (&self.label, labels) {
            (Some(want), Some(labels)) => labels.get(&want.key).is_some_and(|v| *v == want.value),
            _ => false,
        }
    }
}

/// Turns Job objects into [`JobRec`]s.
#[derive(Debug, Clone)]
pub struct JobTracker {
    matcher: JobMatcher,
    helm: HelmFilter,
}

impl JobTracker {
    pub fn new(matcher: JobMatcher, helm: HelmFilter) -> Self {
        Self { matcher, helm }
    }

    /// The record for `job`, or `None` if it is neither a sync Job nor a carrier of a Helm hint.
    pub fn trim(&self, namespace: &str, job: &Job) -> Option<JobRec> {
        let name = job.metadata.name.as_deref()?;
        let uid = job.metadata.uid.as_deref()?;
        let labels = job.metadata.labels.as_ref();
        let sync = self.matcher.matches(name, labels);
        let helm = self.helm.hints(labels);
        if !sync && helm.is_none() {
            return None;
        }
        let status = job.status.as_ref();
        let condition_true = |kind: &str| {
            status
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == kind && c.status == "True"))
        };
        let suspended = job.spec.as_ref().and_then(|s| s.suspend).unwrap_or(false);
        let completed = status.is_some_and(|s| s.completion_time.is_some());
        let finished = completed || condition_true("Complete") || condition_true("Failed") || suspended;
        let millis = |t: &k8s_openapi::apimachinery::pkg::apis::meta::v1::Time| {
            Timestamp::from_unix_millis(t.0.as_millisecond())
        };
        Some(JobRec {
            namespace: job
                .metadata
                .namespace
                .clone()
                .unwrap_or_else(|| namespace.to_owned()),
            name: name.to_owned(),
            uid: uid.to_owned(),
            sync,
            phase: if finished {
                JobPhase::Finished
            } else {
                JobPhase::Running
            },
            started_at: status
                .and_then(|s| s.start_time.as_ref())
                .or(job.metadata.creation_timestamp.as_ref())
                .map(millis),
            finished_at: status.and_then(|s| s.completion_time.as_ref()).map(millis),
            helm,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tracker(globs: &[&str], label: Option<(&str, &str)>) -> JobTracker {
        let globs: Vec<ShortText> = globs.iter().map(|g| ShortText::parse(g).unwrap()).collect();
        let label = label.map(|(k, v)| LabelSelector {
            key: k.to_owned(),
            value: v.to_owned(),
        });
        let helm = HelmFilter::new(&[ShortText::parse("csp-tenant-data-*").unwrap()]).unwrap();
        JobTracker::new(JobMatcher::new(&globs, label.as_ref()).unwrap(), helm)
    }

    fn job(value: serde_json::Value) -> Job {
        serde_json::from_value(value).unwrap()
    }

    // The fixtures take their JSON literals by value, which reads best at the call sites.
    #[allow(clippy::needless_pass_by_value)]
    fn running(name: &str, labels: serde_json::Value) -> Job {
        job(json!({
            "apiVersion": "batch/v1", "kind": "Job",
            "metadata": { "name": name, "namespace": "sit1", "uid": "u-1", "labels": labels,
                          "creationTimestamp": "2026-10-10T11:59:00Z" },
            "status": { "active": 1, "startTime": "2026-10-10T11:59:05Z" },
        }))
    }

    #[test]
    fn a_job_is_a_sync_job_by_name_glob() {
        let t = tracker(&["*dataload*"], None);
        let rec = t
            .trim("sit1", &running("csp-dataload-20261010", json!({})))
            .unwrap();
        assert!(rec.sync);
        assert_eq!(rec.phase, JobPhase::Running);
        assert_eq!(rec.uid, "u-1");
        assert_eq!(
            rec.started_at,
            Some(Timestamp::from_unix_millis(1_791_633_545_000))
        );
        assert_eq!(rec.finished_at, None);
        assert!(t.trim("sit1", &running("nightly-report", json!({}))).is_none());
    }

    #[test]
    fn a_job_is_a_sync_job_by_label() {
        let t = tracker(&["*dataload*"], Some(("lanekeeper.io/sync", "true")));
        assert!(
            t.trim(
                "sit1",
                &running("copy-config", json!({"lanekeeper.io/sync": "true"}))
            )
            .unwrap()
            .sync
        );
        assert!(
            t.trim(
                "sit1",
                &running("copy-config", json!({"lanekeeper.io/sync": "false"}))
            )
            .is_none()
        );
        assert!(t.trim("sit1", &running("copy-config", json!({}))).is_none());
    }

    #[test]
    fn a_job_with_a_helm_hint_is_kept_but_is_not_a_sync_job() {
        let t = tracker(&["*dataload*"], None);
        let rec = t
            .trim(
                "sit1",
                &running(
                    "tenant-data",
                    json!({"helm.sh/chart": "csp-tenant-data-sit1-1.0.0"}),
                ),
            )
            .unwrap();
        assert!(!rec.sync);
        assert_eq!(rec.helm.unwrap().chart, "csp-tenant-data-sit1-1.0.0");
    }

    #[test]
    fn a_job_is_running_until_it_completes_fails_or_is_suspended() {
        let t = tracker(&["*dataload*"], None);
        let with_status = |status: serde_json::Value| {
            job(json!({
                "apiVersion": "batch/v1", "kind": "Job",
                "metadata": { "name": "dataload", "namespace": "sit1", "uid": "u-1" },
                "status": status,
            }))
        };
        let phase = |status| t.trim("sit1", &with_status(status)).unwrap().phase;
        assert_eq!(phase(json!({})), JobPhase::Running, "just created, no pod yet");
        assert_eq!(phase(json!({"active": 2})), JobPhase::Running);
        assert_eq!(
            phase(json!({"conditions": [{"type": "Complete", "status": "True"}]})),
            JobPhase::Finished
        );
        assert_eq!(
            phase(json!({"conditions": [{"type": "Failed", "status": "True"}]})),
            JobPhase::Finished
        );
        assert_eq!(
            phase(json!({"conditions": [{"type": "Failed", "status": "False"}]})),
            JobPhase::Running
        );
        assert_eq!(
            phase(json!({"completionTime": "2026-10-10T12:03:00Z"})),
            JobPhase::Finished
        );
        let suspended = job(json!({
            "apiVersion": "batch/v1", "kind": "Job",
            "metadata": { "name": "dataload", "namespace": "sit1", "uid": "u-1" },
            "spec": { "suspend": true, "template": { "spec": { "containers": [] } } },
        }));
        assert_eq!(t.trim("sit1", &suspended).unwrap().phase, JobPhase::Finished);
    }

    #[test]
    fn the_completion_time_is_kept() {
        let t = tracker(&["*dataload*"], None);
        let done = job(json!({
            "apiVersion": "batch/v1", "kind": "Job",
            "metadata": { "name": "dataload", "namespace": "sit1", "uid": "u-1" },
            "status": { "startTime": "2026-10-10T11:59:05Z", "completionTime": "2026-10-10T12:03:00Z" },
        }));
        let rec = t.trim("sit1", &done).unwrap();
        assert_eq!(
            rec.finished_at,
            Some(Timestamp::from_unix_millis(1_791_633_780_000))
        );
    }

    #[test]
    fn a_job_with_no_uid_is_not_tracked() {
        let t = tracker(&["*dataload*"], None);
        let no_uid = job(json!({
            "apiVersion": "batch/v1", "kind": "Job",
            "metadata": { "name": "dataload", "namespace": "sit1" },
        }));
        assert!(t.trim("sit1", &no_uid).is_none());
    }
}
