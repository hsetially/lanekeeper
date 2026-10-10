//! The agent as a process (T7): from validated settings to a running [`App`], and back to an exit code.
//!
//! The order matters, and is what `/readyz` reports:
//!
//! 1. The **health port** opens first, so that the kubelet sees a live process that is not ready, instead of a refused
//!    connection, while the rest starts.
//! 2. The **Kubernetes client** comes from the pod's service account. Without one the agent cannot read its certificate
//!    Secret, so it stops with an error and the kubelet restarts it.
//! 3. The **certificate**: the stored one if it is still good, otherwise a join through Workload Identity (or the join
//!    token, if the metadata server is unreachable and a token Secret is configured). The agent keeps trying, with
//!    backoff, for as long as the hub or the metadata server does not answer.
//! 4. The [`App`] runs until the shutdown future completes.
//!
//! A shutdown signal at any of these points ends the process in an orderly way.

use std::future::Future;
use std::io;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::info;

use crate::Started;
use crate::app::{App, AppError, Parts};
use crate::clock::SystemClock;
use crate::identity::idtoken::MetadataIdTokens;
use crate::identity::joiner::{IdentityHandle, Joiner};
use crate::identity::jointoken::{JoinTokenSource, KubeSecretJoinToken};
use crate::identity::store::KubeCertStore;
use crate::ops::health;
use crate::ops::{Health, Metrics};
use crate::transport::tls::HubRoots;
use crate::transport::{GrpcTransport, TlsError, TransportError};

/// Building the Kubernetes client reads files and environment variables of the pod; it should not take long.
const KUBE_CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why the process stops with an error. Messages name what failed, never a value from the environment (S10).
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("cannot listen on the health address: {0:?}")]
    HealthPort(io::ErrorKind),
    #[error("cannot build the Kubernetes client from the pod's service account")]
    Kube,
    #[error("the hub CA file cannot be used: {0}")]
    HubCa(#[from] TlsError),
    #[error("the hub endpoint cannot be used: {0}")]
    Hub(#[from] TransportError),
    #[error(transparent)]
    App(#[from] AppError),
}

/// Run the agent until `shutdown` completes. Must be called inside a tokio runtime.
pub async fn run(
    started: Started,
    metrics: Arc<Metrics>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), ProcessError> {
    let clock = Arc::new(SystemClock);
    let health = Health::new(clock);
    let listener = TcpListener::bind(started.settings.health_addr)
        .await
        .map_err(|e| ProcessError::HealthPort(e.kind()))?;
    info!(
        swimlane = started.settings.swimlane.as_str(),
        version = env!("CARGO_PKG_VERSION"),
        "the agent is starting"
    );
    let server: JoinHandle<_> =
        tokio::spawn(health::serve(listener, Arc::clone(&health), Arc::clone(&metrics)));
    let outcome = assemble(started, health, metrics, shutdown).await;
    server.abort();
    outcome
}

async fn assemble(
    started: Started,
    health: Arc<Health>,
    metrics: Arc<Metrics>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), ProcessError> {
    let Started { settings, root } = started;
    let mut shutdown = pin!(shutdown);

    let client = tokio::time::timeout(KUBE_CLIENT_TIMEOUT, ::kube::Client::try_default())
        .await
        .map_err(|_| ProcessError::Kube)?
        .map_err(|_| ProcessError::Kube)?;

    // The CA file is read with blocking I/O, once.
    let ca_path = settings.hub_ca_file.clone();
    let roots = tokio::task::spawn_blocking(move || HubRoots::load(&ca_path))
        .await
        .map_err(|_| TlsError::CaUnreadable)??;
    let transport = Arc::new(GrpcTransport::tcp(&settings.hub_endpoint, roots)?);

    let clock = Arc::new(SystemClock);
    let id_tokens = Arc::new(MetadataIdTokens::new(settings.metadata_url.clone()));
    let join_tokens = settings
        .join_token_secret
        .as_ref()
        .map(|name| Arc::new(KubeSecretJoinToken::new(client.clone(), name)) as Arc<dyn JoinTokenSource>);
    let store = Arc::new(KubeCertStore::new(client.clone(), &settings.cert_secret));
    let joiner = Arc::new(Joiner::new(
        &settings,
        id_tokens,
        join_tokens,
        transport.clone(),
        store,
        clock.clone(),
    ));

    // Not ready while this lasts: `/readyz` says "starting".
    let identity = tokio::select! {
        identity = joiner.establish() => identity,
        () = &mut shutdown => {
            info!("shutdown requested before the agent had a certificate");
            return Ok(());
        }
    };
    let identity = Arc::new(IdentityHandle::new(identity));

    let parts = Parts::new(settings, root, transport, identity, clock, health, metrics)
        .with_joiner(joiner)
        .with_kube(client);
    App::new(parts).run(shutdown).await?;
    Ok(())
}
