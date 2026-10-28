//! The write paths in the composed hub (plan 18, T5; S4, S7, S8, S9, S10, D41 to D49, D77, D85, D107): edits, uploads, deletes,
//! reverts, restarts, notifies, proposals and drafts, the GitHub token vault, pull requests, and the Teams notifier.
//!
//! `hub-writes` is a library with a service ([`WriteHub`]) and workers that are bare `run_once` methods; it does not wire itself
//! (plan A15). This is the order the parts are put together in, from the settings, the platform's ports and the registry's two:
//!
//! 1. [`teams`]: the [`TeamsNotifier`], made first because the identity module and the registry take a notifier too. A failing
//!    webhook never fails the action that caused it.
//! 2. [`WriteHub`], over the user directory, the audit log, the bus, the gateway, the blob store, the registry's read port and
//!    [`PgToolWrites`](hub_registry::PgToolWrites). Every applied write has exactly one audit event, recorded in the same transaction
//!    as its tool-write registration, its outbox event and its stored response (rule 8, D80).
//! 3. The token vault, over the KMS envelope and GitHub, and the pull-request jobs. A pull-request job may use its author's token
//!    at job time (D107); the vault re-reads the author from the directory at every step.
//! 4. The routes: [`WriteState`] and `routes::mount`, 18 operations, each behind the hub's guard and the hub's CSRF layer.
//! 5. The four leased jobs ([`crate::job_wrappers`]) and the two tasks that are the process's own: the watcher that turns a
//!    pull-request state change into a `DriftChanged`, and the batcher that announces auto-notified writes.
//!
//! **One list of deny globs** (see [`crate::registry`]): the write path is built from the registry's list, so a path withheld by the
//! read API is refused by the write path too.
//!
//! **The preview key** (D46) is the Secret Manager secret `write-preview-key`, read lazily by the crate. It is read once here at
//! start, so that a deployment without it is an exit that names it and not a `503` on the first upload.

use std::sync::Arc;
use std::time::Duration;

use hub_platform::GuardedRouter;
use hub_platform::clock::Clock;
use hub_platform::jobs::JobRunner;
use hub_writes::github::RepoRef;
use hub_writes::prs::{PrDeps, PrSettings};
use hub_writes::routes::WriteState;
use hub_writes::teams::{TeamsConfig, TeamsDeps, TeamsNotifier};
use hub_writes::vault::{PolicyRepos, VaultDeps};
use hub_writes::{PREVIEW_KEY_SECRET, WriteConfig, WriteHub, WriteHubDeps};
use ports::{
    AgentGateway, AuditLog, BlobStore, EventBus, Leases, Notifier, RegistryRead, SecretSource, ToolWrites,
    Users,
};
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::info;

use crate::compose::Module;
use crate::config::{GitSettings, RegistrySettings, WritesSettings};
use crate::edges::{WriteEdges, WritesClock};
use crate::job_wrappers::{self, JobEntry};
use crate::wiring::Tasks;

/// The shortest preview key `hub-writes` signs with (`MIN_KEY_BYTES`, which the crate keeps to itself).
const MIN_PREVIEW_KEY_BYTES: usize = 32;

/// Why the write paths could not be set up. Every message is fixed text that names a variable or a part, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WritesBuildError {
    #[error("the Teams notifier could not be set up (LK_TEAMS_ALLOWED_HOSTS, LK_PUBLIC_ORIGIN)")]
    Teams,
    /// The crate's own message names the field; it is fixed text.
    #[error("the write path refused its configuration: {0}")]
    Config(&'static str),
    #[error("a repository name is not one GitHub takes (LK_GIT_BASE_REPO, LK_GIT_TENANT_REPO)")]
    Repository,
    #[error("the preview key cannot be read, or is shorter than 32 bytes (the secret write-preview-key)")]
    PreviewKey,
    #[error("a job of the write path could not be registered")]
    Job,
    #[error("setting up the write path took too long")]
    TimedOut,
}

fn config_error(error: hub_writes::ConfigError) -> WritesBuildError {
    match error {
        hub_writes::ConfigError::Invalid(why) => WritesBuildError::Config(why),
        hub_writes::ConfigError::DenyGlob => WritesBuildError::Config("an extra deny glob is not valid"),
    }
}

