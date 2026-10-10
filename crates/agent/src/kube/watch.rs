//! The watchers: Deployments, Pods and Jobs of the configured namespaces, kept as a [`Projection`] and turned into
//! debounced cluster reports (T6, S17).
//!
//! One watcher per kind and namespace, each a kube-rs `watcher` (list, then watch, with backoff and paging) and not a
//! poll. Every object is trimmed at once ([`super::trim`]) and dropped; only the trimmed record reaches the shared
//! [`Projection`]. Changes mark the affected Deployments, and the report task releases them one second after the first
//! change, as one delta.
//!
//! The environment allowlist can change while the agent runs. The Deployment watchers then list again, so that values
//! are kept only for the names now allowed (and dropped for the names no longer allowed). A full report waits until
//! that fresh list is in.
//!
//! RBAC (S17): `get`, `list` and `watch` on the three kinds in the configured namespaces, and nothing else here. A
//! watcher whose list is refused logs the HTTP status and tries again with a growing delay; it never gives up, because
//! an agent that stops watching would report a cluster that no longer exists.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ::kube::runtime::WatchStreamExt;
use ::kube::runtime::watcher::{self, Event};
use ::kube::{Api, Client, Resource};
use domain::{ClusterReport, JobRef, ShortText};
use futures::StreamExt;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{debug, warn};

use super::debounce::ReportDebouncer;
use super::env::EnvGuard;
use super::error::{BuildError, ReportError};
use super::helm::HelmFilter;
use super::jobs::{JobMatcher, JobRec, JobTracker};
use super::projection::{Change, DirtyKey, Projection};
use super::trim::{self, DeploymentRec, PodRec};
use crate::config::{KubeName, Settings};

/// Changes within this long leave as one report.
pub const REPORT_DEBOUNCE: Duration = Duration::from_secs(1);
/// Objects per list page. Paging keeps the memory of a list in step with a page, not with the cluster.
pub const LIST_PAGE_SIZE: u32 = 200;
/// How long a full report waits for the first lists.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(20);
/// Reports waiting for the connection. Past this the report task waits, and the changes pile up in the dirty set,
/// which is bounded by the number of Deployments.
const REPORT_QUEUE: usize = 8;

/// What the watchers need to know.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub namespaces: Vec<KubeName>,
    pub helm: HelmFilter,
    pub jobs: JobMatcher,
    pub debounce: Duration,
    pub page_size: u32,
    pub sync_timeout: Duration,
}

impl WatchConfig {
    pub fn new(namespaces: Vec<KubeName>, helm: HelmFilter, jobs: JobMatcher) -> Self {
        Self {
            namespaces,
            helm,
            jobs,
            debounce: REPORT_DEBOUNCE,
            page_size: LIST_PAGE_SIZE,
            sync_timeout: SYNC_TIMEOUT,
        }
    }

    pub fn from_settings(settings: &Settings) -> Result<Self, BuildError> {
        Ok(Self::new(
            settings.namespaces.clone(),
            HelmFilter::new(&settings.helm_hint_chart_globs)?,
            JobMatcher::new(&settings.sync_job_name_globs, settings.sync_job_label.as_ref())?,
        ))
    }
}

/// What trimming needs, shared by the watch tasks.
#[derive(Debug, Clone)]
struct TrimContext {
    guard: EnvGuard,
    helm: HelmFilter,
    jobs: JobTracker,
}

/// A kind of object the agent watches.
trait Watched:
    Resource<DynamicType = (), Scope = NamespaceResourceScope> + Clone + DeserializeOwned + Debug + Send + 'static
{
    type Rec: Send + 'static;
    const NAME: &'static str;
    /// Only Deployments hold environment values, so only they must list again when the allowlist changes.
    const FOLLOWS_ALLOWLIST: bool;
    fn trim(&self, namespace: &str, ctx: &TrimContext) -> Option<Self::Rec>;
    fn apply(projection: &mut Projection, namespace: &str, change: Change<Self::Rec>) -> Vec<DirtyKey>;
}

impl Watched for Deployment {
    type Rec = DeploymentRec;
    const NAME: &'static str = "deployments";
    const FOLLOWS_ALLOWLIST: bool = true;

    fn trim(&self, namespace: &str, ctx: &TrimContext) -> Option<DeploymentRec> {
        trim::trim_deployment(self, namespace, &ctx.guard, &ctx.helm)
    }

    fn apply(projection: &mut Projection, namespace: &str, change: Change<DeploymentRec>) -> Vec<DirtyKey> {
        projection.apply_deployment(namespace, change)
    }
}

impl Watched for Pod {
    type Rec = PodRec;
    const NAME: &'static str = "pods";
    const FOLLOWS_ALLOWLIST: bool = false;

    fn trim(&self, namespace: &str, _ctx: &TrimContext) -> Option<PodRec> {
        trim::trim_pod(self, namespace)
    }

    fn apply(projection: &mut Projection, namespace: &str, change: Change<PodRec>) -> Vec<DirtyKey> {
        projection.apply_pod(namespace, change)
    }
}

