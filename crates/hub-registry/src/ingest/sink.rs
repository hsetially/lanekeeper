//! `AgentIngest`: the [`ReportSink`] over Postgres.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::{AgentConfig, ClusterReport, Heartbeat, HeartbeatAction, Hello, ScanDelta, SwimlaneId};
use engine::DenyMatcher;
use ports::{AgentIdentity, AuditLog, BlobStore, EventBus, ReportSink, SinkError};
use sqlx::PgPool;

use super::cluster::ClusterApplier;
use super::delta::{Accepted, DeltaAssembler};
use super::tool_writes::ToolWrites;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::error::RegistryError;
use crate::metrics::{MessageKind, Outcome, RegistryMetrics};

/// Everything ingestion needs from the rest of the hub.
pub struct IngestDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for IngestDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngestDeps").finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub db: Db,
    pub blobs: Arc<dyn BlobStore>,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    pub deny: DenyMatcher,
    /// Swimlanes that said `hello` on this replica. Bounded by `max_streams`.
    streams: Mutex<HashSet<SwimlaneId>>,
    /// One lock per swimlane, so that its deltas are applied one at a time on this replica. Keyed like `streams`.
    locks: Mutex<HashMap<SwimlaneId, Arc<tokio::sync::Mutex<()>>>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    pub(crate) fn said_hello(&self, s: &SwimlaneId) -> bool {
        locked(&self.streams).contains(s)
    }

    /// Make room for a swimlane that is saying `hello`. `Ok(true)` when it was not in yet (the caller gives the place back with
    /// [`Inner::forget_stream`] if the `hello` then fails), `Ok(false)` when it already was (a reconnecting agent is never
    /// refused for want of room). A new swimlane over the bound is refused (rule 5).
    pub(crate) fn admit_stream(&self, s: &SwimlaneId) -> Result<bool, RegistryError> {
        let mut streams = locked(&self.streams);
        if streams.contains(s) {
            return Ok(false);
        }
        if streams.len() >= self.config.max_streams {
            return Err(RegistryError::Capacity);
        }
        streams.insert(s.clone());
        self.metrics.streams(streams.len());
        locked(&self.locks).entry(s.clone()).or_default();
        Ok(true)
    }

    /// Take back a place that [`Inner::admit_stream`] gave.
    pub(crate) fn forget_stream(&self, s: &SwimlaneId) {
        let mut streams = locked(&self.streams);
        streams.remove(s);
        self.metrics.streams(streams.len());
        locked(&self.locks).remove(s);
    }

    pub(crate) fn lock_for(&self, s: &SwimlaneId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(locked(&self.locks).entry(s.clone()).or_default())
    }
}

/// The registry's side of the agent protocol.
#[derive(Clone)]
pub struct AgentIngest {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AgentIngest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentIngest").finish_non_exhaustive()
    }
}

impl AgentIngest {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range or a deny glob does not compile.
    pub fn new(deps: IngestDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let deny = DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                blobs: deps.blobs,
                bus: deps.bus,
                audit: deps.audit,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
                deny,
                streams: Mutex::new(HashSet::new()),
                locks: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The seam with prompt 06 (A10): it registers a tool write in its own transaction before it sends `WriteFile`.
    pub fn tool_writes(&self) -> ToolWrites {
        ToolWrites::new(Arc::clone(&self.inner.bus), Arc::clone(&self.inner.clock))
    }

    pub fn metrics(&self) -> &RegistryMetrics {
        &self.inner.metrics
    }

    /// Drop staged parts that have waited longer than `staging_ttl` (A14). Also done on every `hello`; a job may call it too.
    pub async fn sweep_staging(&self) -> Result<u64, RegistryError> {
        self.inner.sweep_staging().await
    }

    fn done<T>(&self, kind: MessageKind, result: Result<T, RegistryError>) -> Result<T, SinkError> {
        match result {
            Ok(v) => {
                self.inner.metrics.message(kind, Outcome::Ok);
                Ok(v)
            }
            Err(e) => {
                let e = SinkError::from(e);
                self.inner.metrics.message(kind, outcome_of(e));
                Err(e)
            }
        }
    }
}

fn outcome_of(e: SinkError) -> Outcome {
    match e {
        SinkError::HelloRequired => Outcome::HelloRequired,
        SinkError::IdentityMismatch => Outcome::IdentityMismatch,
        SinkError::Invalid => Outcome::Invalid,
        SinkError::TooLarge => Outcome::TooLarge,
        SinkError::Unavailable => Outcome::Unavailable,
    }
}

#[async_trait]
impl ReportSink for AgentIngest {
    async fn hello(&self, id: &AgentIdentity, h: Hello) -> Result<AgentConfig, SinkError> {
        let result = self.inner.hello(id, h).await;
        self.done(MessageKind::Hello, result)
    }

    async fn heartbeat(&self, id: &AgentIdentity, hb: Heartbeat) -> Result<HeartbeatAction, SinkError> {
        let result = self.inner.heartbeat(id, hb).await;
        self.done(MessageKind::Heartbeat, result)
    }

    async fn delta(&self, id: &AgentIdentity, d: ScanDelta) -> Result<(), SinkError> {
        let result = DeltaAssembler::new(&self.inner).accept(id, d).await;
        match result {
            Ok(accepted) => {
                let outcome = match accepted {
                    Accepted::Staged => Outcome::Staged,
                    Accepted::Applied(report) => {
                        // Ids and counts only (S10).
                        tracing::debug!(
                            swimlane = %id.swimlane(),
                            recorded = report.recorded,
                            removed = report.removed,
                            history_only = report.history_only,
                            unchanged = report.unchanged,
                            replayed = report.replayed,
                            out_of_band = report.out_of_band,
                            paths = report.paths,
                            chunks = report.chunks,
                            "scan delta applied"
                        );
                        Outcome::Ok
                    }
                };
                self.inner.metrics.message(MessageKind::Delta, outcome);
                Ok(())
            }
            Err(e) => {
                let e = SinkError::from(e);
                self.inner.metrics.message(MessageKind::Delta, outcome_of(e));
                Err(e)
            }
        }
    }

    async fn cluster(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), SinkError> {
        let result = ClusterApplier::new(&self.inner).apply(id, c).await;
        self.done(MessageKind::Cluster, result)
    }
}
