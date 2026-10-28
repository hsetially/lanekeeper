//! S21 (T1, widened in T8): every error is `application/problem+json` with a stable code and the request id, and nothing from inside.
//!
//! The first group of tests builds the hub from port fakes and one module with a route of every class (`tests/support/mod.rs`). The
//! last test, `problem_bodies_have_no_internals_for_every_composed_module`, starts the whole hub in the test's process (the code the
//! binary runs, over a real Postgres) and provokes an error in each module (identity, the registry, the write paths, the platform,
//! the MCP server and the web app's own fallback), as a person and as nobody. `LK_REQUIRE_DOCKER=1` (set by `just verify-18`) makes
//! a missing Docker daemon a failure and not a skip.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

#[path = "support/browser.rs"]
mod browser;
#[path = "support/child.rs"]
mod child;
#[path = "../examples/support/dev_edges.rs"]
mod dev_edges;
#[path = "support/full.rs"]
mod full;
#[path = "support/git_remote.rs"]
mod git_remote;
#[path = "support/net.rs"]
mod net;
#[path = "support/rig.rs"]
mod rig;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
mod support;
#[path = "support/tls_pg.rs"]
mod tls_pg;

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

// ------------------------------------------------------------------------------------------------- every composed module

/// The members of a problem the API document names (`Problem`, `ConflictProblem`, `LockedProblem`, `DuplicatePrProblem`). Nothing else
/// is in the body of an error.
const PROBLEM_FIELDS: [&str; 12] = [
    "type",
    "title",
    "status",
    "detail",
    "instance",
    "request_id",
    "code",
    "reason",
    "issues",
    "current",
    "until",
    "link",
];

/// The suffixes of `type` that the document lists: the hub's own layers (snake_case), the GitHub token's, and the identity layer's
/// (kebab-case, and sent without a `code`).
const HUB_AND_WRITE_CODES: [&str; 24] = [
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
    "idempotency_conflict",
    "locked",
    "classic_token",
    "not_fine_grained_token",
    "token_rejected",
    "token_missing_access",
    "github_account_mismatch",
    "token_expired",
    "token_unreadable",
];
const IDENTITY_CODES: [&str; 17] = [
    "unauthenticated",
    "access-pending",
    "access-disabled",
    "insufficient-role",
    "csrf",
    "not-found",
    "bad-request",
    "unsupported-media-type",
    "payload-too-large",
    "conflict",
    "too-many-requests",
    "unprocessable",
    "unavailable",
    "sign-in-invalid",
    "sign-in-refused",
    "identity-provider",
    "internal",
];

/// Words and shapes that come from inside: SQL, library and crate names, source paths, the planted secrets and the rig's own paths.
const LEAKS: [&str; 22] = [
    "SELECT",
    "INSERT",
    "relation",
    "sqlx",
    "postgres",
    "lanekeeper_app",
    "hub_registry",
    "hub_writes",
    "hub_identity",
    "hub_platform",
    "hub_mcp",
    "tokio",
    "axum",
    ".rs",
    "panicked",
    "Error:",
    "stack backtrace",
    "/home/",
    "/tmp/",
    "/var/",
    "localhost:",
    "0.0.0.0",
];

