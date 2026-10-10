//! The plain-HTTP client that talks to the metadata server and the config-server (T2, T12).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use agent::config::BaseUrl;
use agent::http::{HttpClient, HttpError, HttpRequest};
use support::raw_server::{RawServer, Reply, unreachable_url};

fn client() -> HttpClient {
    HttpClient::new(Duration::from_secs(5), 1024)
}

fn base(url: &str) -> BaseUrl {
    BaseUrl::parse(url, "http").unwrap()
}

#[tokio::test]
async fn get_returns_the_status_and_body() {
    let server = RawServer::start(|_| Reply::ok("hello")).await;
    let response = client()
        .send(&base(&server.url()), HttpRequest::get("/a/b?x=1"))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(&response.body[..], b"hello");
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, "/a/b?x=1");
}

#[tokio::test]
async fn sends_host_connection_close_and_the_callers_headers() {
    let server = RawServer::start(|_| Reply::ok("")).await;
    let request = HttpRequest::get("/")
        .with_header("Metadata-Flavor", "Google")
        .unwrap();
    client().send(&base(&server.url()), request).await.unwrap();
    let seen = &server.requests()[0];
    assert_eq!(
        seen.header("host"),
        Some(server.url().trim_start_matches("http://"))
    );
    assert_eq!(seen.header("connection"), Some("close"));
    assert_eq!(seen.header("metadata-flavor"), Some("Google"));
}

#[tokio::test]
async fn post_sends_the_body_with_its_length() {
    let server = RawServer::start(|_| Reply::ok("")).await;
    let request = HttpRequest::post("/update-resources", b"a=1&b=2".to_vec())
        .with_header("Content-Type", "application/x-www-form-urlencoded")
        .unwrap();
    client().send(&base(&server.url()), request).await.unwrap();
    let seen = &server.requests()[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.header("content-length"), Some("7"));
    assert_eq!(seen.body, b"a=1&b=2");
}

#[tokio::test]
async fn an_error_status_is_an_answer_not_an_error() {
    let server = RawServer::start(|_| Reply::status(503, "busy")).await;
    let response = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap();
    assert_eq!(response.status, 503);
    assert_eq!(&response.body[..], b"busy");
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = RawServer::start(|_| Reply::Http {
        status: 302,
        headers: vec![("Location".into(), "http://127.0.0.1:1/elsewhere".into())],
        body: Vec::new(),
    })
    .await;
    let response = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_when_its_length_is_declared() {
    let server = RawServer::start(|_| Reply::ok(vec![b'x'; 2048])).await;
    let error = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::TooLarge), "{error:?}");
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_when_no_length_is_declared() {
    // No Content-Length: the body runs until the server closes the connection.
    let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
    raw.extend(std::iter::repeat_n(b'x', 4096));
    let server = RawServer::start(move |_| Reply::Raw(raw.clone())).await;
    let error = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::TooLarge), "{error:?}");
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_accepted() {
    let server = RawServer::start(|_| Reply::ok(vec![b'x'; 1024])).await;
    let response = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap();
    assert_eq!(response.body.len(), 1024);
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let server = RawServer::start(|_| Reply::Hang).await;
    let patient = HttpClient::new(Duration::from_millis(200), 1024);
    let error = patient
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::Timeout), "{error:?}");
}

#[tokio::test]
async fn a_refused_connection_is_a_connect_error() {
    let url = unreachable_url().await;
    let error = client()
        .send(&base(&url), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::Connect), "{error:?}");
}

#[tokio::test]
async fn a_connection_closed_without_an_answer_is_a_protocol_error() {
    let server = RawServer::start(|_| Reply::Close).await;
    let error = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::Protocol), "{error:?}");
}

#[tokio::test]
async fn garbage_instead_of_a_response_is_a_protocol_error() {
    let server = RawServer::start(|_| Reply::Raw(b"\x00\x01 not http at all\r\n\r\n".to_vec())).await;
    let error = client()
        .send(&base(&server.url()), HttpRequest::get("/"))
        .await
        .unwrap_err();
    assert!(matches!(error, HttpError::Protocol), "{error:?}");
}

#[test]
fn headers_and_targets_with_control_characters_are_refused() {
    assert!(
        HttpRequest::get("/")
            .with_header("X-A", "a\r\nInjected: 1")
            .is_err()
    );
    assert!(HttpRequest::get("/").with_header("bad name", "v").is_err());
    // A request target is checked when the request is sent.
    let bad = HttpRequest::get("/a b");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(client().send(&base("http://127.0.0.1:9"), bad))
        .unwrap_err();
    assert!(matches!(error, HttpError::InvalidRequest), "{error:?}");
}

#[test]
fn errors_say_what_failed_without_echoing_data() {
    for (error, word) in [
        (HttpError::Connect, "connect"),
        (HttpError::Timeout, "timed out"),
        (HttpError::TooLarge, "too large"),
        (HttpError::Protocol, "HTTP"),
        (HttpError::InvalidRequest, "request"),
    ] {
        assert!(error.to_string().contains(word), "{error}");
    }
}
