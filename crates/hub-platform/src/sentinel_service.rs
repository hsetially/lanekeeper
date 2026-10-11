//! The `Sentinel` gRPC service (T11, D72, D74, S5, S6, S11): the daemon on the NFS VM joins, then reports spooled audit batches.
//!
//! **Join** is the agent's CA path with `kind = Sentinel` ([`JoinService::join_sentinel`]): the VM's Google ID token, whose email
//! must be the service account registered for that VM in `sentinels`, and nothing else. A join token is an agent's fallback and is
//! not taken here.
//!
//! **Report**, in order: a verified *sentinel* certificate (`spiffe://lanekeeper/sentinel/<name>`; an agent's or a replica's
//! certificate is a valid certificate for another service and is refused with `PermissionDenied`); `SentinelHello` first, within
//! 10 seconds, with `vm` equal to the name in the certificate; the sink's answer goes back as `settings`; then each message is
//! decoded through `proto::convert` (size, enum and path validation happen before a domain value exists) and a batch is handed to
//! [`SentinelSink::records`]. **Only after the sink returned** is `acked_seq` sent, and it is the `seq` the sink returned: a batch
//! it had already applied is answered with the highest `seq` so far, and that is what the sentinel hears (D74). A batch the sink
//! refuses or does not answer in time gets no ack, so the sentinel keeps it in its spool and resends it.
//!
//! The identity the sink is given is made from the certificate and nowhere else. Every wait is bounded and every queue is.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use domain::ShortText;
use ports::{SentinelIdentity, SentinelSink, SinkError};
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use proto::convert::{FromSentinel, JoinParams, ToSentinel};
use proto::pb;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::service::Routes;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, warn};

use crate::agent_metrics::StreamEnd;
use crate::join::JoinService;
use crate::listener::{PeerAddr, PeerTls};
use crate::tls::PeerIdentity;

/// Messages waiting to be written to one sentinel: its settings, and an ack for each batch in flight.
const OUTBOUND_QUEUE: usize = 16;

/// Bounds of one `Report` stream (rule 5).
#[derive(Debug, Clone, Copy)]
pub struct SentinelLimits {
    /// `SentinelHello` must be the first message and arrive within this long.
    pub hello_timeout: Duration,
    /// A stream that carries nothing for this long is closed. The sentinel sends a heartbeat every few seconds.
    pub idle_timeout: Duration,
    /// One call into the sink.
    pub sink_timeout: Duration,
    /// Writing one message to a sentinel that is not reading.
    pub send_timeout: Duration,
}

impl Default for SentinelLimits {
    fn default() -> Self {
        Self {
            hello_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            sink_timeout: Duration::from_secs(30),
            send_timeout: Duration::from_secs(30),
        }
    }
}

/// What came of one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EncodeLabelValue)]
pub enum BatchOutcome {
    /// Applied and acknowledged.
    Acked,
    /// The sink refused it or did not answer: no ack, the sentinel resends.
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct BatchLabels {
    outcome: BatchOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct EndLabels {
    reason: StreamEnd,
}

/// Metrics of the sentinel endpoint (S10). Every label comes from a closed set; a VM name is never a label. Cloning shares the
/// counters.
#[derive(Debug, Clone, Default)]
pub struct SentinelMetrics {
    streams_active: Gauge,
    batches: Family<BatchLabels, Counter>,
    stream_ends: Family<EndLabels, Counter>,
}

impl SentinelMetrics {
    pub fn register(&self, registry: &mut Registry) {
        registry.register(
            "sentinel_streams_active",
            "Sentinel Report streams open on this replica",
            self.streams_active.clone(),
        );
        registry.register(
            "sentinel_batches",
            "Audit record batches from sentinels by outcome",
            self.batches.clone(),
        );
        registry.register(
            "sentinel_stream_ends",
            "Sentinel streams that ended by reason",
            self.stream_ends.clone(),
        );
    }

    fn batch(&self, outcome: BatchOutcome) {
        self.batches.get_or_create(&BatchLabels { outcome }).inc();
    }

    #[must_use]
    pub fn streams_active(&self) -> i64 {
        self.streams_active.get()
    }

    #[must_use]
    pub fn batches_total(&self, outcome: BatchOutcome) -> u64 {
        self.batches.get_or_create(&BatchLabels { outcome }).get()
    }
}

#[derive(Clone)]
pub struct SentinelService {
    join: Arc<JoinService>,
    sink: Arc<dyn SentinelSink>,
    limits: SentinelLimits,
    /// `true` once the hub is shutting down: every stream ends and tells its sentinel to reconnect elsewhere.
    closing: watch::Receiver<bool>,
    metrics: SentinelMetrics,
}

impl std::fmt::Debug for SentinelService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SentinelService").finish_non_exhaustive()
    }
}

