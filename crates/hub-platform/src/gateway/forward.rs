//! Calling the replica that holds an agent's stream (D62, S6): the client side of the internal `Forward` RPC.
//!
//! One HTTP/2 connection per peer, over TLS 1.3 with this replica's own certificate (issued by the replica CA, so the peer
//! knows it is a hub) and with the peer's certificate checked against the replica CA only. Connections are pooled in a bounded
//! cache and multiplexed, so a burst of requests to one peer shares one handshake.
//!
//! A command is never sent twice. If the connection breaks after a request went out, the caller gets
//! [`GatewayError::Unavailable`] (the command may or may not have reached the agent, as the port documents), the pooled
//! connection is dropped, and the next call dials again.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use domain::AgentReply;
use http::uri::{Authority, PathAndQuery, Scheme};
use http::{Request, Response, Uri};
use hyper::body::Incoming;
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use moka::sync::Cache;
use ports::GatewayError;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tonic::Code;
use tower::Service;
use tracing::{debug, warn};

use super::wire::{self, ForwardCall, MAX_FORWARD_BYTES, pb};
use crate::tls::ClientTls;

/// `host:port` of a replica, as it writes it into `agent_connections.replica_address`: a DNS name (the pod's name behind the
/// headless service) and a port. Validated before it is used to dial anything, because the value comes from a table.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerAddress {
    text: String,
    host_len: usize,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("a replica address is host:port")]
    Shape,
    #[error("the host is not a DNS name")]
    Host,
    #[error("the port is not valid")]
    Port,
}

impl PeerAddress {
    pub const MAX_LEN: usize = 253 + 6;

    pub fn parse(s: &str) -> Result<Self, AddressError> {
        if s.len() > Self::MAX_LEN {
            return Err(AddressError::Shape);
        }
        let (host, port) = s.rsplit_once(':').ok_or(AddressError::Shape)?;
        let label_ok = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        };
        if host.is_empty() || host.len() > 253 || !host.split('.').all(label_ok) {
            return Err(AddressError::Host);
        }
        let port: u16 = port.parse().map_err(|_| AddressError::Port)?;
        if port == 0 || !port.to_string().eq(&s[host.len() + 1..]) {
            return Err(AddressError::Port);
        }
        Ok(Self {
            text: s.to_owned(),
            host_len: host.len(),
            port,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.text[..self.host_len]
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl std::fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Bounds of the replica client (rule 5).
#[derive(Debug, Clone, Copy)]
pub struct ReplicaClientLimits {
    /// Peers with a pooled connection. A hub has a handful of peers; the cache is a bound, not a target.
    pub max_peers: u64,
    /// TCP, TLS and HTTP/2 set-up together.
    pub connect_timeout: Duration,
    /// A pooled connection unused for this long is closed.
    pub idle: Duration,
    /// Added to the request's own timeout for the call itself: the owner answers at the request's deadline, and the network
    /// needs a moment to carry that answer back.
    pub call_slack: Duration,
}

impl Default for ReplicaClientLimits {
    fn default() -> Self {
        Self {
            max_peers: 64,
            connect_timeout: Duration::from_secs(5),
            idle: Duration::from_secs(300),
            call_slack: Duration::from_secs(3),
        }
    }
}

/// Why a forwarded request did not produce an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardFailure {
    /// The owner no longer holds the stream with the epoch the caller read: the agent has moved, re-read the registry.
    StaleEpoch,
    Gateway(GatewayError),
}

/// An HTTP/2 connection as the generated client wants it: it adds the scheme and authority the generated code leaves out.
#[derive(Clone)]
struct PeerChannel {
    send: http2::SendRequest<tonic::body::Body>,
    scheme: Scheme,
    authority: Authority,
}

impl Service<Request<tonic::body::Body>> for PeerChannel {
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
        if let Ok(uri) = Uri::builder()
            .scheme(self.scheme.clone())
            .authority(self.authority.clone())
            .path_and_query(path)
            .build()
        {
            parts.uri = uri;
        }
        Box::pin(self.send.send_request(Request::from_parts(parts, body)))
    }
}

fn dial_failed(code: &'static str) -> GatewayError {
    // Only a fixed code is logged: the text of a connection error can carry peer-controlled bytes.
    debug!(code, "dialling a peer replica failed");
    GatewayError::Unavailable
}

/// The replica RPC client, with the pool of peer connections.
pub struct ReplicaClient {
    tls: ClientTls,
    limits: ReplicaClientLimits,
    peers: Cache<String, PeerChannel>,
    /// One dial at a time: a burst of first requests to a peer shares one handshake instead of opening hundreds.
    dial: Mutex<()>,
}

impl std::fmt::Debug for ReplicaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplicaClient").finish_non_exhaustive()
    }
}