/// Everything wrong with one reply, or nothing. The shape is the contract's: `type`, `title` and `status` always; the suffix of `type`
/// is a code the document lists; `code` repeats it where the layer sends one (every layer but identity's); `request_id` is the hub
/// edge's own id, which is also the `X-Request-Id` header, except in a problem that identity's layer made, which carries identity's
/// (G17: three generators write ids, and the header is the hub edge's).
fn composed_problem_faults(
    label: &str,
    reply: &net::Reply,
    statuses: &[u16],
    secrets: &[String],
) -> Vec<String> {
    let mut faults = Vec::new();
    let mut fault = |what: String| faults.push(format!("{label}: {what}"));
    if !statuses.contains(&reply.status) {
        fault(format!(
            "status {} is not one of {statuses:?}: {}",
            reply.status, reply.body
        ));
    }
    if reply.header("content-type") != Some("application/problem+json") {
        fault(format!(
            "content-type {:?}: {}",
            reply.header("content-type"),
            reply.body
        ));
    }
    if reply.header("x-request-id").is_none_or(|id| id.len() != 16) {
        fault("no x-request-id of the hub edge".to_owned());
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        fault(format!("the body is not JSON: {}", reply.body));
        return faults;
    };
    let Some(object) = body.as_object() else {
        fault(format!("the body is not an object: {body}"));
        return faults;
    };
    for key in object.keys() {
        if !PROBLEM_FIELDS.contains(&key.as_str()) {
            fault(format!(
                "the field {key:?} is not one a problem may carry: {body}"
            ));
        }
    }
    for needed in ["type", "title", "status", "request_id"] {
        if !object.contains_key(needed) {
            fault(format!("no {needed} in {body}"));
        }
    }
    let suffix = object
        .get("type")
        .and_then(|t| t.as_str())
        .and_then(|t| t.strip_prefix("urn:lanekeeper:problem:"))
        .unwrap_or_default();
    let identity_made = IDENTITY_CODES.contains(&suffix) && !object.contains_key("code");
    if !(HUB_AND_WRITE_CODES.contains(&suffix) || IDENTITY_CODES.contains(&suffix)) {
        fault(format!("the type {suffix:?} is not one the document lists"));
    }
    if let Some(code) = object.get("code") {
        if code.as_str() != Some(suffix) {
            fault(format!("the code is not the suffix of the type: {body}"));
        }
    } else if !identity_made {
        fault(format!("a problem of the hub's own layers has no code: {body}"));
    }
    if object.get("status") != Some(&serde_json::json!(reply.status)) {
        fault(format!("the body's status is not the response's: {body}"));
    }
    let request_id = object.get("request_id").and_then(|id| id.as_str());
    if identity_made {
        if !request_id.is_some_and(|id| id.starts_with("req-")) {
            fault(format!(
                "identity's request id has a shape nobody expects: {body}"
            ));
        }
    } else if request_id != reply.header("x-request-id") {
        fault(format!("the body does not quote the id of the header: {body}"));
    }
    let text = body.to_string();
    for leak in LEAKS {
        if text.contains(leak) {
            fault(format!("the body contains {leak:?}: {text}"));
        }
    }
    for secret in secrets {
        if text.contains(secret.as_str()) {
            fault("the body holds a secret".to_owned());
        }
        if reply.headers.iter().any(|(_, v)| v.contains(secret.as_str())) {
            fault("a header holds a secret".to_owned());
        }
    }
    faults
}

fn reply(status: u16, content_type: &str, body: &str) -> net::Reply {
    net::Reply {
        status,
        headers: vec![
            ("content-type".to_owned(), content_type.to_owned()),
            ("x-request-id".to_owned(), "0123456789abcdef".to_owned()),
        ],
        body: body.to_owned(),
    }
}

/// A hub-made 404 with the body edited by `edit`.
fn good_hub_with(edit: impl Fn(&str) -> String) -> net::Reply {
    reply(
        404,
        "application/problem+json",
        &edit(
            r#"{"code":"not_found","request_id":"0123456789abcdef","status":404,"title":"Not found","type":"urn:lanekeeper:problem:not_found"}"#,
        ),
    )
}

