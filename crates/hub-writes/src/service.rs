//! `WriteHub`: the `WriteService` implementation, and the state its steps share.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{
    ChangePreview, DeleteRequest, Draft, DraftId, EditRequest, NfsPath, NotifyRequest, PrRequest,
    PreviewRequest, RestartRequest, RevertRequest, Role, ServiceRef, SwimlaneId, UploadRequest, WriteCtx,
    WriteOutcome,
};
use engine::{DenyMatcher, Limits};
use ports::{
    AgentGateway, AuditLog, BlobStore, EventBus, Leases, Notifier, RegistryRead, SecretSource, ToolWrites,
    Users, WriteError, WriteService,
};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::Instrument;

use crate::clock::Clock;
use crate::config::{PREVIEW_KEY_SECRET, WriteConfig};
use crate::error::{ConfigError, Internal, Res};
use crate::idempotency::IdempotencyStore;
use crate::pipeline::authorize::authorize;
use crate::preview::token::PreviewKey;
use crate::reconcile::ReconcileWrites;

/// What the write hub is built from: the ports it uses, the pool for its own tables, and the settings.
pub struct WriteHubDeps {
    pub pool: PgPool,
    pub users: Arc<dyn Users>,
    pub audit: Arc<dyn AuditLog>,
    pub bus: Arc<dyn EventBus>,
    pub gateway: Arc<dyn AgentGateway>,
    pub blobs: Arc<dyn BlobStore>,
    pub registry: Arc<dyn RegistryRead>,
    pub tool_writes: Arc<dyn ToolWrites>,
    pub notifier: Arc<dyn Notifier>,
    /// The preview token's key is read from here (secret [`PREVIEW_KEY_SECRET`]).
    pub secrets: Arc<dyn SecretSource>,
    /// One restart per service at a time, across replicas.
    pub leases: Arc<dyn Leases>,
    pub clock: Arc<dyn Clock>,
    pub config: WriteConfig,
}

impl std::fmt::Debug for WriteHubDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteHubDeps").finish_non_exhaustive()
    }
}

/// The state every step of the pipeline reads. Shared behind one `Arc`, so a request, a replica's reconcile job and a retry all
/// see the same ports.
pub(crate) struct Inner {
    pub pool: PgPool,
    pub users: Arc<dyn Users>,
    pub audit: Arc<dyn AuditLog>,
    pub bus: Arc<dyn EventBus>,
    pub gateway: Arc<dyn AgentGateway>,
    pub blobs: Arc<dyn BlobStore>,
    pub registry: Arc<dyn RegistryRead>,
    pub tool_writes: Arc<dyn ToolWrites>,
    pub notifier: Arc<dyn Notifier>,
    pub leases: Arc<dyn Leases>,
    pub preview_key: PreviewKey,
    pub clock: Arc<dyn Clock>,
    pub cfg: WriteConfig,
    pub deny: Arc<DenyMatcher>,
    pub limits: Limits,
    pub idem: IdempotencyStore,
    /// Bounds the validations that run at once on the blocking pool.
    pub checks: Semaphore,
}

impl Inner {
    pub fn now_ms(&self) -> i64 {
        self.clock.now().unix_millis()
    }

    /// Runs CPU-bound work (a parse, a scan, a diff) on the blocking pool (rule 4), at most `max_concurrent_checks` at a time.
    pub async fn blocking<T: Send + 'static>(
        &self,
        stage: &'static str,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Res<T> {
        let _permit = self
            .checks
            .acquire()
            .await
            .map_err(|_| Internal::new(stage, "worker pool"))?;
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|_| Internal::new(stage, "blocking task"))
    }

    /// Runs a call that has no timeout of its own within `db_timeout` (rule 5).
    pub async fn within<T>(&self, stage: &'static str, fut: impl Future<Output = T>) -> Res<T> {
        tokio::time::timeout(self.cfg.db_timeout, fut)
            .await
            .map_err(|_| Internal::timeout(stage))
    }
}

/// Milliseconds of a duration, saturating.
pub(crate) fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// The write service (`docs/interfaces.md`): every change goes through the pipeline in [`crate::pipeline`].
#[derive(Clone)]
pub struct WriteHub {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for WriteHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteHub").finish_non_exhaustive()
    }
}

