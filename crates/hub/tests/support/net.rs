//! A small HTTP client for the process tests: plain HTTP for the operations listener, TLS 1.3 for the web listener.
//!
//! One request per connection, `Connection: close`, read to the end. Enough to see a status line, the headers and the body.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn parse(raw: &[u8]) -> Reply {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_owned(),
    }
}

fn request(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAccept: */*\r\n\r\n")
}

/// `GET path` on a connection that is already open, and read until the server closes it.
pub async fn exchange<S>(mut stream: S, path: &str) -> Reply
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(request(path).as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    // A server that closes without a close_notify ends the read with an error after the data; the data is what counts.
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw)).await;
    parse(&raw)
}

/// `GET path` over plain HTTP.
pub async fn http_get(addr: SocketAddr, path: &str) -> Reply {
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    exchange(stream, path).await
}

/// A client that trusts `ca` and speaks TLS 1.3 only (or TLS 1.2 only, to prove the server refuses it). It offers `http/1.1`,
/// which is what [`exchange`] speaks.
pub fn client_config(
    ca: &CertificateDer<'static>,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Arc<ClientConfig> {
    client_config_with(ca, versions, b"http/1.1")
}

/// Like [`client_config`], offering `alpn` (the gRPC listeners speak `h2` and nothing else).
pub fn client_config_with(
    ca: &CertificateDer<'static>,
    versions: &[&'static rustls::SupportedProtocolVersion],
    alpn: &[u8],
) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(ca.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![alpn.to_vec()];
    Arc::new(config)
}

/// Open a TLS connection to `addr`, naming `localhost`.
pub async fn tls_connect(
    addr: SocketAddr,
    config: Arc<ClientConfig>,
) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let stream = TcpStream::connect(addr).await?;
    TlsConnector::from(config)
        .connect(ServerName::try_from("localhost").unwrap(), stream)
        .await
}

/// A request with any method, extra headers and a body, over TLS 1.3 trusting `ca` (T5: the write routes). One request per
/// connection, `Connection: close`, read to the end.
pub async fn https_request(
    addr: SocketAddr,
    ca: &CertificateDer<'static>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Reply {
    let config = client_config(ca, &[&rustls::version::TLS13]);
    let mut stream = tls_connect(addr, config).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    // `Accept: */*` unless the caller names its own: a client of the MCP endpoint must say `application/json` and
    // `text/event-stream` in so many words, and two `Accept` headers are read as the first of them.
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("accept"))
    {
        head.push_str("Accept: */*\r\n");
    }
    for (name, value) in headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if let Some(body) = body {
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    if let Some(body) = body {
        stream.write_all(body).await.unwrap();
    }
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw)).await;
    parse(&raw)
}

/// `GET path` over TLS 1.3, trusting `ca`.
pub async fn https_get(addr: SocketAddr, ca: &CertificateDer<'static>, path: &str) -> Reply {
    let config = client_config(ca, &[&rustls::version::TLS13]);
    let stream = tls_connect(addr, config).await.unwrap();
    exchange(stream, path).await
}
