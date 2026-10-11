//! Multi-replica routing of commands to agents (D62).
//!
//! An agent holds one `Connect` stream, to one of the hub's replicas. A command for it can start on any replica:
//!
//! ```text
//!   AgentGateway::request(swimlane, command)
//!     -> read agent_connections (cached 1 s)     the owner replica, its address, the epoch
//!     -> owner is this replica?  yes: write to the local stream, wait for the reply with the same request id
//!                                no:  Forward RPC to the owner over mTLS, with the epoch that was read
//!     -> the owner refuses a stale epoch: re-read once, retry once, then NotConnected
//! ```
//!
//! Every wait has a timeout, the requests in flight per agent and the table of streams are bounded, and when a stream is
//! replaced or ends, every request on it completes with [`GatewayError::Closed`] rather than waiting for a timeout. A command
//! is never sent twice by the gateway.
//!
//! - [`registry`]: `agent_connections`, with the epoch rules.
//! - [`local`] and [`correlation`]: the streams this replica holds and the requests waiting on them.
//! - [`forward`] and [`replica_service`]: the two ends of the `Forward` RPC (`proto/replica.proto`, [`wire`]).
//! - [`metrics`]: counters with closed label sets.

pub mod correlation;
pub mod forward;
pub mod local;
pub mod metrics;
pub mod registry;
pub mod replica_service;
pub mod wire;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentReply, AgentStatus, HubCommand, SwimlaneId};
use moka::sync::Cache;
use ports::{AgentGateway, GatewayError};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::ca::profile::PeerName;
use crate::stream::ConnectedAgent;
use forward::{ForwardFailure, PeerAddress, ReplicaClient};
use local::{Epoch, LocalStream, LocalStreams};
use metrics::{ForwardOutcome, GatewayMetrics, RequestOutcome, RequestPath};
use registry::{ConnectionRegistry, Lookup, RegistryError};
use wire::ForwardCall;

/// Bounds and timings of the gateway.
#[derive(Debug, Clone, Copy)]
pub struct GatewaySettings {
    /// Agent streams this replica holds at most.
    pub local_capacity: usize,
    /// The longest a request waits, whatever the caller asked for.
    pub max_request_timeout: Duration,
    /// How long a read of `agent_connections` is reused for a swimlane.
    pub route_cache_ttl: Duration,
    pub route_cache_capacity: u64,
    /// How often the streams held here renew their claim in `agent_connections`. A claim not renewed for 30 seconds counts as
    /// dead, so this must stay well below that.
    pub refresh_interval: Duration,
    /// The pause before the one retry of a request that met a stale route.
    pub retry_backoff: Duration,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            local_capacity: 2048,
            max_request_timeout: Duration::from_secs(300),
            route_cache_ttl: Duration::from_secs(1),
            route_cache_capacity: 4096,
            refresh_interval: Duration::from_secs(10),
            retry_backoff: Duration::from_millis(25),
        }
    }
}

/// Why an agent stream could not be attached to the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    #[error("the connection registry could not record the stream")]
    Registry(#[from] RegistryError),
    #[error("this replica holds as many agent streams as it can")]
    TooManyStreams,
}

/// The gateway of one hub replica.
pub struct HubGateway {
    registry: ConnectionRegistry,
    local: Arc<LocalStreams>,
    client: ReplicaClient,
    routes: Cache<SwimlaneId, Lookup>,
    metrics: GatewayMetrics,
    settings: GatewaySettings,
}

impl std::fmt::Debug for HubGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubGateway")
            .field("replica", self.registry.replica())
            .finish_non_exhaustive()
    }
}

impl HubGateway {
    #[must_use]
    pub fn new(
        registry: ConnectionRegistry,
        client: ReplicaClient,
        metrics: GatewayMetrics,
        settings: GatewaySettings,
    ) -> Arc<Self> {
        Arc::new(Self {
            routes: Cache::builder()
                .max_capacity(settings.route_cache_capacity)
                .time_to_live(settings.route_cache_ttl)
                .build(),
            local: Arc::new(LocalStreams::new(settings.local_capacity)),
            registry,
            client,
            metrics,
            settings,
        })
    }

    /// The streams held here, for the replica service.
    #[must_use]
    pub fn local(&self) -> Arc<LocalStreams> {
        Arc::clone(&self.local)
    }

