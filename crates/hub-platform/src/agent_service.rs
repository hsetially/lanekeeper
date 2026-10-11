//! The `Agent` gRPC service (S5, S6, S11): `Join` and the long-lived `Connect` stream.
//!
//! **Identity is the certificate.** The listener puts the verified peer certificate into the request ([`PeerTls`]); this
//! service reads the swimlane from it and nowhere else. `Hello` must name the same swimlane, and the sink is only ever handed
//! an [`AgentIdentity`] made from the certificate.
//!
//! **Connect**, in order: a verified agent certificate; `Hello` first, within 10 seconds, for the certificate's swimlane;
//! the sink's answer (`AgentConfig`) goes back; then each message is decoded through `proto::convert` (size, enum and path
//! validation happen before a domain value exists) and handed to the sink or answered. The stream ends when the agent
//! closes it, when nothing arrives for too long, when a message breaks the protocol, when the agent stops reading, or when
//! the hub is shutting down. Every wait is bounded.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use domain::{HubCommand, SwimlaneId};
use ports::{AgentIdentity, ReportSink, SinkError};
use proto::convert::{FromAgent, JoinParams, ToAgent};
use proto::pb;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;
use tonic::service::Routes;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, warn};

use crate::agent_metrics::{AgentMetrics, MessageKind, StreamEnd};
use crate::gateway::{Attachment, HubGateway};
use crate::join::JoinService;
use crate::listener::{PeerAddr, PeerTls};
use crate::stream::ConnectedAgent;
use crate::tls::PeerIdentity;

/// Bounds of one `Connect` stream (rule 5).
#[derive(Debug, Clone, Copy)]
pub struct StreamLimits {
    /// `Hello` must be the first message and arrive within this long.
    pub hello_timeout: Duration,
    /// A stream that carries nothing for this long is closed. Agents send a heartbeat every few seconds.
    pub idle_timeout: Duration,
    /// One call into the sink.
    pub sink_timeout: Duration,
    /// Writing one message to an agent that is not reading.
    pub send_timeout: Duration,
    /// A certificate renewal sooner than this after the last one on the same stream is ignored.
    pub renewal_min_interval: Duration,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            hello_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            sink_timeout: Duration::from_secs(30),
            send_timeout: Duration::from_secs(30),
            renewal_min_interval: Duration::from_secs(60),
        }
    }
}

#[derive(Clone)]
pub struct AgentService {
    join: Arc<JoinService>,
    sink: Arc<dyn ReportSink>,
    limits: StreamLimits,
    /// `true` once the hub is shutting down: every stream ends and tells its agent to reconnect elsewhere.
    closing: watch::Receiver<bool>,
    metrics: AgentMetrics,
    /// Where open streams register, so that commands reach them from any replica (T4). Without it the service still ingests
    /// what agents send, and a reply has no request to complete.
    gateway: Option<Arc<HubGateway>>,
}

impl std::fmt::Debug for AgentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentService").finish_non_exhaustive()
    }
}

impl AgentService {
    #[must_use]
    pub fn new(
        join: Arc<JoinService>,
        sink: Arc<dyn ReportSink>,
        closing: watch::Receiver<bool>,
        metrics: AgentMetrics,
        limits: StreamLimits,
    ) -> Self {
        Self {
            join,
            sink,
            limits,
            closing,
            metrics,
            gateway: None,
        }
    }

    /// Register every stream with `gateway` and hand it the agents' replies (D62).
    #[must_use]
    pub fn with_gateway(mut self, gateway: Arc<HubGateway>) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// The service as a router, with zstd and the message size limit (`proto::grpc`).
    #[must_use]
    pub fn routes(self) -> Routes {
        Routes::new(proto::grpc::agent_server(self))
    }
}