impl SentinelService {
    #[must_use]
    pub fn new(
        join: Arc<JoinService>,
        sink: Arc<dyn SentinelSink>,
        closing: watch::Receiver<bool>,
        metrics: SentinelMetrics,
        limits: SentinelLimits,
    ) -> Self {
        Self {
            join,
            sink,
            limits,
            closing,
            metrics,
        }
    }

    /// The service as a router, with zstd and the message size limit (`proto::grpc`).
    #[must_use]
    pub fn routes(self) -> Routes {
        Routes::new(proto::grpc::sentinel_server(self))
    }

    /// The service added to `routes`, next to the services already there (the hub serves `Agent` and `Sentinel` on one listener).
    #[must_use]
    pub fn add_to(self, routes: Routes) -> Routes {
        routes.add_service(proto::grpc::sentinel_server(self))
    }
}

/// The VM name the verified certificate names, or the status that refuses the call.
fn certificate_vm<T>(request: &Request<T>) -> Result<String, Status> {
    match request.extensions().get::<PeerTls>() {
        Some(PeerTls(Some(cert))) => match &cert.identity {
            PeerIdentity::Sentinel(name) => Ok(name.as_str().to_owned()),
            // An agent or a replica certificate is a valid certificate for another service (S5).
            PeerIdentity::Agent(_) | PeerIdentity::Hub(_) => Err(Status::permission_denied("")),
        },
        _ => Err(Status::unauthenticated("")),
    }
}

fn sink_status(error: SinkError) -> Status {
    match error {
        SinkError::HelloRequired => Status::failed_precondition(""),
        SinkError::IdentityMismatch => Status::permission_denied(""),
        SinkError::Invalid | SinkError::TooLarge => Status::invalid_argument(""),
        SinkError::Unavailable => Status::unavailable(""),
    }
}

async fn within<T>(
    limit: Duration,
    call: impl Future<Output = Result<T, SinkError>>,
) -> Result<T, SinkError> {
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or(Err(SinkError::Unavailable))
}

#[async_trait::async_trait]
impl pb::sentinel_server::Sentinel for SentinelService {
    async fn join(&self, request: Request<pb::JoinRequest>) -> Result<Response<pb::JoinResponse>, Status> {
        let remote = request.extensions().get::<PeerAddr>().map(|addr| addr.0.ip());
        // A request that does not even parse is refused like any other: the same status, no reason.
        let params = JoinParams::try_from(request.into_inner()).map_err(|_| Status::unauthenticated(""))?;
        match self.join.join_sentinel(remote, params).await {
            Ok(issued) => Ok(Response::new(issued.into())),
            Err(error) => Err(Status::new(error.status_code(), "")),
        }
    }

    type ReportStream = ReceiverStream<Result<pb::SentinelAck, Status>>;

    async fn report(
        &self,
        request: Request<Streaming<pb::SentinelMessage>>,
    ) -> Result<Response<Self::ReportStream>, Status> {
        let vm = certificate_vm(&request)?;
        let mut inbound = request.into_inner();

        // The sentinel speaks first, and what it says first is `SentinelHello`, for the VM its certificate names.
        let first = tokio::time::timeout(self.limits.hello_timeout, inbound.message())
            .await
            .map_err(|_| Status::deadline_exceeded(""))?
            .map_err(|_| Status::invalid_argument(""))?
            .ok_or_else(|| Status::cancelled(""))?;
        let Ok(Some(FromSentinel::Hello(hello))) = FromSentinel::from_proto(first) else {
            debug!(
                code = "report_hello_not_first",
                "the first message was not a valid SentinelHello"
            );
            return Err(Status::failed_precondition(""));
        };
        // Names are compared exactly: `NFS-VM-1` is another VM than `nfs-vm-1`.
        if hello.vm.as_str() != vm {
            debug!(
                code = "report_hello_vm_mismatch",
                "SentinelHello names another VM than the certificate"
            );
            return Err(Status::permission_denied(""));
        }
        let name = ShortText::parse(&vm).map_err(|_| Status::permission_denied(""))?;
        let identity = SentinelIdentity::new(name);
        let settings = within(self.limits.sink_timeout, self.sink.hello(&identity, hello))
            .await
            .map_err(sink_status)?;

        let (outbound, queue) = mpsc::channel(OUTBOUND_QUEUE);
        outbound
            .try_send(Ok(ToSentinel::Settings(settings).into_proto()))
            .map_err(|_| Status::unavailable(""))?;
        self.metrics.streams_active.inc();
        let session = Session {
            sink: Arc::clone(&self.sink),
            limits: self.limits,
            closing: self.closing.clone(),
            metrics: self.metrics.clone(),
            identity,
            outbound,
        };
        tokio::spawn(session.run(inbound));
        Ok(Response::new(ReceiverStream::new(queue)))
    }
}

