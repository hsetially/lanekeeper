//! The Kubernetes side of the agent (T6, S17): watching Deployments, Pods and Jobs, cluster reports, and restarts.
//!
//! - [`watch`]: one watcher per kind and namespace, kept as a [`projection::Projection`] and released as reports
//!   debounced to one second ([`debounce`]).
//! - [`trim`]: what is kept of each object (names, selectors, a few labels) and what is dropped at once.
//! - [`env`]: which environment values may leave the cluster (D88), and which never do.
//! - [`helm`], [`jobs`]: release hints from Helm labels (Q19) and the sync Jobs (D72, Q3).
//! - [`restart`]: the `restartedAt` merge patch, in the configured namespaces only.
//!
//! What the agent may ask of the API server is written down in `RBAC.md` and checked by a lint (acceptance 4): `get`,
//! `list` and `watch` on the three kinds, `patch` on Deployments, and `get` and `update` on exactly the named Secrets.
//! Nothing here lists or watches Secrets or `ConfigMaps`.

pub mod debounce;
pub mod env;
pub mod error;
pub mod helm;
pub mod jobs;
pub mod projection;
pub mod restart;
pub mod trim;
pub mod watch;

use std::fmt;
use std::sync::Arc;

use ::kube::Client;
use async_trait::async_trait;
use domain::{ClusterReport, JobRef, ServiceRef, ShortText};
use tokio::sync::mpsc;

pub use env::EnvGuard;
pub use error::{BuildError, ReportError, RestartError};
pub use restart::Restarter;
pub use watch::{ClusterWatcher, WatchConfig};

use crate::clock::Clock;
use crate::config::Settings;

/// What the hub can ask of the cluster side. The dispatcher answers `RequestClusterReport` and `RestartDeployment`
/// with it; a test can stand in for the whole cluster.
#[async_trait]
pub trait ClusterOps: Send + Sync + fmt::Debug + 'static {
    /// The whole picture, for a hub that asked for it (and for the first message after a connect).
    async fn full_report(&self) -> Result<ClusterReport, ReportError>;

    /// Restart a Deployment by patching its pod template.
    async fn restart(&self, service: &ServiceRef) -> Result<(), RestartError>;
}

/// The watchers and the restarter, built from the settings.
#[derive(Debug)]
pub struct Cluster {
    watcher: ClusterWatcher,
    restarter: Restarter,
}

impl Cluster {
    /// Start watching. The channel carries the delta reports. Must be called inside a tokio runtime.
    pub fn start(
        client: &Client,
        settings: &Settings,
        clock: Arc<dyn Clock>,
    ) -> Result<(Self, mpsc::Receiver<ClusterReport>), BuildError> {
        Ok(Self::start_with(
            client,
            WatchConfig::from_settings(settings)?,
            clock,
        ))
    }

    pub fn start_with(
        client: &Client,
        config: WatchConfig,
        clock: Arc<dyn Clock>,
    ) -> (Self, mpsc::Receiver<ClusterReport>) {
        let restarter = Restarter::new(
            client.clone(),
            config.namespaces.iter().map(crate::config::KubeName::as_str),
            clock,
        );
        let (watcher, reports) = ClusterWatcher::start(client, config);
        (Self { watcher, restarter }, reports)
    }

    /// `AgentConfig.env_allowlist`, when the hub sends it.
    pub fn set_env_allowlist(&self, names: &[ShortText]) {
        self.watcher.set_env_allowlist(names);
    }

    /// Both watchers have listed their kinds.
    pub fn is_ready(&self) -> bool {
        self.watcher.is_ready()
    }

    /// The sync Jobs that are running now.
    pub fn active_sync_jobs(&self) -> Vec<JobRef> {
        self.watcher.active_sync_jobs()
    }
}

#[async_trait]
impl ClusterOps for Cluster {
    async fn full_report(&self) -> Result<ClusterReport, ReportError> {
        self.watcher.full_report().await
    }

    async fn restart(&self, service: &ServiceRef) -> Result<(), RestartError> {
        self.restarter.restart(service).await
    }
}
