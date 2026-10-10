//! The connection to the hub (T3, S6).
//!
//! - [`tls`]: TLS 1.3 only, one pinned CA, a client certificate only when there is one.
//! - [`dial`]: the byte stream under TLS, behind a trait so that tests can count and cut it.
//! - [`grpc`]: the production [`HubTransport`], and `Join`, over HTTP/2 with zstd.
//! - [`wire`]: the line between generated messages and validated ones; nothing else in the agent touches `proto::pb`.
//! - [`outbox`]: the bounded queue in front of the stream.
//! - [`session`]: reconnecting with backoff, the `Hello` and `AgentConfig` exchange, certificate renewal over the stream.
pub mod dial;
pub mod error;
pub mod grpc;
pub mod outbox;
pub mod session;
pub mod tls;
pub mod wire;

use std::fmt;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use proto::pb;
use tokio::sync::mpsc;

pub use dial::{BoxedIo, Dialer, Io, TcpDialer};
pub use error::{TlsError, TlsFailure, TransportError};
pub use grpc::GrpcTransport;
pub use outbox::{Outbox, OutboxError, OutboxLimits, OutboxReceiver};
pub use session::{ConnectionState, Link, LinkHandler, Session, SessionConfig, SessionStats};

use crate::identity::ClientIdentity;

/// What comes from the hub, one generated message at a time. [`wire::decode`] validates each.
pub type InboundStream = Pin<Box<dyn Stream<Item = Result<pb::HubMessage, TransportError>> + Send>>;

/// An open stream to the hub.
pub struct Connection {
    /// Messages for the hub. Small and bounded: the real queue is the [`Outbox`] in front of it. Dropping it ends the
    /// stream from this side.
    pub outbound: mpsc::Sender<pb::AgentMessage>,
    pub inbound: InboundStream,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Connection")
    }
}

/// The way to the hub. gRPC over HTTP/2 today ([`GrpcTransport`]); a WebSocket over HTTPS could implement the same trait
/// if a proxy ever blocks HTTP/2 trailers.
#[async_trait]
pub trait HubTransport: Send + Sync + fmt::Debug {
    /// Open the stream with this identity's certificate. `first` goes out before the call is answered, so a hub that
    /// reads the agent's `Hello` before it replies does not wait for an agent that waits for the reply.
    async fn connect(
        &self,
        identity: &ClientIdentity,
        first: pb::AgentMessage,
    ) -> Result<Connection, TransportError>;
}
