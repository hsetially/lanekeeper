//! The gRPC listener for agents, sentinels and replicas (S5, S6): TLS 1.3 with client certificates, then HTTP/2, then the
//! generated services.
//!
//! tonic's own server is not used. This accept loop owns the three things that must not be left to defaults: how many
//! connections and handshakes are in flight (rule 5), which TLS configuration speaks (see [`crate::tls`]), and how the
//! verified peer certificate reaches a handler: as the [`PeerCert`] extension of the request, put there after the handshake,
//! never from anything the peer wrote.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper::server::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tonic::service::Routes;
use tower::Service;
use tracing::{debug, warn};

use crate::tls::{IdentityError, PeerCert};

/// The address of the peer's TCP connection, for limits and logs. Inserted into the request extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

/// What came of the peer's certificate, for a handler to read. `None` is a connection that presented none (`Join` only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerTls(pub Option<PeerCert>);

/// Bounds of the listener (rule 5).
#[derive(Debug, Clone, Copy)]
pub struct GrpcLimits {
    /// Open connections at once; one more is closed without an answer.
    pub max_connections: usize,
    /// A TLS handshake has this long.
    pub handshake_timeout: Duration,
    /// HTTP/2 streams (calls) one connection may have open.
    pub max_concurrent_streams: u32,
    /// HTTP/2 pings, so that a path that silently died is noticed within `keepalive_interval + keepalive_timeout`.
    pub keepalive_interval: Duration,
    pub keepalive_timeout: Duration,
    /// After the stop signal, connections in flight have this long.
    pub drain_timeout: Duration,
}

impl Default for GrpcLimits {
    fn default() -> Self {
        Self {
            max_connections: 2048,
            handshake_timeout: Duration::from_secs(10),
            max_concurrent_streams: 32,
            keepalive_interval: Duration::from_secs(30),
            keepalive_timeout: Duration::from_secs(20),
            drain_timeout: Duration::from_secs(25),
        }
    }
}

/// Counters of what the listener turned away or finished, for metrics and for tests.
#[derive(Debug, Default)]
pub struct ListenerStats {
    pub accepted: AtomicU64,
    pub handshakes_failed: AtomicU64,
    pub refused_at_limit: AtomicU64,
    /// Certificates that passed TLS but name no identity the hub knows.
    pub unknown_identities: AtomicU64,
}

impl ListenerStats {
    #[must_use]
    pub fn handshakes_failed(&self) -> u64 {
        self.handshakes_failed.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }
}

/// Puts what the connection proved into each request, then hands it to the routes.
#[derive(Clone)]
struct WithPeer {
    routes: Routes,
    addr: SocketAddr,
    tls: PeerTls,
}

impl<B> Service<http::Request<B>> for WithPeer
where
    Routes: Service<http::Request<B>, Error = Infallible>,
{
    type Response = <Routes as Service<http::Request<B>>>::Response;
    type Error = Infallible;
    type Future = <Routes as Service<http::Request<B>>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.routes.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        // Whatever the peer put in its request, these two are ours.
        request.extensions_mut().insert(PeerAddr(self.addr));
        request.extensions_mut().insert(self.tls.clone());
        self.routes.call(request)
    }
}

/// Accept connections on `listener` until `stop` resolves, serve `routes` on them over `tls`, then wait up to
/// `limits.drain_timeout` for connections in flight. Long-lived streams are the services' to end (they watch the same
/// shutdown), so that the wait does not hang on them.
pub async fn serve_grpc(
    listener: TcpListener,
    tls: TlsAcceptor,
    routes: Routes,
    stop: impl Future<Output = ()> + Send,
    limits: GrpcLimits,
    stats: Arc<ListenerStats>,
) {
    let permits = Arc::new(Semaphore::new(limits.max_connections));
    let graceful = GracefulShutdown::new();
    let mut stop = std::pin::pin!(stop);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut stop => break,
        };
        let (stream, addr) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                // File-descriptor exhaustion and the like: log the kind, back off briefly instead of spinning.
                warn!(kind = ?error.kind(), "accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            stats.refused_at_limit.fetch_add(1, Ordering::Relaxed);
            debug!("connection refused: at the connection limit");
            drop(stream);
            continue;
        };
        stats.accepted.fetch_add(1, Ordering::Relaxed);
        if stream.set_nodelay(true).is_err() {
            debug!("could not set TCP_NODELAY");
        }
        let tls = tls.clone();
        let routes = routes.clone();
        let watcher = graceful.watcher();
        let stats = Arc::clone(&stats);
        tokio::spawn(async move {
            let _permit = permit;
            let handshake = tokio::time::timeout(limits.handshake_timeout, tls.accept(stream)).await;
            let Ok(Ok(stream)) = handshake else {
                stats.handshakes_failed.fetch_add(1, Ordering::Relaxed);
                debug!("TLS handshake failed or timed out");
                return;
            };
            let peer = match PeerCert::from_connection(stream.get_ref().1) {
                None => PeerTls(None),
                Some(Ok(cert)) => PeerTls(Some(cert)),
                Some(Err(error)) => {
                    // The chain verified but the certificate is not one of ours to use: treat it as no certificate at all.
                    stats.unknown_identities.fetch_add(1, Ordering::Relaxed);
                    log_identity_error(error);
                    PeerTls(None)
                }
            };
            let service = TowerToHyperService::new(WithPeer {
                routes,
                addr,
                tls: peer,
            });
            let mut builder = http2::Builder::new(TokioExecutor::new());
            builder
                .timer(TokioTimer::new())
                .keep_alive_interval(Some(limits.keepalive_interval))
                .keep_alive_timeout(limits.keepalive_timeout)
                .max_concurrent_streams(limits.max_concurrent_streams);
            let connection = builder.serve_connection(TokioIo::new(stream), service);
            if let Err(error) = watcher.watch(connection).await {
                debug!(kind = %connection_error_kind(&error), "connection ended with an error");
            }
        });
    }
    drop(listener);
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(limits.drain_timeout) => warn!("gRPC connections still open after the drain period"),
    }
}

fn log_identity_error(error: IdentityError) {
    debug!(%error, "a verified certificate names no known identity");
}

fn connection_error_kind(error: &hyper::Error) -> &'static str {
    // Only a fixed word is logged: the error text of a connection can include peer-controlled bytes.
    if error.is_timeout() {
        "timeout"
    } else if error.is_closed() || error.is_canceled() {
        "closed"
    } else {
        "protocol"
    }
}