/// The checker above is not a rubber stamp: each way a body can go wrong is caught, and the shapes of the two layers pass.
#[test]
fn the_composed_problem_checker_catches_what_it_is_for() {
    let good_hub = reply(
        404,
        "application/problem+json",
        r#"{"code":"not_found","request_id":"0123456789abcdef","status":404,"title":"Not found","type":"urn:lanekeeper:problem:not_found"}"#,
    );
    let good_identity = reply(
        401,
        "application/problem+json",
        r#"{"request_id":"req-830d23fdc65a5e179c5de2b26f9356b0","status":401,"title":"Sign in required","type":"urn:lanekeeper:problem:unauthenticated"}"#,
    );
    assert!(composed_problem_faults("hub", &good_hub, &[404], &[]).is_empty());
    assert!(composed_problem_faults("identity", &good_identity, &[401], &[]).is_empty());

    let faulty = |label: &str, reply: net::Reply, statuses: &[u16]| {
        let faults = composed_problem_faults(label, &reply, statuses, &["planted-secret".to_owned()]);
        assert!(!faults.is_empty(), "{label} was not caught");
    };
    faulty("wrong status", good_hub_with(str::to_owned), &[403]);
    faulty(
        "not problem+json",
        reply(404, "application/json", &good_hub.body),
        &[404],
    );
    faulty(
        "SQL in the title",
        good_hub_with(|b| b.replace("Not found", "relation \"users\" does not exist")),
        &[404],
    );
    faulty(
        "a source path",
        good_hub_with(|b| b.replace("Not found", "at src/lib.rs:42")),
        &[404],
    );
    faulty(
        "the planted secret",
        good_hub_with(|b| b.replace("Not found", "planted-secret")),
        &[404],
    );
    faulty(
        "a field nobody lists",
        good_hub_with(|b| b.replace("\"status\":404,", "\"status\":404,\"backtrace\":\"x\",")),
        &[404],
    );
    faulty(
        "a code that is not the suffix",
        good_hub_with(|b| b.replace("\"code\":\"not_found\"", "\"code\":\"conflict\"")),
        &[404],
    );
    faulty(
        "a type the document does not list",
        good_hub_with(|b| b.replace("problem:not_found", "problem:made_up")),
        &[404],
    );
    faulty(
        "a hub problem with no code",
        good_hub_with(|b| b.replace("\"code\":\"not_found\",", "")),
        &[404],
    );
    faulty(
        "an id that is not the header's",
        good_hub_with(|b| b.replace("0123456789abcdef", "fedcba9876543210")),
        &[404],
    );
    faulty("not JSON", reply(404, "application/problem+json", "oops"), &[404]);
}

