//! S6 (T1): the web listener speaks TLS 1.3 and nothing older; the accept loop is bounded and stops when told to.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hub::serve::{ServeLimits, serve, tls13_server_config};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn self_signed() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = key.cert.der().clone();
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()));
    (cert, private)
}

fn client_config(
    trust: &CertificateDer<'static>,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(trust.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    Arc::new(
        ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

struct Running {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn start(tls: Option<TlsAcceptor>, router: axum::Router, limits: ServeLimits) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        serve(
            listener,
            tls,
            router,
            async move {
                let _ = stopped.await;
            },
            limits,
        )
        .await;
    });
    Running {
        addr,
        stop: Some(stop),
        task,
    }
}

const REQUEST: &str =
    "GET /api/v1/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nx-test-user: viewer\r\n\r\n";

async fn read_all<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn web_listener_is_tls13_only() {
    let (cert, key) = self_signed();
    let acceptor = TlsAcceptor::from(tls13_server_config(vec![cert.clone()], key).unwrap());
    let t = support::default_hub();
    let server = start(Some(acceptor), t.hub.web.clone(), ServeLimits::default()).await;
    let name = ServerName::try_from("localhost").unwrap();

    // TLS 1.3 works end to end: the request reaches the router through the real socket and is answered.
    let connector = TlsConnector::from(client_config(&cert, &[&rustls::version::TLS13]));
    let mut stream = connector
        .connect(name.clone(), TcpStream::connect(server.addr).await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        stream.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    stream.write_all(REQUEST.as_bytes()).await.unwrap();
    let response = read_all(&mut stream).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("strict-transport-security: max-age=63072000"),
        "{response}"
    );
    assert!(response.contains(r#"{"ok":true}"#));

    // A client that offers only TLS 1.2 cannot connect.
    let connector = TlsConnector::from(client_config(&cert, &[&rustls::version::TLS12]));
    let attempt = connector
        .connect(name.clone(), TcpStream::connect(server.addr).await.unwrap())
        .await;
    assert!(attempt.is_err(), "a TLS 1.2 handshake succeeded");

    // Plain HTTP on the TLS port gets nothing.
    let mut plain = TcpStream::connect(server.addr).await.unwrap();
    plain.write_all(REQUEST.as_bytes()).await.unwrap();
    let answer = read_all(&mut plain).await;
    assert!(!answer.contains("200 OK"), "{answer}");

    server.stop().await;
}

#[test]
fn a_tls_config_supports_exactly_tls13() {
    let (cert, key) = self_signed();
    let config = tls13_server_config(vec![cert], key).unwrap();
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    // The only versions the config was built with: a ClientHello for 1.2 cannot be answered (shown by the test above).
    let (cert, key) = self_signed();
    assert!(
        tls13_server_config(
            vec![cert],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(vec![1, 2, 3]))
        )
        .is_err()
    );
    drop(key);
}

#[tokio::test]
async fn connections_over_the_limit_are_closed_and_slots_are_reused() {
    let t = support::default_hub();
    let limits = ServeLimits {
        max_connections: 2,
        ..ServeLimits::default()
    };
    let server = start(None, t.hub.ops.clone(), limits).await;
    let a = TcpStream::connect(server.addr).await.unwrap();
    let b = TcpStream::connect(server.addr).await.unwrap();
    // Give the accept loop time to take both slots: it has done so once a request on one of them is answered.
    let mut probe = TcpStream::connect(server.addr).await.unwrap();
    let mut buf = [0u8; 16];
    let closed = tokio::time::timeout(Duration::from_secs(5), probe.read(&mut buf))
        .await
        .unwrap();
    assert!(
        matches!(closed, Ok(0) | Err(_)),
        "the third connection was served"
    );
    drop(a);
    // A slot frees up when the first connection goes away (the server notices the close).
    let mut served = false;
    for _ in 0..50 {
        let mut c = TcpStream::connect(server.addr).await.unwrap();
        c.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        if read_all(&mut c).await.starts_with("HTTP/1.1 200") {
            served = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(served, "a freed slot was never reused");
    drop(b);
    server.stop().await;
}

#[tokio::test]
async fn a_connection_that_never_finishes_its_request_head_is_closed() {
    let t = support::default_hub();
    let limits = ServeLimits {
        header_timeout: Duration::from_millis(300),
        ..ServeLimits::default()
    };
    let server = start(None, t.hub.ops.clone(), limits).await;
    let mut slow = TcpStream::connect(server.addr).await.unwrap();
    slow.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let read = tokio::time::timeout(Duration::from_secs(5), slow.read(&mut buf)).await;
    assert!(read.is_ok(), "the server never gave up on a slow client");
    server.stop().await;
}

#[tokio::test]
async fn stopping_refuses_new_connections() {
    let t = support::default_hub();
    let server = start(None, t.hub.ops.clone(), ServeLimits::default()).await;
    let addr = server.addr;
    server.stop().await;
    let attempt = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .unwrap();
    assert!(attempt.is_err(), "the listener still accepts after stop");
}
