//! The real client against a local rustls server: the TLS 1.3-only rule, the timeout and the body cap are properties of
//! the wire, so they are proved on a socket and not on a fake.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig, SupportedProtocolVersion};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// 200 with a small body and `Cache-Control: max-age=300`.
    Answer,
    /// Close the first connection before the handshake, then answer.
    DropFirstThenAnswer,
    /// Complete the handshake, read the request, then never answer.
    Stall,
    /// Promise and send 2 MiB.
    Oversize,
    /// 503 once, then 200.
    Unavailable503Once,
}

struct TestServer {
    addr: SocketAddr,
    roots: RootCertStore,
    connections: Arc<AtomicUsize>,
}

fn pki() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = ck.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
    (cert, key)
}

async fn start(versions: &[&'static SupportedProtocolVersion], behaviour: Behaviour) -> TestServer {
    let (cert, key) = pki();
    let mut roots = RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if behaviour == Behaviour::DropFirstThenAnswer && n == 0 {
                    drop(tcp);
                    return;
                }
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => seen.extend_from_slice(&buf[..k]),
                    }
                }
                match behaviour {
                    Behaviour::Stall => tokio::time::sleep(Duration::from_secs(60)).await,
                    Behaviour::Oversize => {
                        let head = "HTTP/1.1 200 OK\r\ncontent-length: 2097152\r\nconnection: close\r\n\r\n";
                        let _ = tls.write_all(head.as_bytes()).await;
                        let chunk = vec![b'a'; 64 * 1024];
                        for _ in 0..32 {
                            if tls.write_all(&chunk).await.is_err() {
                                return;
                            }
                        }
                        let _ = tls.shutdown().await;
                    }
                    Behaviour::Unavailable503Once if n == 0 => {
                        let head = "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                        let _ = tls.write_all(head.as_bytes()).await;
                        let _ = tls.shutdown().await;
                    }
                    _ => {
                        let body = "hello";
                        let head = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncache-control: max-age=300\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = tls.write_all(head.as_bytes()).await;
                        let _ = tls.shutdown().await;
                    }
                }
            });
        }
    });
    TestServer {
        addr,
        roots,
        connections,
    }
}

fn client(server: &TestServer) -> HyperHttpsClient {
    HyperHttpsClient::with_roots(&["localhost"], server.roots.clone())
        .unwrap()
        .connect_to(server.addr)
}

fn url(path: &str) -> Url {
    Url::parse(&format!("https://localhost{path}")).unwrap()
}

#[tokio::test]
async fn real_https_client_speaks_tls13_and_reads_the_answer() {
    let server = start(&[&rustls::version::TLS13], Behaviour::Answer).await;
    let resp = client(&server)
        .send(HttpsRequest::get(url("/x?y=1")))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_ref(), b"hello");
    assert_eq!(resp.cache_control(), Some("max-age=300"));
}

#[tokio::test]
async fn real_https_client_refuses_tls12_and_slow_and_oversize_bodies() {
    // A server that only speaks TLS 1.2 is refused at the handshake (and the failure is not retried).
    let tls12 = start(&[&rustls::version::TLS12], Behaviour::Answer).await;
    let err = client(&tls12)
        .send(HttpsRequest::get(url("/")))
        .await
        .unwrap_err();
    assert_eq!(err, HttpsError::Tls);
    assert_eq!(
        tls12.connections.load(Ordering::SeqCst),
        1,
        "a handshake failure is not retried"
    );

    // A server that never answers hits the timeout, and a POST is attempted once.
    let slow = start(&[&rustls::version::TLS13], Behaviour::Stall).await;
    let quick = client(&slow).with_timeout(Duration::from_millis(150));
    let post = HttpsRequest::post_form(url("/token"), &[("code", "c")]);
    assert_eq!(quick.send(post).await.unwrap_err(), HttpsError::Timeout);
    assert_eq!(
        slow.connections.load(Ordering::SeqCst),
        1,
        "a POST is never retried"
    );

    // A body past the cap is refused, whatever the server promises.
    let big = start(&[&rustls::version::TLS13], Behaviour::Oversize).await;
    let capped = client(&big).with_max_body(1024 * 1024);
    assert_eq!(
        capped.send(HttpsRequest::get(url("/"))).await.unwrap_err(),
        HttpsError::TooLarge
    );
}

#[tokio::test]
async fn real_https_client_retries_a_get_but_not_forever() {
    let flaky = start(&[&rustls::version::TLS13], Behaviour::DropFirstThenAnswer).await;
    let resp = client(&flaky).send(HttpsRequest::get(url("/"))).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(flaky.connections.load(Ordering::SeqCst), 2);

    let busy = start(&[&rustls::version::TLS13], Behaviour::Unavailable503Once).await;
    assert_eq!(
        client(&busy)
            .send(HttpsRequest::get(url("/")))
            .await
            .unwrap()
            .status,
        200
    );

    let dead = start(&[&rustls::version::TLS13], Behaviour::Stall).await;
    let quick = client(&dead).with_timeout(Duration::from_millis(100));
    assert_eq!(
        quick.send(HttpsRequest::get(url("/"))).await.unwrap_err(),
        HttpsError::Timeout
    );
    assert_eq!(
        dead.connections.load(Ordering::SeqCst),
        1 + MAX_RETRIES as usize,
        "one attempt plus the bounded retries"
    );
}

#[tokio::test]
async fn host_outside_allowlist_is_refused() {
    let server = start(&[&rustls::version::TLS13], Behaviour::Answer).await;
    let c = client(&server);
    let other = Url::parse("https://evil.example.com/jwks").unwrap();
    assert_eq!(
        c.send(HttpsRequest::get(other)).await.unwrap_err(),
        HttpsError::HostNotAllowed
    );
    assert_eq!(
        server.connections.load(Ordering::SeqCst),
        0,
        "no connection was made"
    );
    let plain = Url::parse("http://localhost/x").unwrap();
    assert_eq!(
        c.send(HttpsRequest::get(plain)).await.unwrap_err(),
        HttpsError::InvalidRequest
    );
    let creds = Url::parse("https://user:pw@localhost/x").unwrap();
    assert_eq!(
        c.send(HttpsRequest::get(creds)).await.unwrap_err(),
        HttpsError::InvalidRequest
    );
}

#[test]
fn headers_are_validated_and_client_owned_headers_are_refused() {
    let r = HttpsRequest::get(url("/"));
    assert!(
        r.clone()
            .with_header("authorization", "Bearer x\r\nHost: evil")
            .is_err()
    );
    assert!(r.clone().with_header("host", "evil.example.com").is_err());
    assert!(r.clone().with_header("content-length", "5").is_err());
    assert!(r.with_header("authorization", "Bearer x").is_ok());
}

#[test]
fn request_debug_hides_query_headers_and_body() {
    let u = Url::parse("https://localhost/token?code=SECRETQUERY").unwrap();
    let r = HttpsRequest::post_form(u, &[("client_secret", "SECRETBODY")])
        .with_header("authorization", "Bearer SECRETHEADER")
        .unwrap();
    let text = format!("{r:?}");
    for leaked in ["SECRETQUERY", "SECRETBODY", "SECRETHEADER"] {
        assert!(!text.contains(leaked), "{leaked} leaked into Debug");
    }
}

#[test]
fn backoff_is_exponential_and_capped() {
    assert_eq!(backoff_ceiling(0), Duration::from_millis(100));
    assert_eq!(backoff_ceiling(1), Duration::from_millis(200));
    assert_eq!(backoff_ceiling(30), Duration::from_secs(2));
}