    #[must_use]
    pub fn metrics(&self) -> &GatewayMetrics {
        &self.metrics
    }

    #[must_use]
    pub fn replica(&self) -> &PeerName {
        self.registry.replica()
    }

    #[must_use]
    pub fn client(&self) -> &ReplicaClient {
        &self.client
    }

    /// Register an agent stream that has just been accepted: the registry gives it the next epoch, and any older stream of the
    /// same swimlane on this replica is closed (its requests fail with `Closed`).
    pub async fn attach(self: &Arc<Self>, agent: Arc<ConnectedAgent>) -> Result<Attachment, AttachError> {
        let swimlane = agent.swimlane().clone();
        let epoch = self.registry.register(&swimlane).await?;
        let stream = LocalStream::new(agent, epoch);
        let Ok(replaced) = self.local.insert(Arc::clone(&stream)) else {
            // Registered but not held: let go of the registration so that nobody forwards to a stream that is not here.
            if let Err(error) = self.registry.clear(&swimlane, epoch).await {
                debug!(code = "gateway_release_failed", %error, "releasing a registration failed");
            }
            return Err(AttachError::TooManyStreams);
        };
        if let Some(old) = replaced {
            old.close();
        }
        self.routes.invalidate(&swimlane);
        self.metrics.stream_attached();
        Ok(Attachment {
            gateway: Arc::clone(self),
            stream,
        })
    }

    /// Renew the claim of every stream held here, and close those the agent has left for another replica. Returns how many
    /// streams were found superseded.
    pub async fn refresh_once(&self) -> Result<usize, RegistryError> {
        let held = self.local.snapshot();
        let renewed = self.registry.refresh(&held).await?;
        let mut superseded = 0;
        for (swimlane, epoch) in held {
            if renewed.contains(&swimlane) {
                continue;
            }
            // Only the generation that was checked: a stream attached since then is newer and is not stale.
            if let Some(stream) = self.local.get(&swimlane).filter(|stream| stream.epoch() == epoch) {
                info!(
                    code = "stream_superseded",
                    "an agent connected elsewhere; closing the stream held here"
                );
                stream.close();
                self.metrics.superseded();
                superseded += 1;
            }
        }
        Ok(superseded)
    }

    /// Run [`HubGateway::refresh_once`] every `refresh_interval` until `stop` changes.
    pub fn spawn_liveness(self: &Arc<Self>, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        let gateway = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(gateway.settings.refresh_interval) => {}
                    _ = stop.changed() => return,
                }
                if let Err(error) = gateway.refresh_once().await {
                    // Streams stay open: a database that is briefly away does not disconnect agents. Peers stop routing here
                    // only if the claim goes unrenewed for 30 seconds.
                    warn!(code = "gateway_refresh_failed", %error, "renewing the agent connections failed");
                }
            }
        })
    }

    async fn lookup(&self, swimlane: &SwimlaneId, fresh: bool) -> Result<Lookup, GatewayError> {
        if !fresh && let Some(found) = self.routes.get(swimlane) {
            return Ok(found);
        }
        let found = self
            .registry
            .lookup(swimlane)
            .await
            .map_err(|_| GatewayError::Unavailable)?;
        self.routes.insert(swimlane.clone(), found.clone());
        Ok(found)
    }

    async fn pause_before_retry(&self, deadline: Instant) {
        let jitter = self.settings.retry_backoff.mul_f64(1.0 + fastrand::f64());
        let room = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(jitter.min(room / 2)).await;
    }

    async fn route(
        &self,
        swimlane: &SwimlaneId,
        command: HubCommand,
        timeout: Duration,
    ) -> (RequestPath, Result<AgentReply, GatewayError>) {
        let deadline = Instant::now() + timeout;
        let mut path = RequestPath::Unrouted;
        // Two looks: the first may use a read of the registry up to a second old, the second always reads it again.
        for attempt in 0..2 {
            let fresh = attempt > 0;
            let route = match self.lookup(swimlane, fresh).await {
                Ok(Lookup::Live(route)) => route,
                Ok(Lookup::Stale | Lookup::Absent) => {
                    if fresh {
                        return (path, Err(GatewayError::NotConnected));
                    }
                    continue;
                }
                Err(error) => return (path, Err(error)),
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return (path, Err(GatewayError::Timeout));
            }
            if route.replica == self.registry.replica().as_str() {
                path = RequestPath::Local;
                let stream = self
                    .local
                    .get(swimlane)
                    .filter(|stream| stream.epoch() == route.epoch);
                let Some(stream) = stream else {
                    // The registry says this replica holds the stream, but the stream is not here (or is another generation):
                    // it is being attached, or it just ended.
                    if !fresh {
                        self.pause_before_retry(deadline).await;
                    }
                    continue;
                };
                return (path, stream.request(command, remaining).await);
            }
            path = RequestPath::Forwarded;
            let Ok(address) = PeerAddress::parse(&route.address) else {
                warn!(
                    code = "replica_address_invalid",
                    "the registry holds an address that is not host:port"
                );
                return (path, Err(GatewayError::Unavailable));
            };
            let call = ForwardCall {
                swimlane: swimlane.clone(),
                epoch: route.epoch,
                timeout: remaining,
                command: command.clone(),
            };
            match self.client.forward(&address, call).await {
                Ok(reply) => {
                    self.metrics.forward(ForwardOutcome::Replied);
                    return (path, Ok(reply));
                }
                Err(ForwardFailure::StaleEpoch) => {
                    self.metrics.forward(ForwardOutcome::StaleEpoch);
                    if !fresh {
                        self.pause_before_retry(deadline).await;
                    }
                }
                Err(ForwardFailure::Gateway(error)) => {
                    self.metrics.forward(ForwardOutcome::Failed);
                    return (path, Err(error));
                }
            }
        }
        (path, Err(GatewayError::NotConnected))
    }
}

