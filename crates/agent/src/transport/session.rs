//! The agent's session with the hub (S6): connect, say `Hello`, take the hub's configuration, run, and when the stream
//! breaks, wait and connect again.
//!
//! [`Session::run`] owns the whole life cycle and never returns. Per connection it:
//!
//! 1. opens the stream through the [`HubTransport`] with the identity that is current *now*, so a certificate that was
//!    renewed since the last attempt is used from this handshake on;
//! 2. sends `Hello`, and waits up to 30 s for the hub's `AgentConfig`;
//! 3. gives the rest of the agent a [`Link`] (the configuration, an [`Outbox`] to send into and a queue of what the hub
//!    sends) and runs the [`LinkHandler`] over it, while moving messages in both directions;
//! 4. ends the connection when the stream fails, when the handler returns, or (once nothing is queued) when a renewed
//!    certificate is ready (A22).
//!
//! Between connections it waits with full-jitter exponential backoff capped at 60 s. The backoff is reset only by a
//! connection that stayed up for [`SessionConfig::stable_after`], so a hub that accepts and drops in a loop is not
//! hammered.
//!
//! Every message from the hub is validated by [`wire::decode`] before anything else sees it; one that fails is dropped
//! and counted, and a command that failed with a readable request id is answered with a refusal, and the stream
//! stays open (S11). The `CertRenewal` answer is routed to [`RenewalChannel::renew`] and never reaches the handler.

use std::convert::Infallible;
use std::fmt;
use std::future::pending;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{Hello, ShortText};
use futures::StreamExt;
use proto::convert::{FromAgent, IssuedCert, ToAgent};
use proto::pb;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep, timeout};
use tracing::{debug, info, warn};

use super::error::TransportError;
use super::outbox::{self, Outbox, OutboxLimits, OutboxReceiver};
use super::wire::{self, Decoded};
use super::{Connection, HubTransport, InboundStream};
use crate::backoff::Backoff;
use crate::config::{Settings, Tunables};
use crate::identity::RenewError;
use crate::identity::joiner::{IdentityHandle, RenewalChannel};

/// The hub must answer `Hello` with its configuration within this time.
pub const CONFIG_TIMEOUT: Duration = Duration::from_secs(30);
/// A renewal that the hub does not answer within this time has failed.
pub const RENEWAL_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the session looks for a quiet moment to reconnect for a renewed certificate.
const QUIET_POLL: Duration = Duration::from_millis(100);

/// Tuning that tests change and production leaves alone.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub backoff_base: Duration,
    /// S6: never longer than a minute.
    pub backoff_cap: Duration,
    /// The longest a whole connection attempt may take, whatever the transport does.
    pub connect_timeout: Duration,
    pub config_timeout: Duration,
    /// A connection that lasted this long counts as having worked, and resets the backoff.
    pub stable_after: Duration,
    /// The longest to wait for an empty outbox before reconnecting with a renewed certificate.
    pub quiet_wait: Duration,
    pub outbox: OutboxLimits,
    /// What the hub may have waiting for the handler.
    pub inbound_capacity: usize,
    /// Fix the retry jitter, so a test sees the same delays every time.
    pub seed: Option<u64>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(45),
            config_timeout: CONFIG_TIMEOUT,
            stable_after: Duration::from_secs(5),
            quiet_wait: Duration::from_secs(30),
            outbox: OutboxLimits::default(),
            inbound_capacity: 16,
            seed: None,
        }
    }
}

/// Where the session is, for `/readyz` and for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    /// Opening the stream, or waiting for the hub's configuration.
    Connecting,
    /// The hub's configuration has arrived and the handler is running.
    Connected,
}

/// What the rest of the agent gets for the length of one connection.
#[derive(Debug)]
pub struct Link {
    /// The hub's configuration, clamped. A later `AgentConfig` arrives as a [`ToAgent::Config`] in `inbound`.
    pub tunables: Tunables,
    /// Everything for the hub goes here. It belongs to this connection: when the connection ends it is closed, and a
    /// message queued on it is never sent on a later connection.
    pub outbox: Outbox,
    /// What the hub sent: configuration updates, acknowledgements and commands. Never a `CertRenewal` answer. Keep
    /// this receiver for as long as the handler runs, even if it is not read: when it is dropped the link is over.
    pub inbound: mpsc::Receiver<ToAgent>,
}

