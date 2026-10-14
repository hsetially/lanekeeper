//! A world with the registry's syncs on top: a `SyncService` over the world's database, Git, audit log, bus and clock, an agent gateway
//! whose agent can be scripted, and a notifier that remembers what it was asked to send.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use domain::{
    Actor, AgentReply, CommitId, HubCommand, JobRef, OpResult, RequestId, ScanEntry, SwimlaneId, TenantId,
};
use hub_registry::{RegistryConfig, SyncDeps, SyncOutcome, SyncService, SyncSource};
use ports::fakes::{FakeAgentGateway, FakeNotifier};
use ports::{ReportSink, content_hash};

use super::rig::{delta, entry};
use super::world::World;

/// The sequence numbers of the scans the scripted agent sends: far above anything a world sends.
static NEXT_SEQ: AtomicU64 = AtomicU64::new(10_000);

/// The configuration the sync tests run under: the same as the default, but the scan is looked for every 10 ms.
pub fn fast() -> RegistryConfig {
    RegistryConfig {
        sync_poll_interval: Duration::from_millis(10),
        ..RegistryConfig::default()
    }
}

pub struct SyncRig {
    pub w: World,
    pub service: SyncService,
    pub gateway: FakeAgentGateway,
    pub notifier: FakeNotifier,
}

pub fn lane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

pub fn tenant(name: &str) -> TenantId {
    TenantId::parse(name).unwrap()
}

pub fn job() -> Actor {
    Actor::Job {
        job: JobRef::new("sync-sit1", "uid-9").unwrap(),
    }
}

pub fn source() -> SyncSource {
    SyncSource::parse("job-report").unwrap()
}

impl SyncRig {
    /// A swimlane `sit1` that deploys `tenants`. `None` when there is no database.
    pub async fn new(tenants: &[&str]) -> Option<Self> {
        Self::with_config(tenants, fast()).await
    }

    pub async fn with_config(tenants: &[&str], config: RegistryConfig) -> Option<Self> {
        let w = World::with_config(tenants, config.clone()).await?;
        let gateway = FakeAgentGateway::new();
        let notifier = FakeNotifier::new();
        let service = SyncService::new(SyncDeps {
            pool: w.rig.pool.clone(),
            git: Arc::new(w.git.clone()),
            bus: Arc::clone(&w.rig.bus) as _,
            audit: Arc::clone(&w.rig.audit) as _,
            gateway: Arc::new(gateway.clone()),
            notifier: Arc::new(notifier.clone()),
            clock: Arc::clone(&w.rig.clock) as _,
            config,
            metrics: w.rig.metrics.clone(),
            pipeline: Some(w.handle.clone()),
        })
        .unwrap();
        Some(Self {
            w,
            service,
            gateway,
            notifier,
        })
    }

    /// An agent that answers a request for a full scan by sending `files` as one, the way the real one does: the request is
    /// acknowledged at once, and the scan comes through ingestion after it. A file the hub has that `files` does not name is gone.
    pub fn agent_scans(&self, files: &[(&str, &str)]) {
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
            .collect();
        let (ingest, identity, clock) = (
            self.w.rig.ingest.clone(),
            self.w.rig.identity.clone(),
            Arc::clone(&self.w.rig.clock),
        );
        self.gateway.attach(
            &lane(),
            Arc::new(move |cmd: &HubCommand| {
                if !matches!(cmd, HubCommand::RequestFullScan) {
                    return None;
                }
                let at = ManualNow::of(&clock);
                let entries: Vec<ScanEntry> = files.iter().map(|(p, c)| entry(p, c.as_bytes(), at)).collect();
                let seq = NEXT_SEQ.fetch_add(1, Ordering::SeqCst);
                let root = content_hash(&seq.to_be_bytes());
                let (ingest, identity) = (ingest.clone(), identity.clone());
                tokio::spawn(async move {
                    ingest
                        .delta(&identity, delta(seq, None, root, entries))
                        .await
                        .unwrap();
                });
                Some(AgentReply::Op(OpResult {
                    request_id: RequestId::parse("ack").unwrap(),
                    ok: true,
                    error: None,
                    current_hash: None,
                }))
            }),
        );
    }

    /// Another service over the same world with another notifier, to show what a notifier that fails or hangs does.
    pub fn service_with(&self, notifier: Arc<dyn ports::Notifier>, config: RegistryConfig) -> SyncService {
        SyncService::new(SyncDeps {
            pool: self.w.rig.pool.clone(),
            git: Arc::new(self.w.git.clone()),
            bus: Arc::clone(&self.w.rig.bus) as _,
            audit: Arc::clone(&self.w.rig.audit) as _,
            gateway: Arc::new(self.gateway.clone()),
            notifier,
            clock: Arc::clone(&self.w.rig.clock) as _,
            config,
            metrics: self.w.rig.metrics.clone(),
            pipeline: Some(self.w.handle.clone()),
        })
        .unwrap()
    }

    /// The commands the agent was sent.
    pub fn commands(&self) -> Vec<HubCommand> {
        self.gateway.received(&lane())
    }

    /// Report a sync of `tenant_name` at the given commits, by a Job, from the `job-report` source.
    pub async fn sync(
        &self,
        tenant_name: &str,
        tenant_commit: &CommitId,
        base_commit: &CommitId,
    ) -> Result<SyncOutcome, hub_registry::SyncError> {
        self.service
            .record_sync(
                &lane(),
                &tenant(tenant_name),
                tenant_commit,
                base_commit,
                &job(),
                &source(),
            )
            .await
    }
}

/// The hub's time, as a scan's `observed_at`.
struct ManualNow;

impl ManualNow {
    fn of(clock: &hub_registry::ManualClock) -> i64 {
        use hub_registry::Clock;
        clock.now().unix_millis()
    }
}
