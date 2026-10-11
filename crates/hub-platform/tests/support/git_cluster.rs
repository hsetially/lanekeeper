//! N hub replicas' Git machinery in one process over one real Postgres (T7): each has its own mirror on its own disk, its own
//! event bus (so `GitHeadMoved` really crosses Postgres), its own fetch queue and worker, and its own webhook endpoint. All of
//! them mirror the same two local "remotes", which the test pushes to with `git fast-import`.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use domain::{CommitId, DomainEvent, GitRef, RepoKind};
use futures::StreamExt;
use futures::stream::BoxStream;
use hub_platform::events::bus::BusSettings;
use hub_platform::events::sse::SseSettings;
use hub_platform::git::credentials::NoCredentials;
use hub_platform::git::deliveries::PgDeliveryLog;
use hub_platform::git::fetch_queue::FetchQueue;
use hub_platform::git::metrics::GitMetrics;
use hub_platform::git::mirror::{Mirror, MirrorSettings, RemoteUrl};
use hub_platform::git::payload::RepoFullName;
use hub_platform::git::sync::{Announcer, FetchWorker, MirrorSync, MirrorSyncer, RetrySettings};
use hub_platform::git::webhook::{self, WebhookSettings, WebhookState};
use hub_platform::{FakeSessionAuth, GuardedRouter};
use ports::fakes::FakeSecretSource;
use ports::{EventBus, EventFilter, EventKind};
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tower::ServiceExt;

use super::event_cluster::EventCluster;
use super::fake_github::{Delivery, GitRemote, push_body};

pub const SECRET: &str = "whsec_ClusterWebhookSecret";
pub const BASE_REPO: &str = "acme/configuration-base-saas";
pub const TENANT_REPO: &str = "acme/csp-tenant-data";

pub struct GitReplica {
    pub name: String,
    pub mirror: Arc<Mirror>,
    pub announcer: Arc<Announcer>,
    pub queue: Arc<FetchQueue>,
    pub metrics: GitMetrics,
    pub bus: Arc<dyn EventBus>,
    pub syncer: Arc<MirrorSyncer>,
    pub webhook: Router,
    /// Every `GitHeadMoved` this replica's bus delivered since the cluster started.
    pub events: BoxStream<'static, DomainEvent>,
    _dir: TempDir,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for GitReplica {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub struct GitCluster {
    pub events: EventCluster,
    pub base: GitRemote,
    pub tenant: GitRemote,
    pub replicas: Vec<GitReplica>,
    stop: watch::Sender<bool>,
}

impl GitCluster {
    /// Both remotes start with one branch each, so every mirror is initialized from something.
    pub async fn start(n: usize) -> Option<Self> {
        let base = GitRemote::new();
        let tenant = GitRemote::new();
        base.commit("main", &[("config/a.yml", Some(b"a: 1\n"))], "base");
        tenant.commit("sit1", &[("data/config/a-sit1.yml", Some(b"a: 1\n"))], "sit1");
        Self::start_over(n, base, tenant).await
    }

    /// The same over remotes that already hold content (the target-scale fixtures, copied).
    pub async fn start_over(n: usize, base: GitRemote, tenant: GitRemote) -> Option<Self> {
        let events = EventCluster::start(n, BusSettings::default(), SseSettings::default()).await?;
        let (stop, stop_rx) = watch::channel(false);
        let mut replicas = Vec::new();
        for (i, replica) in events.replicas.iter().enumerate() {
            let dir = TempDir::new().unwrap();
            let metrics = GitMetrics::default();
            let settings = MirrorSettings::new(
                dir.path(),
                RemoteUrl::local(&base.url()),
                RemoteUrl::local(&tenant.url()),
            );
            let mirror = Mirror::open(settings, Arc::new(NoCredentials), metrics.clone()).unwrap();
            mirror.initialize().await.unwrap();
            let bus: Arc<dyn EventBus> = Arc::new(replica.bus.clone());
            let announcer = Announcer::start(Arc::clone(&mirror), Arc::clone(&bus), metrics.clone()).await;
            let queue = FetchQueue::new(64);
            let syncer = Arc::new(MirrorSyncer::new(Arc::clone(&mirror), Arc::clone(&announcer)));
            let retry = RetrySettings {
                attempts: 3,
                base: Duration::from_millis(20),
                cap: Duration::from_millis(100),
            };
            let worker = FetchWorker::new(Arc::clone(&queue), syncer.clone(), retry).spawn(stop_rx.clone());
            let sync = MirrorSync::new(Arc::clone(&mirror), Arc::clone(&announcer), Arc::clone(&bus))
                .spawn(stop_rx.clone());
            let events_stream = bus.subscribe(EventFilter {
                swimlanes: None,
                kinds: [EventKind::GitHeadMoved].into(),
            });
            let state = WebhookState::new(
                WebhookSettings::new(
                    "webhook-secret",
                    vec![
                        (RepoKind::Base, RepoFullName::parse(BASE_REPO).unwrap()),
                        (RepoKind::Tenant, RepoFullName::parse(TENANT_REPO).unwrap()),
                    ],
                ),
                Arc::new(FakeSecretSource::new().with("webhook-secret", SECRET)),
                Arc::new(PgDeliveryLog::new(replica.pool.clone())),
                Arc::clone(&queue),
                metrics.clone(),
            );
            let (router, _) = webhook::mount(
                GuardedRouter::new(Arc::new(FakeSessionAuth::new()), None),
                Arc::new(state),
            )
            .into_parts()
            .unwrap();
            replicas.push(GitReplica {
                name: format!("hub-{i}"),
                mirror,
                announcer,
                queue,
                metrics,
                bus,
                syncer,
                webhook: router,
                events: events_stream,
                _dir: dir,
                tasks: vec![worker, sync],
            });
        }
        Some(Self {
            events,
            base,
            tenant,
            replicas,
            stop,
        })
    }

    pub fn remote(&self, repo: RepoKind) -> &GitRemote {
        match repo {
            RepoKind::Base => &self.base,
            RepoKind::Tenant => &self.tenant,
        }
    }

    /// Deliver a signed `push` webhook for `branch` of `repo` to replica `to`, as GitHub would after the remote moved.
    pub async fn deliver_push(
        &self,
        to: usize,
        repo: RepoKind,
        branch: &str,
        before: &str,
        after: &str,
        id: &str,
    ) -> http::StatusCode {
        let name = match repo {
            RepoKind::Base => BASE_REPO,
            RepoKind::Tenant => TENANT_REPO,
        };
        let body = push_body(name, &format!("refs/heads/{branch}"), before, after);
        let delivery = Delivery::signed(SECRET, "push", id, body);
        self.replicas[to]
            .webhook
            .clone()
            .oneshot(delivery.request())
            .await
            .unwrap()
            .status()
    }
}

impl Drop for GitCluster {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// The next `GitHeadMoved` a replica's bus delivers, or `None` after `within`.
pub async fn next_head_moved(
    events: &mut BoxStream<'static, DomainEvent>,
    within: Duration,
) -> Option<(RepoKind, GitRef, Option<CommitId>, CommitId)> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.checked_duration_since(tokio::time::Instant::now())?;
        match tokio::time::timeout(left, events.next()).await {
            Ok(Some(DomainEvent::GitHeadMoved {
                repo,
                branch,
                from,
                to,
            })) => return Some((repo, branch, from, to)),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return None,
        }
    }
}

/// Wait up to `within` for `condition`, polling every 20 ms.
pub async fn eventually(what: &str, within: Duration, mut condition: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    loop {
        if condition() {
            return start.elapsed();
        }
        assert!(start.elapsed() < within, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
