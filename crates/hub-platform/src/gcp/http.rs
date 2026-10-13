//! A small HTTPS client for the Google REST APIs (T2, S6, S18).
//!
//! The hub calls three Google methods (a token from the metadata server, Secret Manager `access`, Cloud KMS `asymmetricSign`,
//! `encrypt`, `decrypt` and `getPublicKey`), so it does not carry a Google SDK, which would bring its own TLS and
//! credential stacks. This client is what those calls share:
//!
//! - **Host allowlist (S18).** A request goes only to `https://*.googleapis.com`, or to the metadata server over plain
//!   HTTP (that server speaks nothing else, and the address is link-local). Everything else is refused before a socket is
//!   opened. The check is on the destination, so it holds whatever dialer is plugged in.
//! - **TLS 1.3 only (S6)**, on aws-lc-rs, against the platform's root certificates.
//! - **Bounded (rule 5).** A timeout per attempt, a cap on the response body, no redirects, one connection per request.
//! - **No leaks (S10, S21).** An error is a code. It never carries a URL, a header, a body or a token, and the
//!   `Authorization` header is marked sensitive so that no debug output of a request prints it.

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use http::header::{ACCEPT, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, USER_AGENT};
use http::{HeaderName, HeaderValue, Method, Request, Uri};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use zeroize::Zeroize;

/// The one suffix an HTTPS request may go to.
const GOOGLE_SUFFIX: &str = ".googleapis.com";
/// The hosts that are reached over plain HTTP: the GCE metadata server by name and by its link-local address.
const METADATA_HOSTS: [&str; 2] = ["metadata.google.internal", "169.254.169.254"];
/// How long a TCP connect may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A byte stream the client can speak HTTP or TLS over.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Io for T {}

pub type BoxedIo = Box<dyn Io>;

/// How the client opens the byte stream, before TLS. TCP in production; an in-memory pipe in the tests that need a server
/// they can count, hold and cut, with a paused clock. The destination is checked against the allowlist before the
/// dialer sees it, so a dialer cannot widen where requests may go.
#[async_trait]
pub trait Dialer: Send + Sync + fmt::Debug {
    /// A connected stream to `host:port`. `host` is a name or an address, without brackets.
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo>;
}

/// TCP, for production.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpDialer;

#[async_trait]
impl Dialer for TcpDialer {
    async fn dial(&self, host: &str, port: u16) -> io::Result<BoxedIo> {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        stream.set_nodelay(true)?;
        Ok(Box::new(stream))
    }
}

/// Why an exchange failed. None of these carries a URL, a header, a body or a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the destination is not on the allowlist")]
    HostNotAllowed,
    #[error("the request is not valid")]
    InvalidRequest,
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
}