#[async_trait]
impl AgentGateway for HubGateway {
    async fn request(
        &self,
        s: &SwimlaneId,
        cmd: HubCommand,
        timeout: Duration,
    ) -> Result<AgentReply, GatewayError> {
        if timeout.is_zero() {
            return Err(GatewayError::Timeout);
        }
        let timeout = timeout.min(self.settings.max_request_timeout);
        let (path, result) = self.route(s, cmd, timeout).await;
        let outcome = match &result {
            Ok(_) => RequestOutcome::Replied,
            Err(error) => RequestOutcome::from(*error),
        };
        self.metrics.request(path, outcome);
        result
    }

    async fn status(&self, s: &SwimlaneId) -> AgentStatus {
        match self.lookup(s, false).await {
            Ok(Lookup::Live(_)) => AgentStatus::Connected,
            Ok(Lookup::Stale) => AgentStatus::Disconnected,
            Ok(Lookup::Absent) => match self.registry.has_joined(s).await {
                Ok(false) => AgentStatus::NeverSeen,
                Ok(true) | Err(_) => AgentStatus::Disconnected,
            },
            // The registry cannot be read: what this replica itself knows is all there is.
            Err(_) => {
                if self.local.get(s).is_some() {
                    AgentStatus::Connected
                } else {
                    AgentStatus::Disconnected
                }
            }
        }
    }
}

/// A stream attached to the gateway. The stream's handler completes requests through it and detaches it when the stream ends.
pub struct Attachment {
    gateway: Arc<HubGateway>,
    stream: Arc<LocalStream>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("epoch", &self.stream.epoch())
            .finish_non_exhaustive()
    }
}

impl Attachment {
    #[must_use]
    pub fn epoch(&self) -> Epoch {
        self.stream.epoch()
    }

    /// Give an agent's reply to the request waiting for it. False when nothing was waiting (the reply is dropped).
    pub fn complete(&self, reply: AgentReply) -> bool {
        self.stream.complete(reply)
    }

    /// The stream has ended: stop holding it, fail what is waiting on it with `Closed`, and release the registration if it is
    /// still this stream's. A registration that was replaced in the meantime is left alone.
    pub async fn detach(self) {
        let swimlane = self.stream.swimlane().clone();
        self.gateway.local.remove_if(&self.stream);
        self.stream.close();
        self.gateway.routes.invalidate(&swimlane);
        self.gateway.metrics.stream_detached();
        if let Err(error) = self.gateway.registry.clear(&swimlane, self.stream.epoch()).await {
            // The row goes stale on its own within 30 seconds, which is what a replica that died would have left.
            warn!(code = "gateway_release_failed", %error, "releasing an agent registration failed");
        }
    }
}
