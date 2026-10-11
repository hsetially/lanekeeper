//! The internal `Forward` RPC (D62, S5, S6): deliver a command to an agent stream this replica holds, on behalf of a peer.
//!
//! It runs on its own listener with its own TLS configuration, which trusts the replica CA and nothing else, so an agent's
//! certificate cannot even complete a handshake here. On top of that, every call checks that the peer's certificate names a
//! hub replica (`spiffe://lanekeeper/hub/<replica>`), because a certificate of the right CA with another kind of name is not a
//! replica either. The request is validated like anything from outside ([`ForwardCall`]), and a call whose epoch is not the
//! epoch of the stream held here is refused: the agent has moved on.

use std::sync::Arc;

use tonic::service::Routes;
use tonic::{Request, Response, Status};
use tracing::debug;

use super::local::LocalStreams;
use super::metrics::{GatewayMetrics, ServeOutcome};
use super::wire::{self, ForwardCall, MAX_FORWARD_BYTES, pb};
use crate::listener::PeerTls;
use crate::tls::PeerIdentity;

#[derive(Clone)]
pub struct ReplicaService {
    local: Arc<LocalStreams>,
    metrics: GatewayMetrics,
}

impl std::fmt::Debug for ReplicaService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplicaService").finish_non_exhaustive()
    }
}

impl ReplicaService {
    #[must_use]
    pub fn new(local: Arc<LocalStreams>, metrics: GatewayMetrics) -> Self {
        Self { local, metrics }
    }

    /// The service as a router, with the message size limit.
    #[must_use]
    pub fn routes(self) -> Routes {
        Routes::new(
            pb::replica_server::ReplicaServer::new(self)
                .max_decoding_message_size(MAX_FORWARD_BYTES)
                .max_encoding_message_size(MAX_FORWARD_BYTES),
        )
    }
}

/// Whether the verified certificate of the caller names a hub replica. The listener put it there after the handshake.
fn require_replica<T>(request: &Request<T>) -> Result<(), Status> {
    match request.extensions().get::<PeerTls>() {
        Some(PeerTls(Some(cert))) => match &cert.identity {
            PeerIdentity::Hub(_) => Ok(()),
            // An agent or sentinel certificate is valid for its own service and for nothing here (S5).
            PeerIdentity::Agent(_) | PeerIdentity::Sentinel(_) => Err(Status::permission_denied("")),
        },
        _ => Err(Status::unauthenticated("")),
    }
}

#[async_trait::async_trait]
impl pb::replica_server::Replica for ReplicaService {
    async fn forward(
        &self,
        request: Request<pb::ForwardRequest>,
    ) -> Result<Response<pb::ForwardReply>, Status> {
        if let Err(status) = require_replica(&request) {
            self.metrics.served(ServeOutcome::Denied);
            return Err(status);
        }
        let call = match ForwardCall::try_from(request.into_inner()) {
            Ok(call) => call,
            Err(error) => {
                self.metrics.served(ServeOutcome::Rejected);
                debug!(code = "forward_rejected", %error, "a forwarded request failed validation");
                return Err(Status::invalid_argument(""));
            }
        };
        let stream = self
            .local
            .get(&call.swimlane)
            .filter(|stream| stream.epoch() == call.epoch);
        let Some(stream) = stream else {
            // No stream here, or another generation of it: the caller read a registration that is out of date.
            self.metrics.served(ServeOutcome::StaleEpoch);
            return Err(Status::failed_precondition(""));
        };
        self.metrics.served(ServeOutcome::Answered);
        // If the caller goes away, tonic drops this future, which drops the request's ticket and frees its slot and id.
        let reply = match stream.request(call.command, call.timeout).await {
            Ok(reply) => wire::reply_to_wire(reply),
            Err(error) => pb::ForwardReply {
                result: Some(pb::forward_reply::Result::Error(
                    wire::error_to_wire(error).into(),
                )),
            },
        };
        Ok(Response::new(reply))
    }
}
