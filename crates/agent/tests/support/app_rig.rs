//! The whole agent (`agent::app::App`) against the fake hub, the fake Kubernetes API and a scripted file system, for the
//! tests that are about how the parts fit together (T7): readiness, metrics, shutdown, and what a connection sets going.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use agent::app::{App, AppError, Parts};
use agent::config::Settings;
use agent::identity::joiner::Joiner;
use agent::identity::store::MemoryCertStore;
use agent::ops::{Health, Metrics};
use agent::root::NfsRoot;
use agent::transport::session::SessionConfig;
use agent::tree::TreeSource;
use proto::convert::FromAgent;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::fake_hub::{ConnHandle, ScriptedIdTokens};
use super::fake_kube::FakeKube;
use super::rig::{Rig, WI_TOKEN};
use super::scripted_source::ScriptedSource;
use super::valid_env;

pub struct AppRig {
    pub rig: Rig,
    pub kube: Option<FakeKube>,
    pub health: Arc<Health>,
    pub metrics: Arc<Metrics>,
    pub dir: TempDir,
    pub source: Option<Arc<dyn TreeSource>>,
    /// The identity the agent connected with.
    pub identity: Arc<agent::identity::joiner::IdentityHandle>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), AppError>>>,
}

impl Drop for AppRig {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// What a test chooses about the agent it starts.
pub struct Setup {
    pub kube: Option<FakeKube>,
    /// Where the tree comes from. `None`: the production source, which walks `root` on the worker pool.
    pub source: Option<Arc<dyn TreeSource>>,
    /// The NFS root. Default: an empty temporary directory.
    pub root: Option<TempDir>,
    pub session: SessionConfig,
    pub extra_env: Vec<(&'static str, &'static str)>,
    /// Run the certificate renewal too (a joiner against the fake hub, with an in-memory certificate store).
    pub renewal: bool,
}

impl Default for Setup {
    fn default() -> Self {
        let source = ScriptedSource::new();
        source.write("svc/a.yml", b"a: 1\n");
        Self {
            kube: None,
            source: Some(Arc::new(source)),
            root: None,
            session: SessionConfig::default(),
            extra_env: Vec::new(),
            renewal: false,
        }
    }
}

impl AppRig {
    pub async fn start(setup: Setup) -> Self {
        Self::start_on(Rig::new(), setup).await
    }

    pub async fn start_on(rig: Rig, setup: Setup) -> Self {
        let dir = setup.root.unwrap_or_else(|| TempDir::new().unwrap());
        let mut env = valid_env(dir.path());
        for (name, value) in &setup.extra_env {
            env.insert((*name).to_owned(), (*value).to_owned());
        }
        let settings = Settings::from_env(&env).unwrap();
        let root = NfsRoot::open(dir.path()).unwrap();
        let identity = rig.identity_handle().await;
        let clock = rig.clock.clone();
        let health = Health::new(clock.clone());
        let metrics = Metrics::new();
        let mut parts = Parts::new(
            settings,
            root,
            rig.transport.clone(),
            identity.clone(),
            clock,
            health.clone(),
            metrics.clone(),
        )
        .with_session_config(setup.session);
        if let Some(source) = &setup.source {
            parts = parts.with_source(source.clone());
        }
        if let Some(kube) = &setup.kube {
            parts = parts.with_kube(kube.client());
        }
        if setup.renewal {
            let joiner = Joiner::new(
                &parts.settings,
                ScriptedIdTokens::new(Ok(WI_TOKEN)),
                None,
                rig.transport.clone(),
                Arc::new(MemoryCertStore::new()),
                rig.clock.clone(),
            );
            parts = parts.with_joiner(Arc::new(joiner));
        }
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            App::new(parts)
                .run(async move {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            rig,
            kube: setup.kube,
            health,
            metrics,
            dir,
            source: setup.source,
            identity,
            stop: Some(stop),
            task: Some(task),
        }
    }

    /// The first connection, once the agent has sent its first heartbeat on it.
    pub async fn connected(&self) -> ConnHandle {
        let conn = self.rig.server.wait_for_connection(1).await;
        conn.wait_for(|m| matches!(m, FromAgent::Heartbeat(_))).await;
        conn
    }

    pub fn is_ready(&self) -> bool {
        self.health.ready().is_ok()
    }

    /// Ask the agent to stop and return how it ended. Waits (in virtual time) for it.
    pub async fn stop(&mut self) -> Result<(), AppError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let task = self.task.take().expect("the agent was already stopped");
        tokio::time::timeout(Duration::from_secs(120), task)
            .await
            .expect("the agent stops within two virtual minutes")
            .expect("the agent task did not panic")
    }

    /// Wait for the agent to end by itself and return how.
    pub async fn ended(&mut self) -> Result<(), AppError> {
        let task = self.task.take().expect("the agent was already stopped");
        tokio::time::timeout(Duration::from_secs(600), task)
            .await
            .expect("the agent ends within ten virtual minutes")
            .expect("the agent task did not panic")
    }

    /// Poll (in virtual time) until `condition` holds.
    pub async fn wait_until(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        for _ in 0..6_000 {
            if condition(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting until {what}");
    }
}