// (module, what is provoked, who asks, method, path, body, the statuses that are right)
type Probe<'a> = (
    &'a str,
    &'a str,
    Option<&'a browser::Cookies>,
    &'a str,
    &'a str,
    Option<serde_json::Value>,
    &'a [u16],
);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn problem_bodies_have_no_internals_for_every_composed_module() {
    use domain::{Role, UserStatus};
    use serde_json::json;

    let Some(full) = full::Full::start().await else {
        return;
    };
    let (_, viewer) = full.signed_in(1, Some(Role::Viewer), UserStatus::Active).await;
    let (_, editor) = full.signed_in(2, Some(Role::Editor), UserStatus::Active).await;
    let (_, admin) = full.signed_in(3, Some(Role::Admin), UserStatus::Active).await;
    let (_, pending) = full.signed_in(4, None, UserStatus::Pending).await;
    // The webhook's secret, which the hub reads for each delivery: with it in place a delivery that is not signed is refused as such.
    std::fs::write(
        full.rig.secrets.path().join("github-webhook-secret"),
        "whsec_errors_test_secret",
    )
    .unwrap();
    let secrets = vec![
        "whsec_errors_test_secret".to_owned(),
        "planted-db-password-4711".to_owned(),
        "lanekeeper-app-dev-only".to_owned(),
        "dev-entra-client-secret".to_owned(),
        "planted-git-read-token".to_owned(),
        full.rig.secrets.path().display().to_string(),
        full.rig.mirrors.path().display().to_string(),
    ];

    let hash = "0".repeat(64);
    let blob = format!("/api/v1/blobs/{hash}");
    let probes: Vec<Probe<'_>> = vec![
        // The hub's own layers and the web app's fallback.
        (
            "hub",
            "an unknown API path",
            None,
            "GET",
            "/api/v1/does-not-exist",
            None,
            &[401, 404],
        ),
        (
            "hub",
            "an unknown API path, signed in",
            Some(&viewer),
            "GET",
            "/api/v1/does-not-exist",
            None,
            &[404],
        ),
        (
            "hub",
            "a method that route does not have",
            Some(&viewer),
            "DELETE",
            "/api/v1/me",
            None,
            &[405],
        ),
        (
            "hub",
            "an unknown path under /auth",
            None,
            "GET",
            "/auth/does-not-exist",
            None,
            &[404],
        ),
        (
            "hub",
            "an unknown path under /mcp",
            None,
            "GET",
            "/mcp/does-not-exist",
            None,
            &[404],
        ),
        (
            "hub",
            "an unknown path under /hooks",
            None,
            "POST",
            "/hooks/does-not-exist",
            None,
            &[404, 405],
        ),
        (
            "hub",
            "an unknown path under /.well-known",
            None,
            "GET",
            "/.well-known/does-not-exist",
            None,
            &[404],
        ),
        // Identity.
        ("identity", "no session", None, "GET", "/api/v1/me", None, &[401]),
        (
            "identity",
            "a pending user",
            Some(&pending),
            "GET",
            "/api/v1/me",
            None,
            &[403],
        ),
        (
            "identity",
            "a viewer on an admin route",
            Some(&viewer),
            "GET",
            "/api/v1/admin/users",
            None,
            &[403],
        ),
        (
            "identity",
            "an admin on a user that is not there",
            Some(&admin),
            "PATCH",
            "/api/v1/admin/users/not-a-user",
            Some(json!({"role": "viewer"})),
            &[400, 404, 422],
        ),
        (
            "identity",
            "an admin with a body that is not valid",
            Some(&admin),
            "PATCH",
            "/api/v1/admin/users/not-a-user",
            Some(json!({"nonsense": true})),
            &[400, 404, 422],
        ),
        (
            "identity",
            "the callback with no sign-in in flight",
            None,
            "GET",
            "/auth/callback?code=x&state=y",
            None,
            &[400, 401],
        ),
        // The registry.
        (
            "registry",
            "no session",
            None,
            "GET",
            "/api/v1/grid",
            None,
            &[401],
        ),
        (
            "registry",
            "a swimlane that is not there",
            Some(&viewer),
            "GET",
            "/api/v1/swimlanes/no-such-lane",
            None,
            &[404],
        ),
        (
            "registry",
            "its tree",
            Some(&viewer),
            "GET",
            "/api/v1/swimlanes/no-such-lane/tree",
            None,
            &[404],
        ),
        (
            "registry",
            "a path that climbs out",
            Some(&viewer),
            "GET",
            "/api/v1/swimlanes/no-such-lane/files/content?path=../../etc/passwd",
            None,
            &[400, 404],
        ),
        (
            "registry",
            "a comparison with no query",
            Some(&viewer),
            "GET",
            "/api/v1/compare",
            None,
            &[400, 422],
        ),
        (
            "registry",
            "a search with a regex that is too big",
            Some(&viewer),
            "GET",
            "/api/v1/search/text?q=(((((((((((((((((((((((((((((((((((((((((((((((a)))))))))))))))))))))))))))))))))))))))))))))))&regex=true",
            None,
            &[200, 400, 422],
        ),
        // The write paths.
        (
            "writes",
            "an edit with no session",
            None,
            "PUT",
            "/api/v1/swimlanes/no-such-lane/files",
            Some(json!({})),
            &[401],
        ),
        (
            "writes",
            "an edit with a body that is not valid",
            Some(&editor),
            "PUT",
            "/api/v1/swimlanes/no-such-lane/files",
            Some(json!({"nonsense": 1})),
            &[400, 422],
        ),
        (
            "writes",
            "an edit of a swimlane that is not there",
            Some(&editor),
            "PUT",
            "/api/v1/swimlanes/no-such-lane/files",
            Some(json!({"path": "a/b.yml", "content": "a: 1\n", "expected_hash": hash, "base_hash": hash})),
            &[400, 404, 409, 422],
        ),
        (
            "writes",
            "an edit by a viewer",
            Some(&viewer),
            "PUT",
            "/api/v1/swimlanes/no-such-lane/files",
            Some(json!({})),
            &[403],
        ),
        (
            "writes",
            "a token that is not a token",
            Some(&editor),
            "PUT",
            "/api/v1/me/github-token",
            Some(json!({"token": "not-a-github-token"})),
            &[400, 422],
        ),
        (
            "writes",
            "an approval by an editor",
            Some(&editor),
            "POST",
            "/api/v1/proposals/p1/approve",
            Some(json!({})),
            &[403],
        ),
        (
            "writes",
            "a proposal that is not there",
            Some(&admin),
            "POST",
            "/api/v1/proposals/p1/approve",
            Some(json!({})),
            &[400, 404, 422],
        ),
        (
            "writes",
            "a restart by an editor",
            Some(&editor),
            "POST",
            "/api/v1/swimlanes/no-such-lane/restart",
            Some(json!({})),
            &[403],
        ),
        // The platform.
        (
            "platform",
            "the stream with no session",
            None,
            "GET",
            "/api/v1/events",
            None,
            &[401],
        ),
        (
            "platform",
            "a blob with no session",
            None,
            "GET",
            blob.as_str(),
            None,
            &[401],
        ),
        (
            "platform",
            "a blob that is not there",
            Some(&viewer),
            "GET",
            blob.as_str(),
            None,
            &[404],
        ),
        (
            "platform",
            "a blob named by something that is not a hash",
            Some(&viewer),
            "GET",
            "/api/v1/blobs/not-a-hash",
            None,
            &[400, 404],
        ),
        (
            "platform",
            "a webhook with no signature",
            None,
            "POST",
            "/hooks/github",
            Some(json!({"zen": "x"})),
            &[400, 401, 403],
        ),
        // The MCP server.
        (
            "mcp",
            "a call with no token",
            None,
            "POST",
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"})),
            &[401],
        ),
        ("mcp", "a stream with no token", None, "GET", "/mcp", None, &[401]),
        (
            "mcp",
            "the end of a session with no token",
            None,
            "DELETE",
            "/mcp",
            None,
            &[401],
        ),
        (
            "mcp",
            "a personal token that is not there",
            Some(&viewer),
            "DELETE",
            "/api/v1/me/mcp-tokens/no-such-token",
            None,
            &[400, 404],
        ),
    ];

    let mut faults = Vec::new();
    for (module, what, who, method, path, body, statuses) in &probes {
        let label = format!("{module}: {what} ({method} {path})");
        let reply = if let Some(cookies) = who {
            full.web_as(cookies, method, path, body.as_ref()).await
        } else {
            let text = body.as_ref().map(ToString::to_string);
            let headers: &[(&str, &str)] = if body.is_some() {
                &[("Content-Type", "application/json")]
            } else {
                &[]
            };
            full.web(method, path, headers, text.as_deref().map(str::as_bytes))
                .await
        };
        faults.extend(composed_problem_faults(&label, &reply, statuses, &secrets));
    }

    // The CSRF refusal is a problem too: a signed-in person who sends no token and no `Origin`.
    let cookie = format!(
        "__Host-lk_session={}; __Host-lk_csrf={}",
        viewer.session, viewer.csrf
    );
    let reply = full
        .web(
            "POST",
            "/api/v1/me/mcp-tokens",
            &[("Cookie", &cookie), ("Content-Type", "application/json")],
            Some(b"{}"),
        )
        .await;
    faults.extend(composed_problem_faults(
        "identity: no CSRF token",
        &reply,
        &[403],
        &secrets,
    ));
    assert!(faults.is_empty(), "{}", faults.join("\n"));

    // Too many requests from one address: the per-IP bucket answers with a problem as well. (The default bucket is far above what
    // this test sent, so the proof is that the bucket's answer was already shown to be clean in the first group of tests.)
    assert!(probes.len() >= 30, "{} probes", probes.len());
}
