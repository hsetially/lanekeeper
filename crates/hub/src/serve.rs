//! Listeners (T1, S6): the web listener over TLS 1.3, the operations listener over plain HTTP, both bounded.
//!
//! One accept loop serves both. It limits open connections with a semaphore (a connection over the limit is closed at once),
//! gives a TLS handshake and a request head a deadline, stops accepting when told to, and then lets connections in flight
//! finish for a bounded time. The peer address goes into the request so that limits and logs can key by it.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{debug, warn};

use crate::client_ip::PeerAddr;

/// Why a TLS configuration could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TlsError {
    #[error("the certificate chain or key is not usable")]
    Certificate,
}

/// A server configuration that speaks TLS 1.3 and nothing older (S6), on the stack's one crypto provider (aws-lc-rs).
pub fn tls13_server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ServerConfig>, TlsError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| TlsError::Certificate)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|_| TlsError::Certificate)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// How much of a listener one client or one flood may take (rule 5).
#[derive(Debug, Clone, Copy)]
pub struct ServeLimits {
    /// Open connections at once; one more is closed without an answer.
    pub max_connections: usize,
    /// A TLS handshake has this long.
    pub handshake_timeout: Duration,
    /// A request head has this long to arrive.
    pub header_timeout: Duration,
    /// After the stop signal, connections in flight have this long.
    pub drain_timeout: Duration,
}

impl Default for ServeLimits {
    fn default() -> Self {
        Self {
            max_connections: 4096,
            handshake_timeout: Duration::from_secs(10),
            header_timeout: Duration::from_secs(10),
            drain_timeout: Duration::from_secs(25),
        }
    }
}

/// Accept connections on `listener` until `stop` resolves, serve `router` on them (over TLS when `tls` is set), then wait up
/// to `limits.drain_timeout` for connections in flight.
pub async fn serve(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    router: Router,
    stop: impl Future<Output = ()> + Send,
    limits: ServeLimits,
) {
    let permits = Arc::new(Semaphore::new(limits.max_connections));
    let graceful = GracefulShutdown::new();
    let mut stop = std::pin::pin!(stop);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut stop => break,
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                // File-descriptor exhaustion and the like: log the kind, back off briefly instead of spinning.
                warn!(kind = ?error.kind(), "accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            debug!("connection refused: at the connection limit");
            drop(stream);
            continue;
        };
        if stream.set_nodelay(true).is_err() {
            debug!("could not set TCP_NODELAY");
        }
        let router = router.clone();
        let tls = tls.clone();
        let watcher = graceful.watcher();
        tokio::spawn(async move {
            let _permit = permit;
            let service =
                hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
                    let router = router.clone();
                    async move {
                        request.extensions_mut().insert(PeerAddr(peer));
                        router.oneshot(request.map(axum::body::Body::new)).await
                    }
                });
            let mut builder = auto::Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_timeout);
            builder
                .http2()
                .timer(TokioTimer::new())
                .keep_alive_interval(Some(Duration::from_secs(30)))
                .keep_alive_timeout(Duration::from_secs(20))
                .max_concurrent_streams(128);
            if let Some(acceptor) = tls {
                let handshake = tokio::time::timeout(limits.handshake_timeout, acceptor.accept(stream)).await;
                let Ok(Ok(stream)) = handshake else {
                    debug!("TLS handshake failed or timed out");
                    return;
                };
                let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
                if let Err(error) = watcher.watch(connection).await {
                    debug!(error = %error_kind(error.as_ref()), "connection ended with an error");
                }
            } else {
                let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
                if let Err(error) = watcher.watch(connection).await {
                    debug!(error = %error_kind(error.as_ref()), "connection ended with an error");
                }
            }
        });
    }
    drop(listener);
    tokio::select! {
        () = graceful.shutdown() => {}
        () = tokio::time::sleep(limits.drain_timeout) => warn!("connections still open after the drain period"),
    }
}

fn error_kind(error: &(dyn std::error::Error + Send + Sync + 'static)) -> &'static str {
    // Only a fixed word is logged: the error text of a connection can include peer-controlled bytes.
    if error.downcast_ref::<io::Error>().is_some() {
        "io"
    } else {
        "protocol"
    }
}
