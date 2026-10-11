//! A gRPC server and client over real TCP and real mutual TLS, for the tests of the listener, the join and the connect.
//!
//! The server is the hub's own `serve_grpc` with the hub's own `ServerTls` and a server certificate issued by the CA under
//! test. The client dials TCP, speaks TLS 1.3 with whatever certificate the test hands it, then HTTP/2, and gives the
//! generated client a connection to call through (the agent does the same, in `crates/agent/src/transport/grpc.rs`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http::uri::{Authority, PathAndQuery, Scheme};
use http::{Request, Response, Uri};
use hub_platform::ca::profile::{CertProfile, PeerName};
use hub_platform::listener::{GrpcLimits, ListenerStats, serve_grpc};
use hub_platform::server_identity::issue_server_identity;
use hub_platform::tls::{ClientTls, RotatingServerCert, ServerTls, leaf_certified_key, roots_from_der};
use hyper::body::Incoming;
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rcgen::KeyPair;
use rustls::pki_types::ServerName;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::service::Routes;
use tower::Service;

use super::ca::{TestCa, csr_for};

pub const HUB_NAME: &str = "hub.lanekeeper.test";

/// A running listener.
pub struct TestServer {
    pub addr: SocketAddr,
    pub stats: Arc<ListenerStats>,
    pub server_cert: Arc<RotatingServerCert>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Serve `routes` with a server certificate from `ca`, trusting client certificates from `ca` only.
    pub async fn start(ca: &TestCa, routes: Routes) -> Self {
        Self::start_with(ca, routes, GrpcLimits::default()).await
    }

    pub async fn start_with(ca: &TestCa, routes: Routes, limits: GrpcLimits) -> Self {
        let profile =
            CertProfile::hub_server(&PeerName::parse("hub-0").unwrap(), &[HUB_NAME.to_owned()]).unwrap();
        let (certified, _issued) = issue_server_identity(&ca.issuer, &profile).await.unwrap();
        let server_cert = RotatingServerCert::new(certified);
        let roots = roots_from_der(&[ca.ca_der.clone()]).unwrap();
        let tls = ServerTls::new(roots, Arc::clone(&server_cert), ca.clock.clone()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stats = Arc::new(ListenerStats::default());
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(serve_grpc(
            listener,
            tls.acceptor(),
            routes,
            async move {
                let _ = stopped.await;
            },
            limits,
            Arc::clone(&stats),
        ));
        Self {
            addr,
            stats,
            server_cert,
            stop: Some(stop),
            task: Some(task),
        }
    }

    /// A fresh server certificate from `ca` at the clock's present time, as the renewal job makes one.
    pub async fn reissue_server_cert(&self, ca: &TestCa) {
        let profile =
            CertProfile::hub_server(&PeerName::parse("hub-0").unwrap(), &[HUB_NAME.to_owned()]).unwrap();
        let (certified, _) = issue_server_identity(&ca.issuer, &profile).await.unwrap();
        self.server_cert.replace(certified);
    }

    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// A client identity: an agent-like key and the chain the hub issued for it.
pub struct ClientIdentity {
    pub key: KeyPair,
    pub chain_der: Vec<Vec<u8>>,
}

impl ClientIdentity {
    pub fn certified(&self) -> Arc<rustls::sign::CertifiedKey> {
        leaf_certified_key(&self.key, &self.chain_der).unwrap()
    }
}

/// Issue an agent certificate for `swimlane` from `ca`, for a fresh key.
pub async fn agent_identity(ca: &TestCa, swimlane: &str) -> ClientIdentity {
    use hub_platform::ca::{CertProfile, parse_csr};
    let key = KeyPair::generate().unwrap();
    let csr = parse_csr(&csr_for(&key)).unwrap();
    let issued = ca
        .issuer
        .issue(
            &CertProfile::agent(&domain::SwimlaneId::parse(swimlane).unwrap()),
            &csr,
        )
        .await
        .unwrap();
    ClientIdentity {
        key,
        chain_der: issued.chain_der.iter().map(|c| c.to_vec()).collect(),
    }
}

pub fn client_tls(ca: &TestCa, identity: Option<&ClientIdentity>) -> ClientTls {
    let roots = roots_from_der(&[ca.ca_der.clone()]).unwrap();
    ClientTls::new(roots, identity.map(ClientIdentity::certified), ca.clock.clone()).unwrap()
}

/// An HTTP/2 connection as the service the generated client wants: it adds the scheme and authority to the path-only URIs
/// the generated code uses.
#[derive(Clone)]
pub struct H2 {
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
        if let Ok(uri) = Uri::builder()
            .scheme(self.origin.0.clone())
            .authority(self.origin.1.clone())
            .path_and_query(path)
            .build()
        {
            parts.uri = uri;
        }
        Box::pin(self.send.send_request(Request::from_parts(parts, body)))
    }
}

/// What the TLS handshake settled, for tests that look at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub tls13: bool,
    pub h2: bool,
    pub resumed: bool,
}

/// TCP, TLS with `connector`, then HTTP/2.
pub async fn dial_with(
    addr: SocketAddr,
    connector: tokio_rustls::TlsConnector,
) -> io::Result<(H2, Negotiated)> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(HUB_NAME).unwrap();
    let tls = connector.connect(name, tcp).await?;
    let negotiated = {
        let (_, connection) = tls.get_ref();
        Negotiated {
            tls13: connection.protocol_version() == Some(rustls::ProtocolVersion::TLSv1_3),
            h2: connection.alpn_protocol() == Some(b"h2".as_slice()),
            resumed: matches!(connection.handshake_kind(), Some(rustls::HandshakeKind::Resumed)),
        }
    };
    let (send, connection) = http2::Builder::new(TokioExecutor::new())
        .timer(TokioTimer::new())
        .handshake(TokioIo::new(tls))
        .await
        .map_err(io::Error::other)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok((
        H2 {
            send,
            origin: (
                Scheme::HTTPS,
                format!("{HUB_NAME}:{}", addr.port())
                    .parse::<Authority>()
                    .unwrap(),
            ),
        },
        negotiated,
    ))
}

pub async fn dial(addr: SocketAddr, tls: &ClientTls) -> io::Result<H2> {
    dial_with(addr, tls.connector()).await.map(|(h2, _)| h2)
}

pub fn agent_client(h2: H2) -> proto::pb::agent_client::AgentClient<H2> {
    proto::grpc::agent_client(h2)
}