/// The agent's side of one connection. Everything the agent does while connected hangs off this.
///
/// The returned future is dropped when the connection ends, so it must be safe to cancel at any `await`. It may return
/// by itself to end the connection (the session then reconnects).
#[async_trait]
pub trait LinkHandler: Send + Sync {
    async fn handle(&self, link: Link);
}

/// Counters the session keeps, for metrics (T7) and for tests.
#[derive(Debug, Default)]
pub struct SessionStats {
    connections: AtomicU64,
    failed_attempts: AtomicU64,
    invalid_messages: AtomicU64,
    unknown_messages: AtomicU64,
    early_messages: AtomicU64,
    unanswered_refusals: AtomicU64,
    unsolicited_renewals: AtomicU64,
}

impl SessionStats {
    /// Connections that got as far as the hub's configuration.
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }

    /// Attempts that ended before the hub's configuration arrived.
    pub fn failed_attempts(&self) -> u64 {
        self.failed_attempts.load(Ordering::SeqCst)
    }

    /// Messages from the hub that failed validation and were dropped.
    pub fn invalid_messages(&self) -> u64 {
        self.invalid_messages.load(Ordering::SeqCst)
    }

    /// Empty messages, or ones from a newer hub that this build does not know.
    pub fn unknown_messages(&self) -> u64 {
        self.unknown_messages.load(Ordering::SeqCst)
    }

    /// Messages that arrived before the configuration, and were ignored.
    pub fn early_messages(&self) -> u64 {
        self.early_messages.load(Ordering::SeqCst)
    }

    /// Invalid commands that could not be answered because the outbox was full.
    pub fn unanswered_refusals(&self) -> u64 {
        self.unanswered_refusals.load(Ordering::SeqCst)
    }

    /// Renewal answers that nobody was waiting for.
    pub fn unsolicited_renewals(&self) -> u64 {
        self.unsolicited_renewals.load(Ordering::SeqCst)
    }

    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// The `Hello` for these settings.
pub fn hello(settings: &Settings) -> Result<Hello, domain::TextError> {
    Ok(Hello {
        agent_version: ShortText::parse(env!("CARGO_PKG_VERSION"))?,
        swimlane: settings.swimlane.clone(),
        cluster: settings.cluster.clone(),
        project: settings.project.clone(),
        nfs_server: settings.nfs.server.clone(),
        export: settings.nfs.export.clone(),
        mount_root: ShortText::parse(&settings.nfs.root.to_string_lossy())?,
    })
}

/// The one renewal that may be waiting for the hub's answer, and the outbox it was sent into.
#[derive(Default)]
struct RenewalSlot {
    outbox: Option<Outbox>,
    waiting: Option<(u64, oneshot::Sender<IssuedCert>)>,
    next_id: u64,
}

pub struct Session {
    hello: Hello,
    transport: Arc<dyn HubTransport>,
    identity: Arc<IdentityHandle>,
    config: SessionConfig,
    stats: SessionStats,
    state: watch::Sender<ConnectionState>,
    renewal: Mutex<RenewalSlot>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("swimlane", &self.hello.swimlane.as_str())
            .field("state", &*self.state.borrow())
            .finish_non_exhaustive()
    }
}

/// How one connection attempt went.
struct Attempt {
    /// The hub's configuration arrived, so the connection counts as having been made.
    established: bool,
    result: Result<(), TransportError>,
}

impl Session {
    pub fn new(
        hello: Hello,
        transport: Arc<dyn HubTransport>,
        identity: Arc<IdentityHandle>,
        config: SessionConfig,
    ) -> Self {
        Self {
            hello,
            transport,
            identity,
            config,
            stats: SessionStats::default(),
            state: watch::channel(ConnectionState::Disconnected).0,
            renewal: Mutex::new(RenewalSlot::default()),
        }
    }

    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// Follow the connection state.
    pub fn state(&self) -> watch::Receiver<ConnectionState> {
        self.state.subscribe()
    }

