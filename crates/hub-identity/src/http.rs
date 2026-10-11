//! The one outbound HTTPS client (decision B2): discovery, JWKS, the token endpoint, Google's certificate URL and, in
//! later tasks, the checkpoint bucket and the metadata server.
//!
//! - TLS 1.3 only, on the stack's one provider (aws-lc-rs). Roots are the system roots; `webpki-roots` is refused by `deny.toml`.
//! - The host must be on an allowlist built from configuration, so a poisoned discovery document cannot send the hub to
//!   another host. Redirects are never followed.
//! - Every call has a timeout and a response size cap (rule 5). Concurrency is bounded by a semaphore. Only `GET` is retried
//!   (at most twice, full jitter): a `POST` carries a single-use authorization code.
//! - Nothing from the request or the response is put into an error (S10).

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http::header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, TRANSFER_ENCODING};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use url::Url;

/// The longest response body read. Longer is [`HttpsError::TooLarge`].
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// The time one attempt may take, connect to last byte.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Retries after the first attempt of a `GET`.
pub const MAX_RETRIES: u32 = 2;
/// Requests in flight at once.
const MAX_IN_FLIGHT: usize = 32;

/// Why a request failed. None of these carries a URL, header or body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpsError {
    #[error("could not connect")]
    Connect,
    #[error("the TLS handshake failed")]
    Tls,
    #[error("the request timed out")]
    Timeout,
    #[error("the response body is too large")]
    TooLarge,
    #[error("the peer did not speak HTTP")]
    Protocol,
    #[error("the request is not valid")]
    InvalidRequest,
    #[error("the host is not on the allowlist")]
    HostNotAllowed,
}

/// A request to send. Build it with [`HttpsRequest::get`] or [`HttpsRequest::post_form`].
#[derive(Clone)]
pub struct HttpsRequest {
    method: Method,
    url: Url,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
}

impl HttpsRequest {
    pub fn get(url: Url) -> Self {
        Self {
            method: Method::GET,
            url,
            headers: Vec::new(),
            body: Bytes::new(),
        }
    }

    /// A `POST` with an `application/x-www-form-urlencoded` body. The values may be secrets: the body is never printed.
    pub fn post_form(url: Url, fields: &[(&str, &str)]) -> Self {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in fields {
            ser.append_pair(k, v);
        }
        Self {
            method: Method::POST,
            url,
            headers: vec![(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            )],
            body: Bytes::from(ser.finish()),
        }
    }

    /// A `POST` with a raw body and a content type.
    pub fn post(url: Url, content_type: &'static str, body: Bytes) -> Self {
        Self {
            method: Method::POST,
            url,
            headers: vec![(CONTENT_TYPE, HeaderValue::from_static(content_type))],
            body,
        }
    }

    /// Add a header. Names and values are validated, so a value with a line break cannot inject a second header.
    /// `Host`, `Connection`, `Content-Length` and `Transfer-Encoding` belong to the client and are refused.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, HttpsError> {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| HttpsError::InvalidRequest)?;
        if [HOST, CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING].contains(&name) {
            return Err(HttpsError::InvalidRequest);
        }
        let mut value = HeaderValue::from_str(value).map_err(|_| HttpsError::InvalidRequest)?;
        value.set_sensitive(true);
        self.headers.push((name, value));
        Ok(self)
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn body(&self) -> &Bytes {
        &self.body
    }

    pub fn header(&self, name: &str) -> Option<&HeaderValue> {
        self.headers
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }
}

impl fmt::Debug for HttpsRequest {
    /// Method, host and path only: the query, the headers and the body may hold a secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsRequest")
            .field("method", &self.method.as_str())
            .field("host", &self.url.host_str())
            .field("path", &self.url.path())
            .finish_non_exhaustive()
    }
}

/// What came back.
#[derive(Clone)]
pub struct HttpsResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl fmt::Debug for HttpsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl HttpsResponse {
    /// The `Cache-Control` header, when present and text.
    pub fn cache_control(&self) -> Option<&str> {
        self.headers
            .get(http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
    }
}

/// The outbound HTTPS port of this crate. Production uses [`HyperHttpsClient`]; tests use a fake that serves canned answers.
/// Any status is an answer; only transport trouble is an error.
#[async_trait]
pub trait HttpsClient: Send + Sync + 'static {
    async fn send(&self, request: HttpsRequest) -> Result<HttpsResponse, HttpsError>;
}

/// Stops the connection task when the request finishes or is cancelled, so no task outlives its caller.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// hyper's HTTP/1 client over `tokio-rustls`, TLS 1.3 only.
pub struct HyperHttpsClient {
    allowed_hosts: Arc<[String]>,
    tls: TlsConnector,
    permits: Arc<Semaphore>,
    timeout: Duration,
    max_body: usize,
    /// Connect here instead of resolving the URL's host. For tests that run a local TLS server.
    connect_to: Option<SocketAddr>,
}

impl fmt::Debug for HyperHttpsClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HyperHttpsClient")
            .field("allowed_hosts", &self.allowed_hosts)
            .finish_non_exhaustive()
    }
}

impl HyperHttpsClient {
    /// A client that trusts the system roots and talks only to `allowed_hosts`.
    pub fn new(allowed_hosts: &[&str]) -> Result<Self, HttpsError> {
        let loaded = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let (added, _ignored) = roots.add_parsable_certificates(loaded.certs);
        if added == 0 {
            return Err(HttpsError::Tls);
        }
        Self::with_roots(allowed_hosts, roots)
    }