impl Watched for Job {
    type Rec = JobRec;
    const NAME: &'static str = "jobs";
    const FOLLOWS_ALLOWLIST: bool = false;

    fn trim(&self, namespace: &str, ctx: &TrimContext) -> Option<JobRec> {
        ctx.jobs.trim(namespace, self)
    }

    fn apply(projection: &mut Projection, namespace: &str, change: Change<JobRec>) -> Vec<DirtyKey> {
        projection.apply_job(namespace, change)
    }
}

/// State shared by the watch tasks, the report task and the handle.
struct Shared {
    /// Lock order: `projection`, then `listed`, then the debouncer's own lock.
    projection: Mutex<Projection>,
    /// The allowlist generation each namespace's Deployment list was made under.
    listed: Mutex<BTreeMap<String, u64>>,
    namespaces: Vec<String>,
    debouncer: ReportDebouncer<DirtyKey>,
    ctx: TrimContext,
    /// Bumped when the allowlist changes. Deployment watchers list again when it moves.
    generation: watch::Sender<u64>,
    /// True while every first list is in and every Deployment list is current.
    ready: watch::Sender<bool>,
}

impl Shared {
    fn lock_projection(&self) -> std::sync::MutexGuard<'_, Projection> {
        // A poisoned lock means a thread panicked mid-update; the maps are still whole, and carrying on is better than
        // an agent that cannot report at all.
        self.projection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_listed(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, u64>> {
        self.listed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_ready(&self, projection: &Projection) -> bool {
        let generation = *self.generation.borrow();
        let listed = self.lock_listed();
        projection.is_synced()
            && self
                .namespaces
                .iter()
                .all(|ns| listed.get(ns) == Some(&generation))
    }

    fn publish_ready(&self, projection: &Projection) {
        let ready = self.is_ready(projection);
        self.ready.send_if_modified(|current| {
            let changed = *current != ready;
            *current = ready;
            changed
        });
    }

    /// Turn one watcher event into a change of the projection. `generation` is the allowlist generation the stream was
    /// started under.
    fn ingest<K: Watched>(&self, namespace: &str, generation: u64, event: Event<K>) {
        let change = match event {
            Event::Init => Change::Init,
            Event::InitDone => Change::InitDone,
            Event::InitApply(object) => match object.trim(namespace, &self.ctx) {
                Some(record) => Change::InitApply(record),
                None => return,
            },
            Event::Apply(object) => match object.trim(namespace, &self.ctx) {
                Some(record) => Change::Apply(record),
                // An object that no longer qualifies (a Job whose labels changed, a pod that is terminating) leaves.
                None => match trim::key_of(object.meta(), namespace) {
                    Some(key) => Change::Delete(key),
                    None => return,
                },
            },
            Event::Delete(object) => match trim::key_of(object.meta(), namespace) {
                Some(key) => Change::Delete(key),
                None => return,
            },
        };
        let completes_list = matches!(change, Change::InitDone);
        let mut projection = self.lock_projection();
        let dirty = K::apply(&mut projection, namespace, change);
        self.debouncer.mark(dirty);
        if completes_list && K::FOLLOWS_ALLOWLIST {
            self.lock_listed().insert(namespace.to_owned(), generation);
        }
        self.publish_ready(&projection);
    }
}

/// Run one watcher until the task is aborted.
async fn watch_kind<K: Watched>(shared: Arc<Shared>, client: Client, namespace: String, page_size: u32) {
    let mut generation = shared.generation.subscribe();
    loop {
        let started_under = *generation.borrow_and_update();
        let api: Api<K> = Api::namespaced(client.clone(), &namespace);
        let config = watcher::Config::default().page_size(page_size);
        let mut stream = pin!(watcher::watcher(api, config).default_backoff());
        let ended = loop {
            tokio::select! {
                biased;
                moved = generation.changed(), if K::FOLLOWS_ALLOWLIST => {
                    if moved.is_err() {
                        return;
                    }
                    debug!(kind = K::NAME, namespace, "the env allowlist changed; listing again");
                    break false;
                }
                item = stream.next() => match item {
                    Some(Ok(event)) => shared.ingest::<K>(&namespace, started_under, event),
                    Some(Err(error)) => warn_watch_failed(K::NAME, &namespace, &error),
                    None => break true,
                },
            }
        };
        if ended {
            // The watcher stream never ends on its own. If it does, do not spin.
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Log why a watch failed: the kind, the namespace and the HTTP status. Never the server's message (S10).
fn warn_watch_failed(kind: &'static str, namespace: &str, error: &watcher::Error) {
    let (stage, status) = match error {
        watcher::Error::InitialListFailed(e) => ("list", api_status(e)),
        watcher::Error::WatchStartFailed(e) => ("watch start", api_status(e)),
        watcher::Error::WatchFailed(e) => ("watch", api_status(e)),
        watcher::Error::WatchError(status) => ("watch event", Some(status.code)),
        watcher::Error::NoResourceVersion => ("watch", None),
    };
    warn!(
        kind,
        namespace, stage, status, "the Kubernetes watch failed; trying again"
    );
}

fn api_status(error: &::kube::Error) -> Option<u16> {
    match error {
        ::kube::Error::Api(status) => Some(status.code),
        _ => None,
    }
}

/// Release debounced changes as delta reports.
async fn report_loop(shared: Arc<Shared>, reports: mpsc::Sender<ClusterReport>) {
    let mut ready = shared.ready.subscribe();
    loop {
        // Until every list is in, a delta would show Deployments without their pods.
        if ready.wait_for(|r| *r).await.is_err() {
            return;
        }
        let batch = shared.debouncer.next_batch().await;
        let report = shared.lock_projection().delta_report(&batch, &shared.ctx.guard);
        if let Some(report) = report {
            if reports.send(report).await.is_err() {
                return;
            }
        }
    }
}

/// The running watchers. Dropping it stops them.
pub struct ClusterWatcher {
    shared: Arc<Shared>,
    sync_timeout: Duration,
    _tasks: JoinSet<()>,
}

impl std::fmt::Debug for ClusterWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterWatcher")
            .field("namespaces", &self.shared.namespaces.len())
            .field("ready", &*self.shared.ready.borrow())
            .finish_non_exhaustive()
    }
}

impl ClusterWatcher {
    /// Start watching. The returned channel carries the delta reports; the first thing to send the hub is
    /// [`ClusterWatcher::full_report`]. Must be called inside a tokio runtime.
    pub fn start(client: &Client, config: WatchConfig) -> (Self, mpsc::Receiver<ClusterReport>) {
        let namespaces: Vec<String> = config.namespaces.iter().map(|n| n.as_str().to_owned()).collect();
        let shared = Arc::new(Shared {
            projection: Mutex::new(Projection::new(&namespaces)),
            listed: Mutex::new(BTreeMap::new()),
            namespaces: namespaces.clone(),
            debouncer: ReportDebouncer::new(config.debounce),
            ctx: TrimContext {
                guard: EnvGuard::new(),
                jobs: JobTracker::new(config.jobs, config.helm.clone()),
                helm: config.helm,
            },
            generation: watch::channel(0).0,
            ready: watch::channel(false).0,
        });
        let (reports, receiver) = mpsc::channel(REPORT_QUEUE);
        let mut tasks = JoinSet::new();
        for namespace in &namespaces {
            let page = config.page_size;
            tasks.spawn(watch_kind::<Deployment>(
                Arc::clone(&shared),
                client.clone(),
                namespace.clone(),
                page,
            ));
            tasks.spawn(watch_kind::<Pod>(
                Arc::clone(&shared),
                client.clone(),
                namespace.clone(),
                page,
            ));
            tasks.spawn(watch_kind::<Job>(
                Arc::clone(&shared),
                client.clone(),
                namespace.clone(),
                page,
            ));
        }
        tasks.spawn(report_loop(Arc::clone(&shared), reports));
        (
            Self {
                shared,
                sync_timeout: config.sync_timeout,
                _tasks: tasks,
            },
            receiver,
        )
    }

    /// Set the names whose values may be reported (`AgentConfig.env_allowlist`). When the list changes, the Deployment
    /// watchers list again and a full report waits for that.
    pub fn set_env_allowlist(&self, names: &[ShortText]) {
        if !self.shared.ctx.guard.set_allowlist(names) {
            return;
        }
        self.shared.generation.send_modify(|g| *g += 1);
        let projection = self.shared.lock_projection();
        self.shared.publish_ready(&projection);
    }

    /// Every watched namespace has listed its Deployments and Pods, under the current allowlist.
    pub fn is_ready(&self) -> bool {
        *self.shared.ready.borrow()
    }

    /// What the watchers retain, as text, for tests and diagnostics. It lists names, labels and permitted values; it is
    /// how a test shows that nothing else of an object is kept.
    pub fn retained_state(&self) -> String {
        format!("{:?}", *self.shared.lock_projection())
    }

    /// The sync Jobs that are running now (D75).
    pub fn active_sync_jobs(&self) -> Vec<JobRef> {
        self.shared.lock_projection().active_sync_jobs()
    }

    /// The whole picture, for the hub to replace what it has. It waits for the first lists (up to the sync timeout),
    /// and it covers every change so far, so the changes waiting for a delta are dropped.
    pub async fn full_report(&self) -> Result<ClusterReport, ReportError> {
        let mut ready = self.shared.ready.subscribe();
        let wait = async {
            loop {
                if ready.wait_for(|r| *r).await.is_err() {
                    return false;
                }
                let projection = self.shared.lock_projection();
                if self.shared.is_ready(&projection) {
                    return true;
                }
            }
        };
        match tokio::time::timeout(self.sync_timeout, wait).await {
            Ok(true) => {}
            Ok(false) | Err(_) => return Err(ReportError::NotSynced),
        }
        let projection = self.shared.lock_projection();
        if !self.shared.is_ready(&projection) {
            return Err(ReportError::NotSynced);
        }
        let report = projection.full_report(&self.shared.ctx.guard);
        // Marks are made under this lock, so nothing slips in between the report and the clear.
        self.shared.debouncer.clear();
        Ok(report)
    }
}