struct Session {
    sink: Arc<dyn SentinelSink>,
    limits: SentinelLimits,
    closing: watch::Receiver<bool>,
    metrics: SentinelMetrics,
    identity: SentinelIdentity,
    outbound: mpsc::Sender<Result<pb::SentinelAck, Status>>,
}

/// What to do after handling one message.
enum Flow {
    Continue,
    End(StreamEnd, Option<Status>),
}

impl Session {
    async fn run(mut self, mut inbound: Streaming<pb::SentinelMessage>) {
        let (reason, status) = loop {
            let received = tokio::select! {
                biased;
                _ = self.closing.changed() => break (StreamEnd::HubClosing, Some(Status::unavailable(""))),
                () = self.outbound.closed() => break (StreamEnd::Closed, None),
                received = tokio::time::timeout(self.limits.idle_timeout, inbound.message()) => received,
            };
            let message = match received {
                Err(_elapsed) => break (StreamEnd::Idle, Some(Status::deadline_exceeded(""))),
                // A message the transport refused (too large, bad framing) or a reset.
                Ok(Err(_status)) => break (StreamEnd::Protocol, Some(Status::invalid_argument(""))),
                Ok(Ok(None)) => break (StreamEnd::AgentClosed, None),
                Ok(Ok(Some(message))) => message,
            };
            let parsed = match FromSentinel::from_proto(message) {
                Ok(Some(parsed)) => parsed,
                // A message kind this build does not know: skip it and keep the stream (the contract's rule for new variants).
                Ok(None) => continue,
                Err(error) => {
                    debug!(code = "report_bad_message", %error, "a sentinel message failed validation");
                    break (StreamEnd::Protocol, Some(Status::invalid_argument("")));
                }
            };
            if let Flow::End(reason, status) = self.handle(parsed).await {
                break (reason, status);
            }
        };
        if let Some(status) = status {
            // If there is no room for the final status, the stream's end says the same.
            let _ = self.outbound.try_send(Err(status));
        }
        self.metrics.streams_active.dec();
        self.metrics
            .stream_ends
            .get_or_create(&EndLabels { reason })
            .inc();
    }

    async fn handle(&mut self, message: FromSentinel) -> Flow {
        match message {
            // A second hello breaks the protocol.
            FromSentinel::Hello(_) => Flow::End(StreamEnd::Protocol, Some(Status::failed_precondition(""))),
            // Liveness only: the idle timeout is reset by the message arriving, and nothing is answered.
            FromSentinel::Heartbeat { .. } => Flow::Continue,
            FromSentinel::Batch(batch) => {
                // The ack is built from what the sink returned, after it returned, and from nothing else (D74).
                match within(self.limits.sink_timeout, self.sink.records(&self.identity, batch)).await {
                    Ok(acked) => {
                        let ack = ToSentinel::Ack(acked.get()).into_proto();
                        match tokio::time::timeout(self.limits.send_timeout, self.outbound.send(Ok(ack)))
                            .await
                        {
                            Ok(Ok(())) => {
                                self.metrics.batch(BatchOutcome::Acked);
                                Flow::Continue
                            }
                            Ok(Err(_closed)) => Flow::End(StreamEnd::AgentClosed, None),
                            Err(_elapsed) => Flow::End(StreamEnd::Stalled, Some(Status::unavailable(""))),
                        }
                    }
                    Err(error) => {
                        self.metrics.batch(BatchOutcome::Refused);
                        warn!(code = ?error, "the sink refused a sentinel batch");
                        match error {
                            SinkError::HelloRequired | SinkError::IdentityMismatch => {
                                Flow::End(StreamEnd::Protocol, Some(sink_status(error)))
                            }
                            // The sentinel keeps the batch in its spool and sends it again.
                            SinkError::Invalid | SinkError::TooLarge | SinkError::Unavailable => {
                                Flow::Continue
                            }
                        }
                    }
                }
            }
        }
    }
}
