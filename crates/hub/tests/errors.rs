//! S21 (T1): every error is `application/problem+json` with a stable code and the request id, and nothing from inside.
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

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::Body;
use http::{Method, StatusCode};
use hub::HubSettings;
use hub::config::{LimitSettings, Rate};
use support::{anonymous, build, default_hub, get, header_of, json, request, request_as, send};

const KNOWN_CODES: [&str; 15] = [
    "bad_request",
    "unauthorized",
    "forbidden",
    "not_found",
    "method_not_allowed",
    "request_timeout",
    "conflict",
    "payload_too_large",
    "unsupported_media_type",
    "unprocessable",
    "too_many_requests",
    "internal",
    "unavailable",
    "shutting_down",
    "gateway_timeout",
];

/// What the body of an error may hold, and nothing else.
fn assert_clean_problem(
    label: &str,
    response: http::Response<Body>,
    status: StatusCode,
    code: &str,
) -> serde_json::Value {
    assert_eq!(response.status(), status, "{label}");
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/problem+json"),
        "{label}"
    );
    let request_id = header_of(&response, "x-request-id").unwrap().to_owned();
    let body = futures::executor::block_on(async { json(response).await });
    let keys: BTreeSet<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["code", "request_id", "status", "title", "type"]),
        "{label}: {body}"
    );
    assert_eq!(body["code"], code, "{label}");
    assert_eq!(body["status"], status.as_u16(), "{label}");
    assert_eq!(
        body["request_id"],
        request_id.as_str(),
        "{label}: the body quotes the id of the header"
    );
    assert_eq!(body["type"], format!("urn:lanekeeper:problem:{code}"), "{label}");
    assert!(KNOWN_CODES.contains(&code), "{label}: {code}");
    let text = body.to_string();
    for leak in [
        "SELECT", "relation", "secrets", "/srv", ".rs", "panicked", "Error:", "at ",
    ] {
        assert!(
            !text.contains(leak),
            "{label}: the body contains {leak:?}: {text}"
        );
    }
    body
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread")]
async fn errors_are_problem_json_without_internals() {
    let t = default_hub();
    let w = &t.hub.web;

    assert_clean_problem(
        "404 api",
        send(w, get("/api/v1/does-not-exist")).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_clean_problem(
        "404 hooks",
        send(w, get("/hooks/unknown")).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_clean_problem(
        "404 static",
        send(w, get("/missing.js")).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_clean_problem(
        "405",
        send(w, request(Method::DELETE, "/api/v1/ping", Body::empty())).await,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    );
    assert_clean_problem(
        "401 no session",
        send(w, anonymous(Method::GET, "/api/v1/ping", Body::empty())).await,
        StatusCode::UNAUTHORIZED,
        "unauthorized",
    );
    assert_clean_problem(
        "401 unknown user",
        send(
            w,
            request_as(
                "nobody",
                [198, 51, 100, 1],
                Method::GET,
                "/api/v1/ping",
                Body::empty(),
            ),
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "unauthorized",
    );
    for user in ["pending", "disabled", "norole"] {
        assert_clean_problem(
            user,
            send(
                w,
                request_as(
                    user,
                    [198, 51, 100, 1],
                    Method::GET,
                    "/api/v1/ping",
                    Body::empty(),
                ),
            )
            .await,
            StatusCode::FORBIDDEN,
            "forbidden",
        );
    }
    assert_clean_problem(
        "403 role",
        send(w, request(Method::POST, "/api/v1/edit", Body::empty())).await,
        StatusCode::FORBIDDEN,
        "forbidden",
    );
    // A handler that returns a library's text as a 500: the client sees the code, never the text.
    assert_clean_problem(
        "500 with a leaking handler",
        send(w, get("/api/v1/boom")).await,
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
    );
    assert_clean_problem(
        "413",
        send(
            w,
            request(
                Method::POST,
                "/api/v1/raw",
                Body::from(vec![0u8; 4 * 1024 * 1024 + 1]),
            ),
        )
        .await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
    );
    let mut bad_json = request(Method::POST, "/api/v1/echo", Body::from("{not json"));
    bad_json
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    let response = send(w, bad_json).await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an extractor's own text is replaced"
    );
    assert_clean_problem(
        "400 extractor",
        response_again(w).await,
        StatusCode::BAD_REQUEST,
        "bad_request",
    );
    let mut wrong_type = request(Method::POST, "/api/v1/echo", Body::from("{}"));
    wrong_type
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_clean_problem(
        "415",
        send(w, wrong_type).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
    );
}

async fn response_again(w: &axum::Router) -> http::Response<Body> {
    let mut bad_json = request(Method::POST, "/api/v1/echo", Body::from("{not json"));
    bad_json
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    send(w, bad_json).await
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_and_shutdown_errors_are_clean_problems() {
    let settings = HubSettings {
        limits: LimitSettings {
            per_ip: Rate {
                per_second: 0.001,
                burst: 1,
            },
            ..LimitSettings::default()
        },
        ..HubSettings::default()
    };
    let t = build(settings);
    assert_eq!(
        send(&t.hub.web, get("/api/v1/ping")).await.status(),
        StatusCode::OK
    );
    assert_clean_problem(
        "429",
        send(&t.hub.web, get("/api/v1/ping")).await,
        StatusCode::TOO_MANY_REQUESTS,
        "too_many_requests",
    );

    let t = default_hub();
    t.hub.drain.begin_stopping();
    let response = send(&t.hub.web, get("/api/v1/ping")).await;
    assert_eq!(header_of(&response, "connection"), Some("close"));
    assert_clean_problem(
        "503 shutdown",
        response,
        StatusCode::SERVICE_UNAVAILABLE,
        "shutting_down",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handlers_own_problem_keeps_its_fields_and_gains_the_request_id() {
    let t = default_hub();
    let response = send(&t.hub.web, get("/api/v1/conflict")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let id = header_of(&response, "x-request-id").unwrap().to_owned();
    let body = json(response).await;
    assert_eq!(
        body["current"],
        "a".repeat(64),
        "the conflict's current hash survives"
    );
    assert_eq!(body["request_id"], id.as_str());
}

#[tokio::test(start_paused = true)]
async fn timeout_is_a_clean_problem() {
    let t = default_hub();
    let router = t.hub.web.clone();
    let task = tokio::spawn(async move { send(&router, get("/api/v1/slow")).await });
    tokio::time::advance(Duration::from_secs(31)).await;
    let response = task.await.unwrap();
    assert_clean_problem("408", response, StatusCode::REQUEST_TIMEOUT, "request_timeout");
}

#[tokio::test]
async fn request_ids_are_unique_and_sixteen_hex_characters() {
    let t = default_hub();
    let mut seen = BTreeSet::new();
    for _ in 0..200 {
        let response = send(&t.hub.web, get("/api/v1/ping")).await;
        let id = header_of(&response, "x-request-id").unwrap().to_owned();
        assert_eq!(id.len(), 16);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(seen.insert(id));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn problem_bodies_per_code_snapshot() {
    use hub_platform::ApiError;
    let all = [
        ApiError::BadRequest,
        ApiError::Unauthorized,
        ApiError::Forbidden,
        ApiError::NotFound,
        ApiError::MethodNotAllowed,
        ApiError::RequestTimeout,
        ApiError::Conflict,
        ApiError::PayloadTooLarge,
        ApiError::UnsupportedMediaType,
        ApiError::Unprocessable,
        ApiError::TooManyRequests,
        ApiError::Internal,
        ApiError::Unavailable,
        ApiError::ShuttingDown,
        ApiError::GatewayTimeout,
    ];
    let bodies: Vec<serde_json::Value> = all
        .iter()
        .map(|e| serde_json::from_slice(&e.body(Some("0123456789abcdef"))).unwrap())
        .collect();
    let codes: BTreeSet<&str> = all.iter().map(|e| e.code()).collect();
    assert_eq!(
        codes,
        BTreeSet::from(KNOWN_CODES),
        "every variant has a distinct, known code"
    );
    insta::assert_json_snapshot!("problem_bodies", bodies);
}