    /// Connect, run `handler`, and connect again, for as long as this future is polled.
    pub async fn run(&self, handler: Arc<dyn LinkHandler>) -> Infallible {
        let mut backoff = self.config.seed.map_or_else(
            || Backoff::new(self.config.backoff_base, self.config.backoff_cap),
            |seed| Backoff::with_seed(self.config.backoff_base, self.config.backoff_cap, seed),
        );
        loop {
            self.state.send_replace(ConnectionState::Connecting);
            let started = Instant::now();
            let attempt = self.attempt(&handler).await;
            self.detach_renewals();
            self.state.send_replace(ConnectionState::Disconnected);

            if attempt.established {
                match &attempt.result {
                    Ok(()) | Err(TransportError::Closed) => info!("the stream to the hub ended"),
                    Err(error) => warn!(%error, "the stream to the hub failed"),
                }
                if started.elapsed() >= self.config.stable_after {
                    backoff.reset();
                }
            } else {
                SessionStats::bump(&self.stats.failed_attempts);
                match &attempt.result {
                    Ok(()) => warn!("the hub closed the stream before sending its configuration"),
                    Err(error) => warn!(%error, "cannot connect to the hub"),
                }
            }
            sleep(backoff.next_delay()).await;
        }
    }

    async fn attempt(&self, handler: &Arc<dyn LinkHandler>) -> Attempt {
        let mut established = false;
        let result = self.connection(handler, &mut established).await;
        Attempt { established, result }
    }

    async fn connection(
        &self,
        handler: &Arc<dyn LinkHandler>,
        established: &mut bool,
    ) -> Result<(), TransportError> {
        // Subscribe, then read: the identity used for this connection is exactly the one the receiver has seen, so
        // `changed()` fires only for a certificate renewed after this handshake.
        let mut newer_identity = self.identity.subscribe();
        let identity = Arc::clone(&newer_identity.borrow_and_update());

        let first = wire::encode(FromAgent::Hello(self.hello.clone()));
        let Connection {
            outbound,
            mut inbound,
        } = timeout(
            self.config.connect_timeout,
            self.transport.connect(&identity, first),
        )
        .await
        .map_err(|_| TransportError::Timeout)??;

        let tunables = timeout(self.config.config_timeout, self.await_config(&mut inbound))
            .await
            .map_err(|_| TransportError::Timeout)??;

        let (outbox, outbox_rx) = outbox::channel(self.config.outbox);
        let (commands, link_inbound) = mpsc::channel(self.config.inbound_capacity.max(1));
        self.attach_renewals(outbox.clone());
        *established = true;
        SessionStats::bump(&self.stats.connections);
        self.state.send_replace(ConnectionState::Connected);
        info!(
            scan_interval_s = tunables.scan_interval.as_secs(),
            heartbeat_interval_s = tunables.heartbeat_interval.as_secs(),
            "connected to the hub"
        );

        let link = Link {
            tunables,
            outbox: outbox.clone(),
            inbound: link_inbound,
        };
        tokio::select! {
            result = write_loop(outbox_rx, outbound) => result,
            result = self.read_loop(inbound, commands, &outbox) => result,
            () = handler.handle(link) => Ok(()),
            () = self.wait_for_newer_identity(&mut newer_identity, &outbox) => Ok(()),
        }
    }

    /// Wait for the hub's `AgentConfig`. Anything else that arrives first is ignored: nothing has been set up to act on it.
    async fn await_config(&self, inbound: &mut InboundStream) -> Result<Tunables, TransportError> {
        loop {
            let Some(item) = inbound.next().await else {
                return Err(TransportError::Closed);
            };
            match wire::decode(item?) {
                Decoded::Message(ToAgent::Config(config)) => return Ok(Tunables::from_hub(&config)),
                Decoded::Message(_) => {
                    SessionStats::bump(&self.stats.early_messages);
                    debug!("the hub sent a message before its configuration; ignored");
                }
                Decoded::Unknown => SessionStats::bump(&self.stats.unknown_messages),
                Decoded::Invalid { problem, .. } => {
                    SessionStats::bump(&self.stats.invalid_messages);
                    warn!(%problem, "the hub sent an invalid message; dropped");
                }
            }
        }
    }