impl ReplicaClient {
    #[must_use]
    pub fn new(tls: ClientTls, limits: ReplicaClientLimits) -> Self {
        Self {
            tls,
            limits,
            peers: Cache::builder()
                .max_capacity(limits.max_peers)
                .time_to_idle(limits.idle)
                .build(),
            dial: Mutex::new(()),
        }
    }

    /// Peers with a pooled connection now, for the test that shows the pool is bounded.
    #[must_use]
    pub fn pooled(&self) -> u64 {
        self.peers.run_pending_tasks();
        self.peers.entry_count()
    }

    async fn channel(&self, address: &PeerAddress) -> Result<PeerChannel, GatewayError> {
        if let Some(channel) = self.peers.get(address.as_str()) {
            if !channel.send.is_closed() {
                return Ok(channel);
            }
            self.peers.invalidate(address.as_str());
        }
        let _one_at_a_time = self.dial.lock().await;
        // Another caller may have dialled while this one waited.
        if let Some(channel) = self.peers.get(address.as_str()) {
            if !channel.send.is_closed() {
                return Ok(channel);
            }
            self.peers.invalidate(address.as_str());
        }
        let channel = tokio::time::timeout(self.limits.connect_timeout, self.dial_peer(address))
            .await
            .map_err(|_| {
                debug!(
                    code = "replica_connect_timeout",
                    "dialling a peer replica timed out"
                );
                GatewayError::Unavailable
            })??;
        self.peers.insert(address.as_str().to_owned(), channel.clone());
        Ok(channel)
    }

    async fn dial_peer(&self, address: &PeerAddress) -> Result<PeerChannel, GatewayError> {
        let tcp = TcpStream::connect((address.host(), address.port()))
            .await
            .map_err(|_| dial_failed("replica_connect_failed"))?;
        if tcp.set_nodelay(true).is_err() {
            debug!("could not set TCP_NODELAY toward a peer");
        }
        let name = ServerName::try_from(address.host().to_owned())
            .map_err(|_| dial_failed("replica_bad_server_name"))?;
        let tls = self
            .tls
            .connector()
            .connect(name, tcp)
            .await
            .map_err(|_| dial_failed("replica_tls_failed"))?;
        let (send, connection) = http2::Builder::new(TokioExecutor::new())
            .timer(TokioTimer::new())
            .keep_alive_interval(Some(Duration::from_secs(30)))
            .keep_alive_timeout(Duration::from_secs(20))
            .keep_alive_while_idle(true)
            .handshake(TokioIo::new(tls))
            .await
            .map_err(|_| dial_failed("replica_http2_failed"))?;
        tokio::spawn(async move {
            // The connection ends when the peer goes away or every handle is dropped; either is fine.
            let _ = connection.await;
        });
        let authority =
            Authority::try_from(address.as_str()).map_err(|_| dial_failed("replica_bad_authority"))?;
        Ok(PeerChannel {
            send,
            scheme: Scheme::HTTPS,
            authority,
        })
    }

    /// Deliver `call` to the replica at `address` and return the agent's answer.
    pub async fn forward(
        &self,
        address: &PeerAddress,
        call: ForwardCall,
    ) -> Result<AgentReply, ForwardFailure> {
        let wait = call.timeout + self.limits.call_slack;
        let channel = self.channel(address).await.map_err(ForwardFailure::Gateway)?;
        let mut client = pb::replica_client::ReplicaClient::new(channel)
            .max_decoding_message_size(MAX_FORWARD_BYTES)
            .max_encoding_message_size(MAX_FORWARD_BYTES);
        let mut request = tonic::Request::new(call.into_proto());
        request.set_timeout(wait);
        let answer = match tokio::time::timeout(wait, client.forward(request)).await {
            Err(_elapsed) => return Err(ForwardFailure::Gateway(GatewayError::Timeout)),
            Ok(answer) => answer,
        };
        match answer {
            Ok(response) => wire::reply_from_wire(response.into_inner()).map_err(ForwardFailure::Gateway),
            Err(status) => {
                let code = status.code();
                if code == Code::FailedPrecondition {
                    return Err(ForwardFailure::StaleEpoch);
                }
                // Only the code is logged: a status message from a peer is not ours to trust into a log line.
                warn!(code = ?code, "a peer replica refused or failed a forwarded request");
                if matches!(
                    code,
                    Code::Unavailable | Code::Unknown | Code::Cancelled | Code::Internal
                ) {
                    // The connection is suspect: the next call dials again.
                    self.peers.invalidate(address.as_str());
                }
                Err(ForwardFailure::Gateway(GatewayError::Unavailable))
            }
        }
    }
}
