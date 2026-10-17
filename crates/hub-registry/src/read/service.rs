//! `RegistryService`: what the read modules share (T6). The `RegistryRead` and `RegistryApi` implementations are in
//! [`super::api`]; each read is a method of [`Inner`] in its own module.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use engine::{DenyMatcher, Limits, ScanConfig};
use ports::{AgentGateway, BlobStore, GitReader, ReadError};
use sqlx::PgPool;

use super::cache::Caches;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::Db;
use crate::metrics::{ReadOp, ReadOutcome, RegistryMetrics};

/// What the read service needs from the rest of the hub.
pub struct ReadDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub blobs: Arc<dyn BlobStore>,
    pub git: Arc<dyn GitReader>,
    pub gateway: Arc<dyn AgentGateway>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for ReadDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadDeps").finish_non_exhaustive()
    }
}

pub struct Inner {
    pub db: Db,
    pub blobs: Arc<dyn BlobStore>,
    pub git: Arc<dyn GitReader>,
    pub gateway: Arc<dyn AgentGateway>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// The read pool: text search runs here, inside `spawn_blocking` (rule 4).
    pub pool: Arc<rayon::ThreadPool>,
    pub limits: Limits,
    pub scan: ScanConfig,
    pub deny: Arc<DenyMatcher>,
    pub caches: Caches,
    /// Bounds the `FetchServed` calls in flight.
    pub served_slots: tokio::sync::Semaphore,
    /// Makes the request ids of the commands sent to agents.
    pub request_seq: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner").finish_non_exhaustive()
    }
}

/// The read service. Cloning shares it. It implements [`ports::RegistryRead`] and [`super::RegistryApi`].
#[derive(Clone)]
pub struct RegistryService {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for RegistryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryService").finish_non_exhaustive()
    }
}

impl RegistryService {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range, or the thread pool cannot be built.
    pub fn new(deps: ReadDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(deps.config.read_pool_threads)
            .thread_name(|i| format!("registry-read-{i}"))
            .build()
            .map_err(|_| ConfigError::Invalid("read_pool_threads"))?;
        let limits = Limits {
            input_bytes: usize::try_from(deps.config.max_file_bytes).unwrap_or(usize::MAX),
            // The server's ceilings on a text search (A27): a request above them is clamped by the engine, which says so by `truncated`.
            hits_total: usize::try_from(deps.config.search_max_total).unwrap_or(usize::MAX),
            hits_per_file: usize::try_from(deps.config.search_max_per_file).unwrap_or(usize::MAX),
            ..Limits::default()
        };
        let deny = DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        let caches = Caches::new(&deps.config, deps.metrics.clone());
        let slots = deps.config.served_concurrency;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                blobs: deps.blobs,
                git: deps.git,
                gateway: deps.gateway,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
                pool: Arc::new(pool),
                limits,
                scan: ScanConfig::default(),
                deny: Arc::new(deny),
                caches,
                served_slots: tokio::sync::Semaphore::new(slots),
                request_seq: std::sync::atomic::AtomicU64::new(0),
            }),
        })
    }

    /// Drop every cache, so that the next read is a cold one. For tests and benchmarks; nothing in the service calls it.
    pub fn clear_caches(&self) {
        self.inner.caches.clear();
    }

    /// The rayon pool the text searches and the merges run on. Tests occupy it to prove that a search is cancelled at its deadline.
    pub fn read_pool(&self) -> Arc<rayon::ThreadPool> {
        Arc::clone(&self.inner.pool)
    }

    /// The counters of this service.
    pub fn metrics(&self) -> &RegistryMetrics {
        &self.inner.metrics
    }
}

impl Inner {
    /// Run a read, count it and time it. `op` and the outcome are the only labels (S10).
    pub async fn timed<T, F>(&self, op: ReadOp, call: F) -> Result<T, ReadError>
    where
        F: Future<Output = Result<T, ReadError>>,
    {
        let started = Instant::now();
        let result = call.await;
        let outcome = match &result {
            Ok(_) => ReadOutcome::Ok,
            Err(ReadError::Forbidden) => ReadOutcome::Forbidden,
            Err(ReadError::NotFound) => ReadOutcome::NotFound,
            Err(ReadError::BadRequest) => ReadOutcome::BadRequest,
            Err(ReadError::Unavailable) => ReadOutcome::Unavailable,
        };
        self.metrics.read(op, outcome, started.elapsed().as_secs_f64());
        result
    }

    /// Count a read that was answered "not modified" (the content reads, which carry a tag).
    pub fn not_modified(&self, op: ReadOp, started: Instant) {
        self.metrics
            .read(op, ReadOutcome::NotModified, started.elapsed().as_secs_f64());
    }
}