    /// Move what the hub sends to where it belongs. Ends when the stream does.
    async fn read_loop(
        &self,
        mut inbound: InboundStream,
        commands: mpsc::Sender<ToAgent>,
        outbox: &Outbox,
    ) -> Result<(), TransportError> {
        while let Some(item) = inbound.next().await {
            match wire::decode(item?) {
                Decoded::Message(ToAgent::CertRenewal(issued)) => self.deliver_renewal(issued),
                Decoded::Message(message) => {
                    // A full queue slows reading, and so the hub, through HTTP/2 flow control: nothing is dropped.
                    commands.send(message).await.map_err(|_| TransportError::Closed)?;
                }
                Decoded::Unknown => {
                    SessionStats::bump(&self.stats.unknown_messages);
                    debug!("the hub sent a message this build does not know; skipped");
                }
                Decoded::Invalid { problem, request_id } => {
                    SessionStats::bump(&self.stats.invalid_messages);
                    warn!(%problem, answered = request_id.is_some(), "the hub sent an invalid message; dropped");
                    if let Some(request_id) = request_id {
                        // Never wait here: reading must go on. A refusal that does not fit is counted, and the hub
                        // times the request out.
                        if outbox.try_send(wire::refusal(request_id)).is_err() {
                            SessionStats::bump(&self.stats.unanswered_refusals);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves when a certificate renewed after this connection's handshake is ready and nothing is queued. Pending
    /// forever if there never will be one.
    async fn wait_for_newer_identity(
        &self,
        newer: &mut watch::Receiver<Arc<crate::identity::ClientIdentity>>,
        outbox: &Outbox,
    ) {
        if newer.changed().await.is_err() {
            pending::<()>().await;
        }
        // The new certificate only applies to a new handshake (A22). Pick a moment when nothing is on its way.
        let give_up = Instant::now() + self.config.quiet_wait;
        while !outbox.is_idle() && Instant::now() < give_up {
            sleep(QUIET_POLL).await;
        }
        info!("a renewed certificate is ready; reconnecting to use it");
    }

    // ------------------------------------------------------------ renewal over the stream

    fn slot(&self) -> std::sync::MutexGuard<'_, RenewalSlot> {
        // Nothing in here can panic while the lock is held, so poisoning is not a case; take the data either way.
        self.renewal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn attach_renewals(&self, outbox: Outbox) {
        self.slot().outbox = Some(outbox);
    }

    /// The connection is over: a renewal that is waiting can never be answered.
    fn detach_renewals(&self) {
        let mut slot = self.slot();
        slot.outbox = None;
        slot.waiting = None;
    }

    fn deliver_renewal(&self, issued: IssuedCert) {
        let waiting = self.slot().waiting.take();
        if let Some((_, answer)) = waiting {
            let _ = answer.send(issued);
        } else {
            SessionStats::bump(&self.stats.unsolicited_renewals);
            debug!("the hub sent a certificate nobody asked for; ignored");
        }
    }
}

/// Sends what the outbox holds to the transport, in order, until either side is gone.
async fn write_loop(
    mut outbox: OutboxReceiver,
    wire: mpsc::Sender<pb::AgentMessage>,
) -> Result<(), TransportError> {
    while let Some(queued) = outbox.recv().await {
        let (message, room) = queued.into_parts();
        wire.send(message).await.map_err(|_| TransportError::Closed)?;
        // The room is given back only now, so the outbox counts what is still on its way as well as what waits.
        drop(room);
    }
    Ok(())
}

/// Forgets a waiting renewal if the caller stops waiting (a timeout, or the future is dropped).
struct StopWaiting<'a> {
    session: &'a Session,
    id: u64,
}

impl Drop for StopWaiting<'_> {
    fn drop(&mut self) {
        let mut slot = self.session.slot();
        if slot.waiting.as_ref().is_some_and(|(id, _)| *id == self.id) {
            slot.waiting = None;
        }
    }
}

#[async_trait]
impl RenewalChannel for Session {
    /// Send `csr_der` to the hub on the open stream and wait for the certificate it answers with.
    async fn renew(&self, csr_der: Bytes) -> Result<IssuedCert, RenewError> {
        let (answer, answered) = oneshot::channel();
        let (outbox, id) = {
            let mut slot = self.slot();
            let outbox = slot.outbox.clone().ok_or(RenewError::NotConnected)?;
            slot.next_id += 1;
            let id = slot.next_id;
            // A renewal still waiting is replaced: the newest request is the one that matters.
            slot.waiting = Some((id, answer));
            (outbox, id)
        };
        let _stop = StopWaiting { session: self, id };
        outbox
            .send(FromAgent::CertRenewal { csr_der })
            .await
            .map_err(|_| RenewError::NotConnected)?;
        match timeout(RENEWAL_TIMEOUT, answered).await {
            Ok(Ok(issued)) => Ok(issued),
            // The connection ended, or a newer request took this one's place.
            Ok(Err(_)) => Err(RenewError::NotConnected),
            Err(_) => Err(RenewError::Timeout),
        }
    }
}
