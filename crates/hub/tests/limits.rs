//! S11 (T1): body cap, JSON depth, timeout and rate limits, enforced before a handler runs.
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

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use http::{Method, StatusCode};
use hub::HubSettings;
use hub::config::{LimitSettings, Rate};
use support::{build, default_hub, header_of, json, request, request_as, send};

const CAP: usize = 4 * 1024 * 1024;

fn chunked(total: usize) -> Body {
    let chunk = Bytes::from(vec![b'a'; 64 * 1024]);
    let count = total / chunk.len();
    let rest = total % chunk.len();
    let mut chunks: Vec<Result<Bytes, std::io::Error>> = (0..count).map(|_| Ok(chunk.clone())).collect();
    if rest > 0 {
        chunks.push(Ok(Bytes::from(vec![b'a'; rest])));
    }
    Body::from_stream(futures::stream::iter(chunks))
}

#[tokio::test]
async fn oversized_body_is_rejected_413_before_handler() {
    let t = default_hub();
    // Content-Length says it is too big: refused without reading it.
    let mut declared = request(Method::POST, "/api/v1/raw", Body::empty());
    declared
        .headers_mut()
        .insert("content-length", (CAP + 1).to_string().parse().unwrap());
    let response = send(&t.hub.web, declared).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/problem+json")
    );
    let body = json(response).await;
    assert_eq!(body["code"], "payload_too_large");
    assert_eq!(body["status"], 413);

    // The same, with the bytes actually sent.
    let response = send(
        &t.hub.web,
        request(Method::POST, "/api/v1/raw", Body::from(vec![b'a'; CAP + 1])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // No Content-Length at all (chunked): the cap is enforced as the bytes arrive.
    let response = send(&t.hub.web, request(Method::POST, "/api/v1/raw", chunked(CAP + 1))).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE, "chunked");
    assert_eq!(json(response).await["code"], "payload_too_large");

    // A JSON body, which the edge reads whole to check its depth, is refused the same way, chunked too.
    let mut oversized_json = request(Method::POST, "/api/v1/echo", chunked(CAP + 1));
    oversized_json
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    let response = send(&t.hub.web, oversized_json).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE, "chunked json");

    assert_eq!(
        t.counters.raw.load(Ordering::SeqCst),
        0,
        "no handler ran for an oversized body"
    );
    assert_eq!(t.counters.echo.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_accepted() {
    let t = default_hub();
    let response = send(
        &t.hub.web,
        request(Method::POST, "/api/v1/raw", Body::from(vec![b'a'; CAP])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(support::text(response).await, CAP.to_string());
    let response = send(&t.hub.web, request(Method::POST, "/api/v1/raw", chunked(CAP))).await;
    assert_eq!(response.status(), StatusCode::OK, "chunked at the cap");
    assert_eq!(t.counters.raw.load(Ordering::SeqCst), 2);
}

fn nested(depth: usize) -> String {
    format!("{}1{}", "[".repeat(depth), "]".repeat(depth))
}

fn json_request(body: String) -> http::Request<Body> {
    let mut request = request(Method::POST, "/api/v1/echo", Body::from(body));
    request
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    request
}

#[tokio::test]
async fn json_nesting_depth_is_limited() {
    let t = default_hub();
    // serde_json refuses nesting past 128 by itself; the edge's limit is the configured 64, applied before any parser.
    let response = send(&t.hub.web, json_request(nested(64))).await;
    assert_eq!(response.status(), StatusCode::OK, "depth 64 is allowed");
    let response = send(&t.hub.web, json_request(nested(65))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "depth 65 is refused");
    assert_eq!(json(response).await["code"], "bad_request");
    // A very deep body is refused cheaply, without recursion.
    let response = send(&t.hub.web, json_request(nested(1_000_000))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        t.counters.echo.load(Ordering::SeqCst),
        1,
        "only the depth-64 body reached the handler"
    );
}

#[tokio::test]
async fn brackets_inside_strings_do_not_count_as_nesting() {
    let t = default_hub();
    let body = format!(r#"{{"a":"{}","b":"\"{}"}}"#, "[".repeat(500), "{".repeat(500));
    let response = send(&t.hub.web, json_request(body)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test(start_paused = true)]
async fn a_request_that_takes_too_long_gets_a_408_problem() {
    let t = default_hub();
    let router = t.hub.web.clone();
    let task =
        tokio::spawn(async move { send(&router, request(Method::GET, "/api/v1/slow", Body::empty())).await });
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!task.is_finished(), "still within the 30 s limit");
    tokio::time::advance(Duration::from_secs(2)).await;
    let response = task.await.unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(json(response).await["code"], "request_timeout");
}

fn limited(per_ip: Rate, per_user: Rate) -> HubSettings {
    HubSettings {
        limits: LimitSettings {
            per_ip,
            per_user,
            ..LimitSettings::default()
        },
        ..HubSettings::default()
    }
}

const ONE_PER_HOUR: f64 = 1.0 / 3600.0;

#[tokio::test(start_paused = true)]
async fn per_ip_limit_returns_429_with_retry_after_and_refills() {
    let t = build(limited(
        Rate {
            per_second: 1.0,
            burst: 3,
        },
        Rate {
            per_second: 1000.0,
            burst: 1000,
        },
    ));
    let ping = |ip: [u8; 4]| request_as("viewer", ip, Method::GET, "/api/v1/ping", Body::empty());
    for _ in 0..3 {
        assert_eq!(
            send(&t.hub.web, ping([203, 0, 113, 1])).await.status(),
            StatusCode::OK
        );
    }
    let limited = send(&t.hub.web, ping([203, 0, 113, 1])).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(header_of(&limited, "retry-after"), Some("1"));
    assert_eq!(json(limited).await["code"], "too_many_requests");
    // Another address has its own bucket.
    assert_eq!(
        send(&t.hub.web, ping([203, 0, 113, 2])).await.status(),
        StatusCode::OK
    );
    // One second later one token is back.
    tokio::time::advance(Duration::from_millis(1100)).await;
    assert_eq!(
        send(&t.hub.web, ping([203, 0, 113, 1])).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&t.hub.web, ping([203, 0, 113, 1])).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test(start_paused = true)]
async fn per_user_limit_follows_the_user_across_addresses() {
    let t = build(limited(
        Rate {
            per_second: 1000.0,
            burst: 1000,
        },
        Rate {
            per_second: ONE_PER_HOUR,
            burst: 2,
        },
    ));
    let ping = |user: &str, last: u8| {
        request_as(
            user,
            [203, 0, 113, last],
            Method::GET,
            "/api/v1/ping",
            Body::empty(),
        )
    };
    assert_eq!(send(&t.hub.web, ping("viewer", 1)).await.status(), StatusCode::OK);
    assert_eq!(send(&t.hub.web, ping("viewer", 2)).await.status(), StatusCode::OK);
    assert_eq!(
        send(&t.hub.web, ping("viewer", 3)).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(&t.hub.web, ping("viewer2", 3)).await.status(),
        StatusCode::OK,
        "another user is unaffected"
    );
}

#[tokio::test(start_paused = true)]
async fn a_spoofed_forwarded_for_does_not_change_the_bucket() {
    let t = build(limited(
        Rate {
            per_second: ONE_PER_HOUR,
            burst: 2,
        },
        Rate {
            per_second: 1000.0,
            burst: 1000,
        },
    ));
    // The load balancer appends the real address last; the client controls only what is to its left.
    let from_client = |spoof: &str| {
        let mut r = request_as(
            "viewer",
            [10, 0, 0, 5],
            Method::GET,
            "/api/v1/ping",
            Body::empty(),
        );
        r.headers_mut().insert(
            "x-forwarded-for",
            format!("{spoof}, 203.0.113.50").parse().unwrap(),
        );
        r
    };
    assert_eq!(
        send(&t.hub.web, from_client("1.1.1.1")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&t.hub.web, from_client("2.2.2.2")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&t.hub.web, from_client("3.3.3.3")).await.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "rotating the spoofed entry does not get a fresh bucket"
    );
}

#[tokio::test]
async fn the_bucket_tables_are_bounded() {
    let settings = HubSettings {
        limits: LimitSettings {
            bucket_capacity: 100,
            ..LimitSettings::default()
        },
        ..HubSettings::default()
    };
    let test_hub = build(settings);
    for i in 0..2000u32 {
        let ip = (0x0a00_0000 + i).to_be_bytes();
        let r = request_as("viewer", ip, Method::GET, "/api/v1/ping", Body::empty());
        send(&test_hub.hub.web, r).await;
    }
    test_hub.hub.limits.sync();
    assert!(
        test_hub.hub.limits.ip_buckets() <= 100,
        "{} buckets",
        test_hub.hub.limits.ip_buckets()
    );
}