impl HttpError {
    /// Whether another try may help: the network or the peer failed, not the request.
    #[must_use]
    pub fn is_transient(self) -> bool {
        matches!(self, Self::Connect | Self::Tls | Self::Timeout | Self::Protocol)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

/// Where an API lives: a scheme, a host and a port, and nothing else (no user, path, query or fragment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    scheme: Scheme,
    host: String,
    port: u16,
}

impl Origin {
    /// Parse `https://host[:port]` or `http://host[:port]`. Anything more, or anything less, is refused.
    pub fn parse(text: &str) -> Result<Self, HttpError> {
        let uri: Uri = text.parse().map_err(|_| HttpError::InvalidRequest)?;
        let scheme = match uri.scheme_str() {
            Some("https") => Scheme::Https,
            Some("http") => Scheme::Http,
            _ => return Err(HttpError::InvalidRequest),
        };
        let authority = uri.authority().ok_or(HttpError::InvalidRequest)?;
        // A bare origin: no user, no path beyond the root, no query and no fragment.
        if authority.as_str().contains('@')
            || uri.path() != "/"
            || uri.query().is_some()
            || text.contains(['?', '#'])
        {
            return Err(HttpError::InvalidRequest);
        }
        let host = authority.host();
        if host.is_empty() {
            return Err(HttpError::InvalidRequest);
        }
        let port = authority.port_u16().unwrap_or(match scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        });
        Ok(Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// `https://host` on the default port, for a host the code knows and does not parse.
    #[must_use]
    pub fn https(host: &str) -> Self {
        Self {
            scheme: Scheme::Https,
            host: host.to_owned(),
            port: 443,
        }
    }

    /// `http://host` on the default port, for a host the code knows and does not parse.
    #[must_use]
    pub fn http(host: &str) -> Self {
        Self {
            scheme: Scheme::Http,
            host: host.to_owned(),
            port: 80,
        }
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The value of the `Host` header: the port is left out when it is the scheme's default.
    fn authority(&self) -> String {
        let default = match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Where requests may go (S18).
#[derive(Debug, Clone)]
pub struct HostPolicy {
    kind: PolicyKind,
}

#[derive(Debug, Clone)]
enum PolicyKind {
    Google,
    /// Tests only: these hosts, over either scheme, on any port.
    #[cfg(any(test, feature = "fakes"))]
    Hosts(Vec<String>),
}

fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl HostPolicy {
    /// `https://*.googleapis.com` on port 443, and the metadata server over plain HTTP on port 80.
    #[must_use]
    pub fn google() -> Self {
        Self {
            kind: PolicyKind::Google,
        }
    }

    /// Tests only: allow exactly these hosts, over either scheme and on any port.
    #[cfg(any(test, feature = "fakes"))]
    #[must_use]
    pub fn only_for_tests(hosts: &[&str]) -> Self {
        Self {
            kind: PolicyKind::Hosts(hosts.iter().map(|h| (*h).to_owned()).collect()),
        }
    }

    pub fn check(&self, origin: &Origin) -> Result<(), HttpError> {
        match &self.kind {
            PolicyKind::Google => {
                let allowed = match origin.scheme {
                    Scheme::Https => {
                        origin.port == 443
                            && origin.host.strip_suffix(GOOGLE_SUFFIX).is_some_and(|prefix| {
                                !prefix.is_empty() && prefix.split('.').all(is_dns_label)
                            })
                    }
                    Scheme::Http => origin.port == 80 && METADATA_HOSTS.contains(&origin.host.as_str()),
                };
                if allowed {
                    Ok(())
                } else {
                    Err(HttpError::HostNotAllowed)
                }
            }
            #[cfg(any(test, feature = "fakes"))]
            PolicyKind::Hosts(hosts) => {
                if hosts.contains(&origin.host) {
                    Ok(())
                } else {
                    Err(HttpError::HostNotAllowed)
                }
            }
        }
    }
}

/// The platform's root certificates, for `*.googleapis.com`.
pub fn native_roots() -> Result<RootCertStore, HttpError> {
    let loaded = rustls_native_certs::load_native_certs();
    let mut store = RootCertStore::empty();
    // A single unreadable certificate in the system bundle must not take the hub down, but an empty store must.
    let (added, _ignored) = store.add_parsable_certificates(loaded.certs);
    if added == 0 {
        return Err(HttpError::Tls);
    }
    Ok(store)
}

/// A client configuration: TLS 1.3 and nothing older (S6), the given roots and no client certificate.
pub fn tls_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, HttpError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| HttpError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    // One connection per request, so there is nothing to resume.
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

/// Limits of one exchange.
#[derive(Debug, Clone, Copy)]
pub struct HttpLimits {
    /// Connect, handshake, request and the whole response.
    pub attempt_timeout: Duration,
    /// The response body. The largest legitimate answer is a 64 KiB secret, base64 inside JSON.
    pub max_body: usize,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            attempt_timeout: Duration::from_secs(10),
            max_body: 256 * 1024,
        }
    }
}

/// A request to send. Build it with [`GoogleRequest::get`] or [`GoogleRequest::post_json`].
#[derive(Clone)]
pub struct GoogleRequest {
    method: Method,
    /// Path and query, starting with `/`. Checked when the request is sent.
    target: String,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
    bearer: Option<HeaderValue>,
}

impl fmt::Debug for GoogleRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The target can name a secret and the body can hold one; neither is printed.
        f.debug_struct("GoogleRequest")
            .field("method", &self.method)
            .field("body_len", &self.body.len())
            .field("authenticated", &self.bearer.is_some())
            .finish_non_exhaustive()
    }
}

impl GoogleRequest {
    pub fn get(target: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            target: target.into(),
            headers: Vec::new(),
            body: Bytes::new(),
            bearer: None,
        }
    }

    pub fn post_json(target: impl Into<String>, body: impl Into<Bytes>) -> Self {
        Self {
            method: Method::POST,
            target: target.into(),
            headers: vec![(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            body: body.into(),
            bearer: None,
        }
    }

    /// Add a header. The name and value are validated, so a value with a line break cannot inject a second header.
    /// `Host`, `Connection`, `Content-Length` and `Authorization` belong to the client and are refused.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, HttpError> {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| HttpError::InvalidRequest)?;
        if [HOST, CONNECTION, CONTENT_LENGTH, AUTHORIZATION].contains(&name) {
            return Err(HttpError::InvalidRequest);
        }
        let value = HeaderValue::from_str(value).map_err(|_| HttpError::InvalidRequest)?;
        self.headers.push((name, value));
        Ok(self)
    }

    /// Authenticate with an access token. The header value is marked sensitive.
    pub fn with_bearer(mut self, token: &Secret<String>) -> Result<Self, HttpError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
            .map_err(|_| HttpError::InvalidRequest)?;
        value.set_sensitive(true);
        self.bearer = Some(value);
        Ok(self)
    }
}