/// What [`teams`] is made from.
pub struct TeamsInputs<'a> {
    pub settings: &'a WritesSettings,
    /// `https://host[:port]`, for the button of a card.
    pub public_origin: String,
    pub pool: PgPool,
    pub secrets: Arc<dyn SecretSource>,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for TeamsInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsInputs").finish_non_exhaustive()
    }
}

/// The Teams notifier: TLS 1.3 to an allowed host, HTTPS only, queued and sent by a worker of its own. It needs a running runtime.
pub fn teams(inputs: TeamsInputs<'_>) -> Result<TeamsNotifier, WritesBuildError> {
    let TeamsInputs {
        settings,
        public_origin,
        pool,
        secrets,
        clock,
    } = inputs;
    let mut config = TeamsConfig {
        web_base_url: Some(public_origin),
        ..TeamsConfig::default()
    };
    if let Some(hosts) = &settings.teams_hosts {
        config.allowed_hosts.clone_from(hosts);
    }
    TeamsNotifier::new(
        TeamsDeps {
            pool,
            secrets,
            clock: Arc::new(WritesClock(clock)),
        },
        config,
    )
    .map_err(|_| WritesBuildError::Teams)
}

/// What [`Writes::build`] is made from.
pub struct WritesInputs<'a> {
    pub settings: &'a WritesSettings,
    pub registry: &'a RegistrySettings,
    pub git: &'a GitSettings,
    pub edges: &'a WriteEdges,
    pub pool: PgPool,
    pub users: Arc<dyn Users>,
    pub audit: Arc<dyn AuditLog>,
    pub bus: Arc<dyn EventBus>,
    pub gateway: Arc<dyn AgentGateway>,
    pub blobs: Arc<dyn BlobStore>,
    pub registry_read: Arc<dyn RegistryRead>,
    pub tool_writes: Arc<dyn ToolWrites>,
    pub notifier: Arc<dyn Notifier>,
    pub leases: Arc<dyn Leases>,
    pub secrets: Arc<dyn SecretSource>,
    pub clock: Arc<dyn Clock>,
    /// Turns true when the hub stops: the write path's tasks end then.
    pub closing: watch::Receiver<bool>,
    /// The longest any one step of starting may take.
    pub start_timeout: Duration,
}

impl std::fmt::Debug for WritesInputs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritesInputs").finish_non_exhaustive()
    }
}

/// The write paths, built. The jobs are held until [`Writes::register_jobs`].
pub struct Writes {
    /// The service itself, for the callers inside the process (the MCP server's writes, T6).
    pub hub: WriteHub,
    state: Arc<WriteState>,
    jobs: Vec<JobEntry>,
}