/// The swimlane the verified certificate names, or the status that refuses the call.
fn certificate_swimlane<T>(request: &Request<T>) -> Result<SwimlaneId, Status> {
    match request.extensions().get::<PeerTls>() {
        Some(PeerTls(Some(cert))) => match &cert.identity {
            PeerIdentity::Agent(swimlane) => Ok(swimlane.clone()),
            // A sentinel or a replica certificate is a valid certificate for another service (S5).
            PeerIdentity::Sentinel(_) | PeerIdentity::Hub(_) => Err(Status::permission_denied("")),
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
impl pb::agent_server::Agent for AgentService {
    async fn join(&self, request: Request<pb::JoinRequest>) -> Result<Response<pb::JoinResponse>, Status> {
        let remote = request.extensions().get::<PeerAddr>().map(|addr| addr.0.ip());
        // A request that does not even parse is refused like any other: the same status, no reason.
        let params = JoinParams::try_from(request.into_inner()).map_err(|_| Status::unauthenticated(""))?;
        match self.join.join_agent(remote, params).await {
            Ok(issued) => Ok(Response::new(issued.into())),
            Err(error) => Err(Status::new(error.status_code(), "")),
        }
    }

    type ConnectStream = ReceiverStream<Result<pb::HubMessage, Status>>;

    async fn connect(
        &self,
        request: Request<Streaming<pb::AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        let swimlane = certificate_swimlane(&request)?;
        let mut inbound = request.into_inner();

        // The agent speaks first, and what it says first is `Hello`, for the swimlane its certificate names.
        let first = tokio::time::timeout(self.limits.hello_timeout, inbound.message())
            .await
            .map_err(|_| Status::deadline_exceeded(""))?
            .map_err(|_| Status::invalid_argument(""))?
            .ok_or_else(|| Status::cancelled(""))?;
        self.metrics.message(MessageKind::Hello);
        let Ok(Some(FromAgent::Hello(hello))) = FromAgent::from_proto(first) else {
            debug!(
                code = "connect_hello_not_first",
                "the first message was not a valid Hello"
            );
            return Err(Status::failed_precondition(""));
        };
        if hello.swimlane != swimlane {
            debug!(
                code = "connect_hello_swimlane_mismatch",
                "Hello names another swimlane than the certificate"
            );
            return Err(Status::permission_denied(""));
        }
        let identity = AgentIdentity::new(swimlane.clone());
        let config = within(self.limits.sink_timeout, self.sink.hello(&identity, hello))
            .await
            .map_err(|error| {
                self.metrics.sink_failure(MessageKind::Hello);
                sink_status(error)
            })?;

        let (agent, queue) = ConnectedAgent::new(swimlane);
        agent
            .try_send(ToAgent::Config(config))
            .map_err(|_| Status::unavailable(""))?;
        // The registration comes before the response: once the agent has its stream, commands can already be routed to it.
        let attachment = match &self.gateway {
            Some(gateway) => Some(gateway.attach(Arc::clone(&agent)).await.map_err(|error| {
                warn!(code = "connect_attach_failed", %error, "an agent stream could not be registered");
                Status::unavailable("")
            })?),
            None => None,
        };
        self.metrics.stream_opened();
        let session = Session {
            join: Arc::clone(&self.join),
            sink: Arc::clone(&self.sink),
            limits: self.limits,
            closing: self.closing.clone(),
            metrics: self.metrics.clone(),
            identity,
            agent,
            attachment,
            last_renewal: None,
        };
        tokio::spawn(session.run(inbound));
        Ok(Response::new(queue))
    }
}

/// What to do after handling one message.
enum Flow {
    Continue,
    End(StreamEnd, Option<Status>),
}

struct Session {
    join: Arc<JoinService>,
    sink: Arc<dyn ReportSink>,
    limits: StreamLimits,
    closing: watch::Receiver<bool>,
    metrics: AgentMetrics,
    identity: AgentIdentity,
    agent: Arc<ConnectedAgent>,
    attachment: Option<Attachment>,
    last_renewal: Option<Instant>,
}

impl Session {
    async fn run(mut self, mut inbound: Streaming<pb::AgentMessage>) {
        let mut agent_closed = self.agent.closed();
        let (reason, status) = loop {
            let received = tokio::select! {
                biased;
                _ = self.closing.changed() => break (StreamEnd::HubClosing, Some(Status::unavailable(""))),
                _ = agent_closed.changed() => break (StreamEnd::Closed, Some(Status::unavailable(""))),
                received = tokio::time::timeout(self.limits.idle_timeout, inbound.message()) => received,
            };
            let message = match received {
                Err(_elapsed) => break (StreamEnd::Idle, Some(Status::deadline_exceeded(""))),
                // A message the transport refused (too large, bad framing) or a reset.
                Ok(Err(_status)) => break (StreamEnd::Protocol, Some(Status::invalid_argument(""))),
                Ok(Ok(None)) => break (StreamEnd::AgentClosed, None),
                Ok(Ok(Some(message))) => message,
            };
            let parsed = match FromAgent::from_proto(message) {
                Ok(Some(parsed)) => parsed,
                // A message kind this build does not know: skip it and keep the stream (the contract's rule for new variants).
                Ok(None) => continue,
                Err(error) => {
                    debug!(code = "connect_bad_message", %error, "an agent message failed validation");
                    break (StreamEnd::Protocol, Some(Status::invalid_argument("")));
                }
            };
            if let Flow::End(reason, status) = self.handle(parsed).await {
                break (reason, status);
            }
        };
        if let Some(status) = status {
            self.agent.finish(status);
        }
        // Requests still waiting on this stream fail with `Closed` now, and the registration is released if it is still ours.
        if let Some(attachment) = self.attachment.take() {
            attachment.detach().await;
        }
        self.metrics.stream_closed(reason);
    }

    async fn send(&self, message: ToAgent) -> Flow {
        match self.agent.send(message, self.limits.send_timeout).await {
            Ok(()) => Flow::Continue,
            Err(crate::stream::SendError::Full) => {
                Flow::End(StreamEnd::Stalled, Some(Status::unavailable("")))
            }
            Err(crate::stream::SendError::Closed) => Flow::End(StreamEnd::AgentClosed, None),
        }
    }

    /// What a sink refusal means for the stream. An unavailable sink or a payload it did not like costs the agent a resend
    /// (it keeps unacknowledged versions in its spool); a message that breaks the protocol ends the stream.
    fn sink_failed(&self, kind: MessageKind, error: SinkError) -> Flow {
        self.metrics.sink_failure(kind);
        warn!(code = ?error, "the ingestion sink refused an agent message");
        match error {
            SinkError::HelloRequired | SinkError::IdentityMismatch => {
                Flow::End(StreamEnd::Protocol, Some(sink_status(error)))
            }
            SinkError::Invalid | SinkError::TooLarge | SinkError::Unavailable => Flow::Continue,
        }
    }

    async fn handle(&mut self, message: FromAgent) -> Flow {
        match message {
            FromAgent::Hello(_) => {
                self.metrics.message(MessageKind::Hello);
                Flow::End(StreamEnd::Protocol, Some(Status::failed_precondition("")))
            }
            FromAgent::Heartbeat(heartbeat) => {
                self.metrics.message(MessageKind::Heartbeat);
                match within(
                    self.limits.sink_timeout,
                    self.sink.heartbeat(&self.identity, heartbeat),
                )
                .await
                {
                    Ok(domain::HeartbeatAction::None) => Flow::Continue,
                    Ok(domain::HeartbeatAction::RequestDelta { since_root }) => {
                        self.send(ToAgent::Command(HubCommand::RequestDelta { since_root }))
                            .await
                    }
                    Ok(domain::HeartbeatAction::RequestFullScan) => {
                        self.send(ToAgent::Command(HubCommand::RequestFullScan)).await
                    }
                    Err(error) => self.sink_failed(MessageKind::Heartbeat, error),
                }
            }
            FromAgent::Delta(delta) => {
                self.metrics.message(MessageKind::ScanDelta);
                match within(self.limits.sink_timeout, self.sink.delta(&self.identity, delta)).await {
                    Ok(()) => Flow::Continue,
                    Err(error) => self.sink_failed(MessageKind::ScanDelta, error),
                }
            }
            FromAgent::Cluster(report) => {
                self.metrics.message(MessageKind::ClusterReport);
                match within(
                    self.limits.sink_timeout,
                    self.sink.cluster(&self.identity, report),
                )
                .await
                {
                    Ok(()) => Flow::Continue,
                    Err(error) => self.sink_failed(MessageKind::ClusterReport, error),
                }
            }
            FromAgent::Reply(reply) => {
                self.metrics.message(MessageKind::Reply);
                // A reply completes a request of this stream only, by request id. Anything else (an id nobody waits for, one
                // already answered or timed out, a kind of answer the command cannot have) is dropped and counted.
                if !self
                    .attachment
                    .as_ref()
                    .is_some_and(|attachment| attachment.complete(reply))
                {
                    self.metrics.unmatched_reply();
                    debug!(
                        code = "reply_without_request",
                        "a reply arrived that no request is waiting for"
                    );
                }
                Flow::Continue
            }
            FromAgent::CertRenewal { csr_der } => {
                self.metrics.message(MessageKind::CertRenewal);
                let now = Instant::now();
                if self
                    .last_renewal
                    .is_some_and(|last| now.duration_since(last) < self.limits.renewal_min_interval)
                {
                    self.metrics.renewal_throttled();
                    return Flow::Continue;
                }
                self.last_renewal = Some(now);
                // The identity renewed is the stream's, taken from the certificate that opened it.
                match self.join.renew_agent(self.agent.swimlane(), &csr_der).await {
                    Ok(issued) => self.send(ToAgent::CertRenewal(issued)).await,
                    Err(error) => {
                        warn!(code = "renewal_failed", %error, "a certificate renewal failed; the agent will ask again");
                        Flow::Continue
                    }
                }
            }
        }
    }
}