/// What came back.
#[derive(Clone)]
pub struct GoogleResponse {
    pub status: u16,
    pub body: Bytes,
}

impl fmt::Debug for GoogleResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The body can be a secret.
        f.debug_struct("GoogleResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl GoogleResponse {
    /// Overwrite the body with zeros before it is freed, when nothing else holds it. For answers that carry a secret
    /// or a key: hyper owns the buffer, so this is best effort, and it is the part of the copy that this crate controls.
    pub fn wipe(self) {
        if let Ok(mut buffer) = self.body.try_into_mut() {
            buffer.as_mut().zeroize();
        }
    }
}

/// Stops the connection task when the exchange finishes or is cancelled, so no task outlives its caller.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One exchange at a time, with every limit; retries and tokens are the caller's (`GoogleApi`).
#[derive(Clone)]
pub struct GoogleHttp {
    policy: HostPolicy,
    tls: Arc<ClientConfig>,
    dialer: Arc<dyn Dialer>,
    limits: HttpLimits,
}

impl fmt::Debug for GoogleHttp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleHttp")
            .field("policy", &self.policy)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl GoogleHttp {
    /// For production: the Google allowlist, TCP, and the platform's roots.
    pub fn production(limits: HttpLimits) -> Result<Self, HttpError> {
        Ok(Self::new(
            HostPolicy::google(),
            tls_config(native_roots()?)?,
            Arc::new(TcpDialer),
            limits,
        ))
    }

    /// With every part chosen by the caller. A dialer and a root store are test seams; the policy is what keeps the
    /// destination fixed, and only [`HostPolicy::google`] exists outside tests.
    pub fn new(
        policy: HostPolicy,
        tls: Arc<ClientConfig>,
        dialer: Arc<dyn Dialer>,
        limits: HttpLimits,
    ) -> Self {
        Self {
            policy,
            tls,
            dialer,
            limits,
        }
    }

    /// Send `request` to `origin` and read the whole response. Any status is an answer; only transport trouble, a
    /// refused destination or an oversized body is an error.
    pub async fn exchange(
        &self,
        origin: &Origin,
        request: GoogleRequest,
    ) -> Result<GoogleResponse, HttpError> {
        self.policy.check(origin)?;
        let target = origin_form(&request.target)?;
        tokio::time::timeout(self.limits.attempt_timeout, self.run(origin, request, target))
            .await
            .map_err(|_| HttpError::Timeout)?
    }

    async fn run(
        &self,
        origin: &Origin,
        request: GoogleRequest,
        target: Uri,
    ) -> Result<GoogleResponse, HttpError> {
        let stream = self
            .dialer
            .dial(connect_host(origin.host()), origin.port())
            .await
            .map_err(|_| HttpError::Connect)?;
        match origin.scheme() {
            Scheme::Http => self.speak(origin, request, target, stream).await,
            Scheme::Https => {
                let name =
                    ServerName::try_from(origin.host().to_owned()).map_err(|_| HttpError::InvalidRequest)?;
                let tls = TlsConnector::from(Arc::clone(&self.tls))
                    .connect(name, stream)
                    .await
                    .map_err(|_| HttpError::Tls)?;
                self.speak(origin, request, target, tls).await
            }
        }
    }

    async fn speak<S: Io>(
        &self,
        origin: &Origin,
        request: GoogleRequest,
        target: Uri,
        stream: S,
    ) -> Result<GoogleResponse, HttpError> {
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream))
                .await
                .map_err(|_| HttpError::Protocol)?;
        let _driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));

        let mut builder = Request::builder()
            .method(request.method)
            .uri(target)
            .header(HOST, origin.authority())
            .header(CONNECTION, "close")
            .header(USER_AGENT, "lanekeeper-hub")
            .header(ACCEPT, "application/json");
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(bearer) = request.bearer {
            builder = builder.header(AUTHORIZATION, bearer);
        }
        let outgoing = builder
            .body(Full::new(request.body))
            .map_err(|_| HttpError::InvalidRequest)?;

        let response = sender
            .send_request(outgoing)
            .await
            .map_err(|_| HttpError::Protocol)?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), self.limits.max_body)
            .collect()
            .await
            .map_err(|e| {
                if e.downcast_ref::<LengthLimitError>().is_some() {
                    HttpError::TooLarge
                } else {
                    HttpError::Protocol
                }
            })?
            .to_bytes();
        Ok(GoogleResponse { status, body })
    }
}