impl WriteHub {
    /// # Errors
    /// [`ConfigError`] when a setting is out of bounds or a deny glob does not compile.
    pub fn new(deps: WriteHubDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let deny = DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::DenyGlob)?;
        let max_checks = deps.config.max_concurrent_checks;
        let mut limits = Limits::DEFAULT;
        limits.input_bytes = deps.config.max_content_bytes;
        Ok(Self {
            inner: Arc::new(Inner {
                idem: IdempotencyStore::new(deps.pool.clone()),
                pool: deps.pool,
                users: deps.users,
                audit: deps.audit,
                bus: deps.bus,
                gateway: deps.gateway,
                blobs: deps.blobs,
                registry: deps.registry,
                tool_writes: deps.tool_writes,
                notifier: deps.notifier,
                leases: deps.leases,
                preview_key: PreviewKey::new(deps.secrets, PREVIEW_KEY_SECRET),
                clock: deps.clock,
                cfg: deps.config,
                deny: Arc::new(deny),
                limits,
                checks: Semaphore::new(max_checks),
            }),
        })
    }

    /// The most one file may weigh, for [`crate::routes::WriteState::new`]: the routes limit a form to it before they read it.
    pub fn max_content_bytes(&self) -> usize {
        self.inner.cfg.max_content_bytes
    }

    /// The job that finishes writes that landed without being recorded, and sweeps expired idempotency rows. Run it under a
    /// lease (D63).
    pub fn reconciler(&self) -> ReconcileWrites {
        ReconcileWrites::new(self.inner.clone())
    }

    /// The services a "restart all" confirmation lists for a change to `path` (D49): every service of the swimlane for a file in
    /// the root folder, which every service reads (D82), and the Deployments of the application for a file in its folder. The
    /// screen restarts them one request at a time, each with its own idempotency key. Operator only.
    ///
    /// # Errors
    /// [`WriteError::Forbidden`] below Operator, [`WriteError::Unavailable`] when the store does not answer.
    pub async fn restart_offer(
        &self,
        ctx: &WriteCtx,
        swimlane: &SwimlaneId,
        path: &NfsPath,
    ) -> Result<Vec<ServiceRef>, WriteError> {
        crate::ops::restart::restart_offer(&self.inner, ctx, swimlane, path).await
    }
}

/// Actions that are authorized but not yet carried out by this build (later tasks of prompt 06 wire them): the caller is
/// told the service is unavailable, after the role check, with nothing claimed, sent or recorded.
async fn not_wired<T>(hub: &WriteHub, ctx: &WriteCtx, floor: Role) -> Result<T, WriteError> {
    authorize(&hub.inner, ctx, floor).await?;
    Err(WriteError::Unavailable)
}

#[async_trait]
impl WriteService for WriteHub {
    async fn edit(&self, ctx: &WriteCtx, r: EditRequest) -> Result<WriteOutcome, WriteError> {
        let span = tracing::info_span!(
            "write.edit",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        crate::ops::edit::edit(&self.inner, ctx, r).instrument(span).await
    }

    async fn upload(&self, ctx: &WriteCtx, r: UploadRequest) -> Result<WriteOutcome, WriteError> {
        let span = tracing::info_span!(
            "write.upload",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        crate::ops::upload::upload(&self.inner, ctx, r)
            .instrument(span)
            .await
    }

    async fn delete(&self, ctx: &WriteCtx, r: DeleteRequest) -> Result<WriteOutcome, WriteError> {
        let span = tracing::info_span!(
            "write.delete",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        crate::ops::delete::delete(&self.inner, ctx, r)
            .instrument(span)
            .await
    }

    async fn revert(&self, ctx: &WriteCtx, r: RevertRequest) -> Result<WriteOutcome, WriteError> {
        let span = tracing::info_span!(
            "write.revert",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        crate::ops::revert::revert(&self.inner, ctx, r)
            .instrument(span)
            .await
    }

    async fn restart(&self, ctx: &WriteCtx, r: RestartRequest) -> Result<WriteOutcome, WriteError> {
        let span = tracing::info_span!(
            "write.restart",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
        );
        crate::ops::restart::restart(&self.inner, ctx, r)
            .instrument(span)
            .await
    }

    async fn raise_pr(&self, ctx: &WriteCtx, _r: PrRequest) -> Result<WriteOutcome, WriteError> {
        not_wired(self, ctx, Role::Editor).await
    }

    async fn propose_draft(&self, ctx: &WriteCtx, _r: EditRequest) -> Result<Draft, WriteError> {
        not_wired(self, ctx, Role::Editor).await
    }

    async fn apply_draft(&self, ctx: &WriteCtx, _id: DraftId) -> Result<WriteOutcome, WriteError> {
        not_wired(self, ctx, Role::Editor).await
    }

    async fn preview_effect(&self, ctx: &WriteCtx, r: PreviewRequest) -> Result<ChangePreview, WriteError> {
        let span = tracing::info_span!(
            "write.preview",
            request_id = %ctx.request_id,
            user = %ctx.user.id,
            swimlane = %r.swimlane,
            path = %r.path,
        );
        crate::preview::effect::preview_effect(&self.inner, ctx, r)
            .instrument(span)
            .await
    }

    async fn notify(&self, ctx: &WriteCtx, _r: NotifyRequest) -> Result<WriteOutcome, WriteError> {
        not_wired(self, ctx, Role::Operator).await
    }
}
