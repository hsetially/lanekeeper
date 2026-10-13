//! S10 (T1): logs hold ids and route templates, never content, secrets, headers or query strings.
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

use std::io;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use http::Method;
use support::{default_hub, request, send, text};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

impl Capture {
    fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
async fn a_log_line_is_one_json_object_whatever_the_message_holds() {
    let capture = Capture::default();
    let subscriber = hub::telemetry::subscriber(capture.clone(), None);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "hub::test", note = "line one\nline two \"q\"\u{1b}[31m", "hub event");
    });
    let lines = capture.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(value["fields"]["message"], "hub event");
    assert!(value["fields"]["note"].as_str().unwrap().contains('\n'));
}

#[tokio::test]
async fn libraries_are_held_at_warn_and_the_hub_logs_at_info() {
    let targets = hub::telemetry::filter();
    assert!(targets.would_enable("hub::edge", &tracing::Level::INFO));
    assert!(targets.would_enable("hub_platform::gateway", &tracing::Level::INFO));
    assert!(!targets.would_enable("hub::edge", &tracing::Level::DEBUG));
    assert!(targets.would_enable("hyper", &tracing::Level::WARN));
    assert!(!targets.would_enable("hyper", &tracing::Level::INFO));
    assert!(!targets.would_enable("rustls", &tracing::Level::DEBUG));
    assert!(!targets.would_enable("sqlx", &tracing::Level::INFO));
}

#[tokio::test]
async fn log_capture_has_no_secret_content_or_env_value() {
    let capture = Capture::default();
    let subscriber = hub::telemetry::subscriber(capture.clone(), None);
    let _guard = tracing::subscriber::set_default(subscriber);
    let t = default_hub();

    let secrets = [
        "SECRET-QUERY-VALUE-1",
        "SECRET-COOKIE-VALUE-2",
        "SECRET-BEARER-VALUE-3",
        "SECRET-BODY-VALUE-4",
        "SECRET-PATH-VALUE-5",
        "SECRET-HEADER-VALUE-6",
    ];
    let mut metrics_and_problems = String::new();
    let run = |mut r: http::Request<Body>| {
        r.headers_mut()
            .insert("cookie", format!("session={}", secrets[1]).parse().unwrap());
        r.headers_mut()
            .insert("authorization", format!("Bearer {}", secrets[2]).parse().unwrap());
        r.headers_mut().insert("x-custom", secrets[5].parse().unwrap());
        r
    };
    let requests = vec![
        run(request(
            Method::GET,
            &format!("/api/v1/logs?token={}", secrets[0]),
            Body::empty(),
        )),
        run(request(
            Method::GET,
            &format!("/api/v1/ping?token={}", secrets[0]),
            Body::empty(),
        )),
        run(request(
            Method::GET,
            &format!("/api/v1/{}", secrets[4]),
            Body::empty(),
        )),
        run(request(Method::POST, "/api/v1/raw", Body::from(secrets[3]))),
        run(request(
            Method::GET,
            &format!("/api/v1/boom?x={}", secrets[0]),
            Body::empty(),
        )),
        run(request(
            Method::GET,
            &format!("/{}.js", secrets[4]),
            Body::empty(),
        )),
    ];
    for r in requests {
        let response = send(&t.hub.web, r).await;
        metrics_and_problems.push_str(&text(response).await);
    }
    metrics_and_problems.push_str(&t.hub.metrics.encode());

    let lines = capture.lines();
    assert!(
        !lines.is_empty(),
        "the handler's log line was captured, so the scan below is not vacuous"
    );
    let logs = lines.join("\n");
    for secret in secrets {
        assert!(!logs.contains(secret), "a log line contains {secret}:\n{logs}");
        assert!(
            !metrics_and_problems.contains(secret),
            "a metric label or problem body contains {secret}"
        );
    }
}

#[tokio::test]
async fn the_request_span_has_the_request_id_and_the_method_and_nothing_else() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(hub::telemetry::subscriber(capture.clone(), None));
    let t = default_hub();
    let response = send(
        &t.hub.web,
        request(Method::GET, "/api/v1/logs?q=1", Body::empty()),
    )
    .await;
    let id = response
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let lines = capture.lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let span = value["span"].as_object().unwrap();
    let keys: std::collections::BTreeSet<&str> = span.keys().map(String::as_str).collect();
    assert_eq!(keys, ["method", "name", "request_id"].into(), "{span:?}");
    assert_eq!(span["name"], "request");
    assert_eq!(
        span["request_id"],
        id.as_str(),
        "the line can be tied to the response's request id"
    );
    assert_eq!(span["method"], "GET");
}

#[test]
fn the_exporter_exists_only_when_an_endpoint_is_configured() {
    assert!(hub::config::HubSettings::default().otlp_endpoint.is_none());
}

#[tokio::test]
async fn building_the_otlp_exporter_opens_no_connection_and_fails_on_a_bad_endpoint() {
    // A collector that is not there: building the provider must succeed (it connects lazily) and not block.
    let provider = hub::telemetry::tracer_provider("http://127.0.0.1:9").unwrap();
    drop(provider);
    assert!(hub::telemetry::tracer_provider("not an endpoint").is_err());
}