/// The host to connect to: an IPv6 literal is written `[::1]` in a URL and `::1` for the resolver.
fn connect_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// A request target in origin form: a path that starts with `/`, with no scheme, host or spaces.
fn origin_form(target: &str) -> Result<Uri, HttpError> {
    if !target.starts_with('/') || target.starts_with("//") {
        return Err(HttpError::InvalidRequest);
    }
    let uri: Uri = target.parse().map_err(|_| HttpError::InvalidRequest)?;
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(HttpError::InvalidRequest);
    }
    Ok(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_a_scheme_a_host_and_a_port_and_nothing_else() {
        assert!(Origin::parse("https://cloudkms.googleapis.com").is_ok());
        assert_eq!(
            Origin::parse("https://cloudkms.googleapis.com").map(|o| o.port()),
            Ok(443)
        );
        assert_eq!(
            Origin::parse("http://metadata.google.internal").map(|o| o.port()),
            Ok(80)
        );
        assert_eq!(
            Origin::parse("https://Cloudkms.GoogleAPIs.com/").map(|o| o.host().to_owned()),
            Ok("cloudkms.googleapis.com".to_owned())
        );
        for bad in [
            "",
            "cloudkms.googleapis.com",
            "ftp://cloudkms.googleapis.com",
            "https://user@cloudkms.googleapis.com",
            "https://cloudkms.googleapis.com/v1",
            "https://cloudkms.googleapis.com/?x=1",
            "https://cloudkms.googleapis.com#frag",
            "https://",
        ] {
            assert_eq!(Origin::parse(bad), Err(HttpError::InvalidRequest), "{bad:?}");
        }
    }

    #[test]
    fn the_google_policy_allows_googleapis_over_tls_and_the_metadata_server_over_http_only() {
        let policy = HostPolicy::google();
        let allowed = |url: &str| Origin::parse(url).is_ok_and(|o| policy.check(&o).is_ok());
        assert!(allowed("https://secretmanager.googleapis.com"));
        assert!(allowed("https://europe-west2-cloudkms.googleapis.com"));
        assert!(allowed("https://a.b.googleapis.com"));
        assert!(allowed("http://metadata.google.internal"));
        assert!(allowed("http://169.254.169.254"));

        for url in [
            "https://googleapis.com",
            "https://evil.example",
            "https://googleapis.com.evil.example",
            "https://evilgoogleapis.com",
            "https://-.googleapis.com",
            "https://secretmanager.googleapis.com:8443",
            "http://secretmanager.googleapis.com",
            "https://metadata.google.internal",
            "http://metadata.google.internal:8080",
            "http://127.0.0.1",
            "http://localhost",
            "https://169.254.169.254",
        ] {
            assert!(!allowed(url), "{url} must be refused");
        }
    }

    #[test]
    fn only_origin_form_targets_pass() {
        for ok in ["/", "/v1/a/b", "/a?x=1&y=2"] {
            assert!(origin_form(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "a",
            "//evil.example/x",
            "http://evil.example/x",
            "/a b",
            "/a\r\nHost: x",
            "*",
        ] {
            assert_eq!(
                origin_form(bad).unwrap_err(),
                HttpError::InvalidRequest,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_request_cannot_set_the_headers_the_client_owns() {
        for name in ["host", "connection", "content-length", "authorization"] {
            assert_eq!(
                GoogleRequest::get("/").with_header(name, "x").map(|_| ()),
                Err(HttpError::InvalidRequest),
                "{name}"
            );
        }
        assert!(
            GoogleRequest::get("/")
                .with_header("x-goog-request-params", "a=b")
                .is_ok()
        );
        assert_eq!(
            GoogleRequest::get("/")
                .with_header("x-goog-request-params", "a\r\nHost: evil")
                .map(|_| ()),
            Err(HttpError::InvalidRequest)
        );
    }

    #[test]
    fn debug_output_of_a_request_or_response_has_no_target_body_or_token() {
        let token = Secret::new("ya29.super-secret-token".to_owned());
        let request = GoogleRequest::post_json(
            "/v1/projects/p/secrets/db-password:access",
            Bytes::from_static(b"{\"k\":\"v-secret\"}"),
        )
        .with_bearer(&token)
        .unwrap_or_else(|_| GoogleRequest::get("/"));
        let shown = format!("{request:?}");
        for forbidden in ["db-password", "v-secret", "ya29", "super-secret"] {
            assert!(!shown.contains(forbidden), "{forbidden} leaked into {shown}");
        }
        let response = GoogleResponse {
            status: 200,
            body: Bytes::from_static(b"payload-secret"),
        };
        assert!(!format!("{response:?}").contains("payload-secret"));
    }
}