    /// A client that trusts exactly `roots`. Used by [`HyperHttpsClient::new`] and by tests with a private CA.
    pub fn with_roots(allowed_hosts: &[&str], roots: RootCertStore) -> Result<Self, HttpsError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|_| HttpsError::Tls)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            allowed_hosts: allowed_hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
            tls: TlsConnector::from(Arc::new(config)),
            permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            timeout: REQUEST_TIMEOUT,
            max_body: MAX_RESPONSE_BYTES,
            connect_to: None,
        })
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_body(mut self, max_body: usize) -> Self {
        self.max_body = max_body;
        self
    }

    /// Test hook: dial `addr` whatever the URL says. The host is still checked against the allowlist and used for SNI.
    #[doc(hidden)]
    #[must_use]
    pub fn connect_to(mut self, addr: SocketAddr) -> Self {
        self.connect_to = Some(addr);
        self
    }

    /// Host and port of an acceptable URL: `https`, no credentials, an allowlisted host, port 443 unless the URL says
    /// otherwise.
    fn target(&self, url: &Url) -> Result<(String, u16), HttpsError> {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(HttpsError::InvalidRequest);
        }
        let host = url.host_str().ok_or(HttpsError::InvalidRequest)?;
        if !self.allowed_hosts.iter().any(|h| h == host) {
            return Err(HttpsError::HostNotAllowed);
        }
        Ok((host.to_owned(), url.port().unwrap_or(443)))
    }

    async fn once(&self, request: &HttpsRequest, host: &str, port: u16) -> Result<HttpsResponse, HttpsError> {
        let tcp = match self.connect_to {
            Some(addr) => TcpStream::connect(addr).await,
            None => TcpStream::connect((host, port)).await,
        }
        .map_err(|_| HttpsError::Connect)?;
        let name = ServerName::try_from(host.to_owned()).map_err(|_| HttpsError::InvalidRequest)?;
        let tls = self.tls.connect(name, tcp).await.map_err(|e| {
            use std::io::ErrorKind::{
                BrokenPipe, ConnectionAborted, ConnectionReset, TimedOut, UnexpectedEof,
            };
            // A peer that hangs up is a connection problem and may be retried; an alert, a bad certificate or a
            // protocol version it cannot speak is a TLS problem and is not.
            if matches!(
                e.kind(),
                UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe | TimedOut
            ) {
                HttpsError::Connect
            } else {
                HttpsError::Tls
            }
        })?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tls))
                .await
                .map_err(|_| HttpsError::Protocol)?;
        let _driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));

        let authority = match request.url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_owned(),
        };
        let target = match request.url.query() {
            Some(q) => format!("{}?{q}", request.url.path()),
            None => request.url.path().to_owned(),
        };
        let mut builder = Request::builder()
            .method(request.method.clone())
            .uri(target)
            .header(HOST, authority)
            .header(CONNECTION, "close")
            .header(http::header::USER_AGENT, "lanekeeper-hub");
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let hyper_request = builder
            .body(Full::new(request.body.clone()))
            .map_err(|_| HttpsError::InvalidRequest)?;

        let response = sender
            .send_request(hyper_request)
            .await
            .map_err(|_| HttpsError::Protocol)?;
        let (parts, body) = response.into_parts();
        let body = Limited::new(body, self.max_body)
            .collect()
            .await
            .map_err(|e| {
                if e.downcast_ref::<LengthLimitError>().is_some() {
                    HttpsError::TooLarge
                } else {
                    HttpsError::Protocol
                }
            })?
            .to_bytes();
        Ok(HttpsResponse {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        })
    }
}

/// Full jitter: a uniform delay in `0..=min(2 s, 100 ms * 2^attempt)`.
pub(crate) fn backoff_ceiling(attempt: u32) -> Duration {
    Duration::from_millis(100u64.saturating_mul(1u64 << attempt.min(10)).min(2_000))
}

fn retryable(e: HttpsError) -> bool {
    matches!(
        e,
        HttpsError::Connect | HttpsError::Timeout | HttpsError::Protocol
    )
}

#[async_trait]
impl HttpsClient for HyperHttpsClient {
    async fn send(&self, request: HttpsRequest) -> Result<HttpsResponse, HttpsError> {
        let (host, port) = self.target(&request.url)?;
        let _permit = tokio::time::timeout(self.timeout, self.permits.acquire())
            .await
            .map_err(|_| HttpsError::Timeout)?
            .map_err(|_| HttpsError::Connect)?;
        let attempts = if request.method == Method::GET {
            1 + MAX_RETRIES
        } else {
            1
        };
        let mut last = Err(HttpsError::Connect);
        for attempt in 0..attempts {
            if attempt > 0 {
                let ceiling = backoff_ceiling(attempt - 1);
                let ceiling_ms = u64::try_from(ceiling.as_millis()).unwrap_or(2_000);
                tokio::time::sleep(Duration::from_millis(fastrand::u64(0..=ceiling_ms))).await;
            }
            let outcome = tokio::time::timeout(self.timeout, self.once(&request, &host, port))
                .await
                .unwrap_or(Err(HttpsError::Timeout));
            let retry = match &outcome {
                Ok(resp) => matches!(resp.status, 502..=504),
                Err(e) => retryable(*e),
            };
            last = outcome;
            if !retry {
                break;
            }
        }
        last
    }
}

#[cfg(test)]
mod tests;
