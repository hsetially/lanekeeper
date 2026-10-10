//! Sync Jobs (D72, D75, Q3): which Jobs are the data loads, and whether one is running.
//!
//! A Job is a sync Job when its name matches one of the configured globs (`*dataload*` by default, the name Q36 gives)
//! or it carries the configured `key=value` label. The tracker keeps a trimmed record of those Jobs, and of Jobs that
//! carry a Helm hint, and nothing of the rest. [`window_transition`] turns a change of one of them into what the window
//! ledger needs (T10, D75).

use std::collections::BTreeMap;

use domain::{JobRef, ShortText, Timestamp};
use globset::{Glob, GlobSet, GlobSetBuilder};
use k8s_openapi::api::batch::v1::Job;

use super::error::BuildError;
use super::helm::{HelmFilter, HelmHints};
use crate::config::LabelSelector;
use crate::windows::WindowTransition;

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

/// What the change of a Job from `old` to `new` means for the sync windows: it started running, or it stopped (completed,
/// failed, was suspended, was deleted, or is no longer a sync Job). `None` for everything else, including a Job that was
/// never a sync Job.
pub fn window_transition(old: Option<&JobRec>, new: Option<&JobRec>) -> Option<WindowTransition> {
    let running = |rec: &&JobRec| rec.sync && rec.phase == JobPhase::Running;
    match (old.filter(running), new) {
        (_, Some(new)) if running(&new) => Some(WindowTransition::Running {
            job: JobRef::new(&new.name, &new.uid).ok()?,
            started: new.started_at,
        }),
        // It was running and is not now. `finished_at` is the completion time when the API gives one.
        (Some(was), after) => Some(WindowTransition::Finished {
            job: JobRef::new(&was.name, &was.uid).ok()?,
            finished: after.and_then(|rec| rec.finished_at),
        }),
        // Never seen running: a Job that finished before it was seen has no window.
        (None, _) => None,
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

    fn rec(sync: bool, phase: JobPhase) -> JobRec {
        JobRec {
            namespace: "sit1".to_owned(),
            name: "dataload".to_owned(),
            uid: "u-1".to_owned(),
            sync,
            phase,
            started_at: Some(Timestamp::from_unix_millis(5)),
            finished_at: Some(Timestamp::from_unix_millis(9)),
            helm: None,
        }
    }

    #[test]
    fn a_running_sync_job_opens_and_a_stopped_one_closes() {
        let running = rec(true, JobPhase::Running);
        let done = rec(true, JobPhase::Finished);
        let job = JobRef::new("dataload", "u-1").unwrap();
        assert_eq!(
            window_transition(None, Some(&running)),
            Some(WindowTransition::Running {
                job: job.clone(),
                started: Some(Timestamp::from_unix_millis(5))
            })
        );
        assert_eq!(
            window_transition(Some(&running), Some(&done)),
            Some(WindowTransition::Finished {
                job: job.clone(),
                finished: Some(Timestamp::from_unix_millis(9))
            })
        );
        assert_eq!(
            window_transition(Some(&running), None),
            Some(WindowTransition::Finished { job, finished: None }),
            "deleted while running"
        );
    }

    #[test]
    fn a_job_that_was_never_running_or_is_not_a_sync_job_says_nothing() {
        let done = rec(true, JobPhase::Finished);
        assert_eq!(window_transition(None, Some(&done)), None);
        assert_eq!(window_transition(Some(&done), Some(&done)), None);
        assert_eq!(
            window_transition(None, Some(&rec(false, JobPhase::Running))),
            None
        );
        assert_eq!(
            window_transition(Some(&rec(false, JobPhase::Running)), None),
            None
        );
    }

    #[test]
    fn a_running_job_that_stops_being_a_sync_job_closes() {
        let before = rec(true, JobPhase::Running);
        let after = rec(false, JobPhase::Running);
        assert!(matches!(
            window_transition(Some(&before), Some(&after)),
            Some(WindowTransition::Finished { .. })
        ));
    }
}
