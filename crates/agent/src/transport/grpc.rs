//! The hub transport over gRPC (HTTP/2) with TLS 1.3 and the pinned CA (S6).
//!
//! The generated client from `proto::grpc` runs over an HTTP/2 connection that this file opens itself:
//!
//! 1. the [`Dialer`] opens the byte stream;
//! 2. TLS 1.3 is spoken over it with [`tls::client_config`], with the client certificate for `Connect` and without
//!    one for `Join`;
//! 3. hyper speaks HTTP/2 over the TLS stream, with keepalive pings so a dead path is noticed.
//!
//! tonic's own `Channel` is not used. It would put the URI scheme, the TLS handling and the keepalive behind feature
//! flags that other crates in the workspace can change, and it adds a request buffer and a reconnect loop that the
//! session already provides.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use http::uri::{Authority, PathAndQuery, Scheme};
use http::{Request, Response, Uri};
use hyper::body::Incoming;
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use proto::convert::{IssuedCert, JoinParams};
use proto::pb;
use rustls::pki_types::ServerName;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Code;
use tower::Service;
use tracing::debug;

use super::dial::{Dialer, TcpDialer};
use super::error::TransportError;
use super::tls::{self, HubRoots};
use super::{Connection, HubTransport};
use crate::config::BaseUrl;
use crate::identity::ClientIdentity;
use crate::identity::JoinError;
use crate::identity::joiner::JoinClient;

/// TCP, TLS and the HTTP/2 handshake together.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// From sending the request to getting the response headers of `Connect`.
pub const OPEN_STREAM_TIMEOUT: Duration = Duration::from_secs(15);
/// A whole `Join` call.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(20);
/// HTTP/2 pings, so that a path that silently died is noticed within `KEEPALIVE_INTERVAL + KEEPALIVE_TIMEOUT` (S6).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// Messages waiting between the writer and the HTTP/2 body. One is enough: the outbox is the real queue.
const WIRE_QUEUE: usize = 1;

/// The hub, as `host:port` behind TLS.
pub struct GrpcTransport {
    host: String,
    port: u16,
    server_name: ServerName<'static>,
    origin: (Scheme, Authority),
    roots: HubRoots,
    dialer: Arc<dyn Dialer>,
}

impl fmt::Debug for GrpcTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrpcTransport")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl GrpcTransport {
    /// Over TCP to `endpoint` (`https://host[:port]`), trusting only `roots`.
    pub fn tcp(endpoint: &BaseUrl, roots: HubRoots) -> Result<Self, TransportError> {
        Self::new(endpoint, roots, Arc::new(TcpDialer))
    }

    pub fn new(endpoint: &BaseUrl, roots: HubRoots, dialer: Arc<dyn Dialer>) -> Result<Self, TransportError> {
        let uri: Uri = endpoint
            .as_str()
            .parse()
            .map_err(|_| TransportError::Address("not a URL"))?;
        if uri.scheme() != Some(&Scheme::HTTPS) {
            return Err(TransportError::Address("the hub must be reached over https"));
        }
        let authority = uri
            .authority()
            .cloned()
            .ok_or(TransportError::Address("no host"))?;
        // `Uri::host` keeps the brackets of an IPv6 address; neither the resolver nor TLS wants them.
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let server_name = ServerName::try_from(host.clone())
            .map_err(|_| TransportError::Address("not a valid host name"))?;
        Ok(Self {
            port: authority.port_u16().unwrap_or(443),
            host,
            server_name,
            origin: (Scheme::HTTPS, authority),
            roots,
            dialer,
        })
    }

    /// An HTTP/2 connection to the hub: TCP, TLS 1.3, HTTP/2, in that order, within [`CONNECT_TIMEOUT`].
    async fn open(&self, identity: Option<&ClientIdentity>) -> Result<H2, TransportError> {
        let config = tls::client_config(&self.roots, identity)
            .map_err(|_| TransportError::Address("the TLS configuration was refused"))?;
        let handshake = async {
            let tcp = self.dialer.dial(&self.host, self.port).await.map_err(|error| {
                debug!(kind = ?error.kind(), "cannot reach the hub");
                TransportError::Unreachable
            })?;
            let secured = TlsConnector::from(config)
                .connect(self.server_name.clone(), tcp)
                .await
                .map_err(|error| TransportError::Tls(tls::classify(&error)))?;
            http2::Builder::new(TokioExecutor::new())
                .timer(TokioTimer::new())
                .keep_alive_interval(KEEPALIVE_INTERVAL)
                .keep_alive_timeout(KEEPALIVE_TIMEOUT)
                .keep_alive_while_idle(true)
                .handshake(TokioIo::new(secured))
                .await
                .map_err(|error| {
                    debug!(%error, "HTTP/2 handshake with the hub failed");
                    TransportError::Stream
                })
        };
        let (send, connection) = tokio::time::timeout(CONNECT_TIMEOUT, handshake)
            .await
            .map_err(|_| TransportError::Timeout)??;
        // The connection is a future that moves the bytes. It ends when the connection closes, which happens when the
        // last request handle and stream are dropped or when keepalive finds the path dead.
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                debug!(%error, "the connection to the hub ended with an error");
            }
        });
        Ok(H2 {
            send,
            origin: self.origin.clone(),
        })
    }
}