impl std::fmt::Debug for Writes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writes")
            .field("jobs", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

/// Resolves when `closing` is true (or its sender is gone).
async fn closed(mut closing: watch::Receiver<bool>) {
    let _ = closing.wait_for(|stopping| *stopping).await;
}

impl Writes {
    /// Build the service, the vault, the pull-request jobs and the routes, and adopt the process's own tasks into `tasks`.
    // One construction for each part, in the order of the module's notes; splitting it would hide which depends on which.
    #[allow(clippy::too_many_lines)]
    pub async fn build(inputs: WritesInputs<'_>, tasks: &mut Tasks) -> Result<Self, WritesBuildError> {
        let WritesInputs {
            settings,
            registry,
            git,
            edges,
            pool,
            users,
            audit,
            bus,
            gateway,
            blobs,
            registry_read,
            tool_writes,
            notifier,
            leases,
            secrets,
            clock,
            closing,
            start_timeout,
        } = inputs;

        // The preview key (S7): a deployment without it would be a hub whose uploads and deletes all answer 503.
        let key = tokio::time::timeout(start_timeout, secrets.get(PREVIEW_KEY_SECRET))
            .await
            .map_err(|_| WritesBuildError::TimedOut)?
            .map_err(|_| WritesBuildError::PreviewKey)?;
        if key.expose().len() < MIN_PREVIEW_KEY_BYTES {
            return Err(WritesBuildError::PreviewKey);
        }
        drop(key);

        let config = WriteConfig {
            extra_deny_globs: registry.deny_globs(),
            config_server_deployment: settings.config_server_deployment.clone(),
            ..WriteConfig::default()
        };
        let hub = WriteHub::new(WriteHubDeps {
            pool,
            users,
            audit,
            bus: Arc::clone(&bus),
            gateway,
            blobs,
            registry: registry_read,
            tool_writes,
            notifier,
            secrets,
            leases,
            clock: Arc::new(WritesClock(clock)),
            config,
        })
        .map_err(config_error)?;

        // The vault and the pull-request jobs. The repositories are the two the hub mirrors: a token must be able to push to both.
        let repos = || -> Result<PolicyRepos, WritesBuildError> {
            Ok(PolicyRepos {
                base: RepoRef::parse(git.base_repo.as_str()).map_err(|_| WritesBuildError::Repository)?,
                tenant: RepoRef::parse(git.tenant_repo.as_str()).map_err(|_| WritesBuildError::Repository)?,
            })
        };
        let vault = hub.token_vault(VaultDeps {
            kms: Arc::clone(&edges.envelope),
            github: Arc::clone(&edges.github),
            repos: repos()?,
        });
        let pr_jobs = hub.pr_jobs(PrDeps {
            vault: vault.clone(),
            git: Arc::clone(&edges.git),
            settings: PrSettings {
                repos: repos()?,
                base_branch: settings.base_branch.clone(),
                max_total_bytes: PrSettings::DEFAULT_MAX_TOTAL_BYTES,
            },
        });
        let state = WriteState::new(
            Arc::new(hub.clone()),
            Arc::new(hub.clone()),
            Arc::new(vault),
            Arc::new(pr_jobs.clone()),
            Arc::new(hub.clone()),
            hub.max_content_bytes(),
        );
        let jobs = job_wrappers::jobs(
            &settings.jobs,
            hub.reconciler(),
            hub.expirer(),
            pr_jobs,
            hub.credential_cleanup(),
        )
        .map_err(|_| WritesBuildError::Job)?;

        // The process's own tasks. Both end with the hub, and one that ends early turns readiness off.
        let watcher = hub.pr_state_watcher();
        // Subscribed now, so that a change between here and the task's first poll is not missed.
        let events = watcher.subscribe();
        let stop = closing.clone();
        tasks.adopt(
            "writes-pr-state",
            tokio::spawn(async move {
                tokio::select! {
                    () = watcher.follow(events) => {}
                    () = closed(stop) => {}
                }
            }),
        );
        if let Some(auto) = hub.auto_notifier() {
            let stop = closing.clone();
            tasks.adopt(
                "writes-auto-notify",
                tokio::spawn(async move { auto.run(closed(stop)).await }),
            );
        }
        info!("the write paths are set up");
        Ok(Self { hub, state, jobs })
    }

    /// The names of the jobs, in registration order.
    #[must_use]
    pub fn job_names(&self) -> Vec<String> {
        self.jobs.iter().map(|(spec, _)| spec.name().to_owned()).collect()
    }

    /// Register the four jobs with `runner`. The runner starts later, when everything that registers a job has.
    pub fn register_jobs(&mut self, runner: &mut JobRunner) -> Result<(), WritesBuildError> {
        for (spec, job) in std::mem::take(&mut self.jobs) {
            runner.register(spec, job).map_err(|_| WritesBuildError::Job)?;
        }
        Ok(())
    }

    /// The 18 routes of the write paths, each behind the hub's guard.
    #[must_use]
    pub fn module(&self) -> Arc<dyn Module> {
        Arc::new(WritesModule {
            state: Arc::clone(&self.state),
        })
    }
}

/// Mounts the write routes.
pub struct WritesModule {
    state: Arc<WriteState>,
}

impl std::fmt::Debug for WritesModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WritesModule").finish_non_exhaustive()
    }
}

impl Module for WritesModule {
    fn name(&self) -> &'static str {
        "writes"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        hub_writes::routes::mount(router, Arc::clone(&self.state))
    }
}
