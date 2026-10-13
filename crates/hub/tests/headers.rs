//! S12 (T1): the security headers on every class of response, with a fresh CSP nonce per response.
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

use std::fmt::Write as _;

use axum::body::Body;
use http::{Method, Request, StatusCode};
use hub::HubSettings;
use hub::config::{LimitSettings, Rate};
use support::{anonymous, build, default_hub, get, header_of, request, send, text};

/// The headers every web response carries, whatever its class.
const COMMON: [(&str, &str); 6] = [
    ("strict-transport-security", "max-age=63072000; includeSubDomains"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("cache-control", "no-store"),
];

const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn assert_common(label: &str, response: &http::Response<Body>, cache_control: &str) {
    for (name, expected) in COMMON {
        let expected = if name == "cache-control" {
            cache_control
        } else {
            expected
        };
        assert_eq!(header_of(response, name), Some(expected), "{label}: {name}");
    }
    let permissions = header_of(response, "permissions-policy").unwrap_or_default();
    for feature in [
        "camera=()",
        "microphone=()",
        "geolocation=()",
        "payment=()",
        "usb=()",
    ] {
        assert!(
            permissions.contains(feature),
            "{label}: permissions-policy lacks {feature}: {permissions}"
        );
    }
    assert!(
        header_of(response, "x-request-id").is_some_and(|id| id.len() == 16),
        "{label}: x-request-id"
    );
}

fn nonce_of(csp: &str) -> String {
    let at = csp.find("'nonce-").expect("a nonce in the CSP") + "'nonce-".len();
    csp[at..].split('\'').next().unwrap().to_owned()
}

#[tokio::test]
async fn headers_on_every_route_class() {
    let t = default_hub();
    let router = &t.hub.web;

    // HTML: the app, and a client-side route that is the app too.
    for path in ["/", "/swimlanes/sit1"] {
        let response = send(router, get(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_common(&format!("html {path}"), &response, "no-store");
        let csp = header_of(&response, "content-security-policy")
            .unwrap()
            .to_owned();
        assert!(csp.contains("script-src 'self' 'nonce-"), "{csp}");
        assert!(csp.contains("require-trusted-types-for 'script'"), "{csp}");
        assert!(
            csp.contains("trusted-types lanekeeper-docs lanekeeper-monaco"),
            "{csp}"
        );
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    }

    // A hashed static asset: immutable, with the API policy.
    let response = send(router, get("/assets/app-abc123.js")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_common("static asset", &response, "public, max-age=31536000, immutable");
    assert_eq!(header_of(&response, "content-security-policy"), Some(API_CSP));

    // API JSON.
    let response = send(router, get("/api/v1/ping")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_common("api json", &response, "no-store");
    assert_eq!(header_of(&response, "content-security-policy"), Some(API_CSP));

    // Problem JSON: no session.
    let response = send(router, anonymous(Method::GET, "/api/v1/ping", Body::empty())).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_common("problem json", &response, "no-store");
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/problem+json")
    );

    // SSE.
    let response = send(router, get("/api/v1/events")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_common("sse", &response, "no-store");

    // A blob: the handler's immutable header survives, because a blob is addressed by its hash.
    let response = send(router, get(&format!("/api/v1/blobs/{}", "a".repeat(64)))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_common("blob", &response, "private, max-age=31536000, immutable");

    // The webhook.
    let response = send(router, anonymous(Method::POST, "/hooks/github", Body::empty())).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_common("webhook", &response, "no-store");

    // Errors from the router and the handlers: 404, 405, 500.
    let response = send(router, get("/api/v1/does-not-exist")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_common("not found", &response, "no-store");
    let response = send(router, get("/missing.js")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_common("static not found", &response, "no-store");
    let response = send(router, request(Method::DELETE, "/api/v1/ping", Body::empty())).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_common("405", &response, "no-store");
    let response = send(router, get("/api/v1/boom")).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_common("500", &response, "no-store");

    // 413: a body over the cap.
    let big = vec![b'a'; 4 * 1024 * 1024 + 1];
    let response = send(router, request(Method::POST, "/api/v1/raw", Body::from(big))).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_common("413", &response, "no-store");

    // The operations listener: health.
    let response = send(&t.hub.ops, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "cache-control"), Some("no-store"));
    assert_eq!(header_of(&response, "x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn headers_on_429() {
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
    let response = send(&t.hub.web, get("/api/v1/ping")).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_common("429", &response, "no-store");
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/problem+json")
    );
    assert!(header_of(&response, "retry-after").is_some());
}

#[tokio::test]
async fn csp_nonce_is_fresh_per_response_and_matches_the_html() {
    let t = default_hub();
    let mut nonces = std::collections::HashSet::new();
    for _ in 0..50 {
        let response = send(&t.hub.web, get("/")).await;
        let csp = header_of(&response, "content-security-policy")
            .unwrap()
            .to_owned();
        let nonce = nonce_of(&csp);
        assert_eq!(nonce.len(), 32, "128 bits, hex");
        let html = text(response).await;
        assert!(
            !html.contains("__CSP_NONCE__"),
            "the placeholder is replaced everywhere"
        );
        assert_eq!(
            html.matches(&format!("nonce=\"{nonce}\"")).count(),
            2,
            "both tags carry this response's nonce"
        );
        assert!(nonces.insert(nonce), "a nonce was used twice");
    }
    // The style-src nonce is the same value as the script-src one.
    let response = send(&t.hub.web, get("/")).await;
    let csp = header_of(&response, "content-security-policy").unwrap();
    let nonce = nonce_of(csp);
    assert!(
        csp.contains(&format!("style-src 'self' 'nonce-{nonce}'")),
        "{csp}"
    );
}

#[tokio::test]
async fn csp_has_no_unsafe_inline_unsafe_eval_or_third_party_origin() {
    let t = default_hub();
    let response = send(&t.hub.web, get("/")).await;
    let csp = header_of(&response, "content-security-policy")
        .unwrap()
        .to_owned();
    for forbidden in [
        "unsafe-inline",
        "unsafe-eval",
        "strict-dynamic",
        "http:",
        "https:",
        "*",
        "blob:",
        "wasm-unsafe-eval",
    ] {
        assert!(!csp.contains(forbidden), "CSP contains {forbidden}: {csp}");
    }
    // Every source in every directive is 'self', 'none', a nonce, or the data: scheme for images.
    for directive in csp.split(';').map(str::trim).filter(|d| !d.is_empty()) {
        let mut words = directive.split_whitespace();
        let name = words.next().unwrap();
        if matches!(name, "require-trusted-types-for" | "trusted-types") {
            continue;
        }
        for source in words {
            let ok = source == "'self'"
                || source == "'none'"
                || source.starts_with("'nonce-")
                || (name == "img-src" && source == "data:");
            assert!(ok, "{name} allows {source}");
        }
    }
    for required in [
        "default-src 'none'",
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "object-src 'none'",
        "form-action 'self'",
    ] {
        assert!(csp.contains(required), "CSP lacks {required}: {csp}");
    }
}

#[tokio::test]
async fn blob_is_never_rendered_inline() {
    let t = default_hub();
    let response = send(&t.hub.web, get(&format!("/api/v1/blobs/{}", "b".repeat(64)))).await;
    assert_eq!(header_of(&response, "x-content-type-options"), Some("nosniff"));
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header_of(&response, "content-security-policy"), Some(API_CSP));
}

#[tokio::test]
async fn immutable_is_only_allowed_on_hashed_assets_and_blobs() {
    let t = default_hub();
    // A handler that asks for a long cache on an ordinary API route is overruled.
    let response = send(&t.hub.web, get("/api/v1/cache-me")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "cache-control"), Some("no-store"));
    // And an asset outside assets/ is never immutable.
    let response = send(&t.hub.web, get("/favicon.ico")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "cache-control"), Some("no-store"));
}

#[tokio::test]
async fn headers_snapshot_per_class() {
    let t = default_hub();
    let big = vec![b'a'; 4 * 1024 * 1024 + 1];
    let cases: Vec<(&str, Request<Body>)> = vec![
        ("html", get("/")),
        ("static_asset", get("/assets/app-abc123.js")),
        ("api_json", get("/api/v1/ping")),
        (
            "problem_json",
            anonymous(Method::GET, "/api/v1/ping", Body::empty()),
        ),
        ("sse", get("/api/v1/events")),
        ("blob", get(&format!("/api/v1/blobs/{}", "a".repeat(64)))),
        ("webhook", anonymous(Method::POST, "/hooks/github", Body::empty())),
        ("not_found", get("/api/v1/nope")),
        (
            "method_not_allowed",
            request(Method::DELETE, "/api/v1/ping", Body::empty()),
        ),
        (
            "payload_too_large",
            request(Method::POST, "/api/v1/raw", Body::from(big)),
        ),
    ];
    let mut out = String::new();
    for (label, request) in cases {
        let response = send(&t.hub.web, request).await;
        writeln!(out, "## {label} {}", response.status().as_u16()).unwrap();
        let mut lines: Vec<String> = response
            .headers()
            .iter()
            .filter(|(name, _)| !matches!(name.as_str(), "content-length" | "etag"))
            .map(|(name, value)| {
                let mut value = value.to_str().unwrap().to_owned();
                if name == "x-request-id" {
                    value = "<request id>".to_owned();
                } else if name == "content-security-policy" && value.contains("'nonce-") {
                    value = value.replace(&nonce_of(&value), "<nonce>");
                }
                format!("{name}: {value}")
            })
            .collect();
        lines.sort();
        out.push_str(&lines.join("\n"));
        out.push_str("\n\n");
    }
    insta::assert_snapshot!("headers_per_route_class", out);
}
