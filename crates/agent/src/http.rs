//! A small plain-HTTP/1.1 client (T2, T12).
//!
//! It talks to two in-cluster or link-local services that speak plain HTTP: the GCE metadata server and the
//! config-server. TLS is not needed there and not offered. Every call has a timeout and a body cap (rule 5), redirects are
//! never followed, and nothing from the request or the response is put into an error.

use std::time::Duration;

use bytes::Bytes;
use http::header::{CONNECTION, CONTENT_LENGTH, HOST, TRANSFER_ENCODING};
use http::{HeaderName, HeaderValue, Method, Request, Uri};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::config::BaseUrl;

/// Why a request failed. None of these carries a URL, header or body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("could not connect")]
    Connect,
    #[error("the request timed out")]
    Timeout,
    #[error("the response body is too large")]
    TooLarge,
    #[error("the peer did not speak HTTP")]
    Protocol,
    #[error("the request is not valid")]
    InvalidRequest,
}

/// A request to send. Build it with [`HttpRequest::get`] or [`HttpRequest::post`].
#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: Method,
    /// Path and query, which must start with `/`. Checked when the request is sent.
    target: String,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
}

impl HttpRequest {
    pub fn get(target: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            target: target.into(),
            headers: Vec::new(),
            body: Bytes::new(),
        }
    }

    pub fn post(target: impl Into<String>, body: impl Into<Bytes>) -> Self {
        Self {
            method: Method::POST,
            target: target.into(),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Add a header. The name and value are validated, so a value with a line break cannot inject a second header.
    /// `Host`, `Connection`, `Content-Length` and `Transfer-Encoding` belong to the client and are refused.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, HttpError> {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| HttpError::InvalidRequest)?;
        if [HOST, CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING].contains(&name) {
            return Err(HttpError::InvalidRequest);
        }
        let value = HeaderValue::from_str(value).map_err(|_| HttpError::InvalidRequest)?;
        self.headers.push((name, value));
        Ok(self)
    }
}

/// What came back.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Bytes,
}

/// A client with a total timeout per request and a cap on the response body.
#[derive(Debug, Clone)]
pub struct HttpClient {
    timeout: Duration,
    max_body: usize,
}

/// Stops the connection task when the request finishes or is cancelled, so no task outlives its caller.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl HttpClient {
    pub fn new(timeout: Duration, max_body: usize) -> Self {
        Self { timeout, max_body }
    }

    /// Send `request` to `base` and read the whole response. Any status is an answer; only transport trouble is an error.
    pub async fn send(&self, base: &BaseUrl, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let target = origin_form(&request.target)?;
        tokio::time::timeout(self.timeout, self.exchange(base, request, target))
            .await
            .map_err(|_| HttpError::Timeout)?
    }

    async fn exchange(
        &self,
        base: &BaseUrl,
        request: HttpRequest,
        target: Uri,
    ) -> Result<HttpResponse, HttpError> {
        let authority = base
            .as_str()
            .parse::<Uri>()
            .ok()
            .and_then(|u| u.authority().cloned())
            .ok_or(HttpError::InvalidRequest)?;
        let port = authority.port_u16().unwrap_or(80);
        let stream = TcpStream::connect((connect_host(authority.host()), port))
            .await
            .map_err(|_| HttpError::Connect)?;
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
            .header(HOST, authority.as_str())
            .header(CONNECTION, "close");
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let outgoing = builder
            .body(Full::new(request.body))
            .map_err(|_| HttpError::InvalidRequest)?;

        let response = sender
            .send_request(outgoing)
            .await
            .map_err(|_| HttpError::Protocol)?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), self.max_body)
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
        Ok(HttpResponse { status, body })
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
    fn ipv6_literals_lose_their_brackets_for_the_resolver() {
        assert_eq!(connect_host("[::1]"), "::1");
        assert_eq!(connect_host("[fd00::10]"), "fd00::10");
        assert_eq!(connect_host("10.0.0.1"), "10.0.0.1");
        assert_eq!(
            connect_host("metadata.google.internal"),
            "metadata.google.internal"
        );
        assert_eq!(connect_host("[broken"), "[broken");
    }

    #[test]
    fn only_origin_form_targets_pass() {
        for ok in ["/", "/a/b", "/a?x=1&y=2", "/computeMetadata/v1/instance"] {
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
}