#[async_trait]
impl HubTransport for GrpcTransport {
    async fn connect(
        &self,
        identity: &ClientIdentity,
        first: pb::AgentMessage,
    ) -> Result<Connection, TransportError> {
        let h2 = self.open(Some(identity)).await?;
        let (outbound, queue) = mpsc::channel(WIRE_QUEUE);
        // `Hello` is queued before the call is made: a hub that reads it before answering must not wait for us.
        outbound.try_send(first).map_err(|_| TransportError::Closed)?;
        let mut client = proto::grpc::agent_client(h2.clone());
        let call = client.connect(tonic::Request::new(ReceiverStream::new(queue)));
        let response = tokio::time::timeout(OPEN_STREAM_TIMEOUT, call)
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|status| from_status(&status))?;
        // The HTTP/2 handle rides along with the stream, so the connection lives exactly as long as the stream does.
        let keep_alive = h2;
        let inbound = response.into_inner().map(move |item| {
            let _connection = &keep_alive;
            item.map_err(|status| from_status(&status))
        });
        Ok(Connection {
            outbound,
            inbound: Box::pin(inbound),
        })
    }
}

#[async_trait]
impl JoinClient for GrpcTransport {
    /// `Join` is made on its own connection, without a client certificate: there may be no working one yet.
    async fn join(&self, params: JoinParams) -> Result<IssuedCert, JoinError> {
        let h2 = self.open(None).await.map_err(|error| {
            debug!(%error, "cannot reach the hub to join");
            JoinError::Unavailable
        })?;
        let mut client = proto::grpc::agent_client(h2);
        let mut request = tonic::Request::new(pb::JoinRequest::from(params));
        request.set_timeout(JOIN_TIMEOUT);
        let response = tokio::time::timeout(JOIN_TIMEOUT, client.join(request))
            .await
            .map_err(|_| JoinError::Unavailable)?
            .map_err(|status| match status.code() {
                Code::Unauthenticated | Code::PermissionDenied | Code::InvalidArgument => JoinError::Rejected,
                _ => JoinError::Unavailable,
            })?;
        IssuedCert::try_from(response.into_inner()).map_err(|_| JoinError::Invalid)
    }
}

/// What a gRPC status means for the connection. The message text comes from the hub and is not logged.
fn from_status(status: &tonic::Status) -> TransportError {
    match status.code() {
        Code::Unauthenticated | Code::PermissionDenied | Code::InvalidArgument | Code::FailedPrecondition => {
            TransportError::Refused
        }
        Code::DeadlineExceeded => TransportError::Timeout,
        Code::Ok | Code::Cancelled => TransportError::Closed,
        _ => TransportError::Stream,
    }
}

/// An HTTP/2 connection as the service the generated client wants: it adds the scheme and authority to the path-only
/// URIs the generated code uses.
#[derive(Clone)]
struct H2 {
    send: http2::SendRequest<tonic::body::Body>,
    origin: (Scheme, Authority),
}

impl Service<Request<tonic::body::Body>> for H2 {
    type Response = Response<Incoming>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.send.poll_ready(cx)
    }

    fn call(&mut self, request: Request<tonic::body::Body>) -> Self::Future {
        let (mut parts, body) = request.into_parts();
        let path = parts
            .uri
            .path_and_query()
            .cloned()
            .unwrap_or_else(|| PathAndQuery::from_static("/"));
        let mut uri = Uri::builder()
            .scheme(self.origin.0.clone())
            .authority(self.origin.1.clone());
        uri = uri.path_and_query(path);
        if let Ok(uri) = uri.build() {
            parts.uri = uri;
        }
        let call = self.send.send_request(Request::from_parts(parts, body));
        Box::pin(call)
    }
}
