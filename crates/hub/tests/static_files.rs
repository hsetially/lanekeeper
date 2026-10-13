//! T1, S12, rule 10: the built web app is served with the response's nonce, and its paths are untrusted.
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

use axum::body::Body;
use http::{Method, StatusCode};
use support::{default_hub, get, header_of, request, send, text};

#[tokio::test]
async fn index_carries_the_nonce_of_its_own_response_on_every_tag() {
    let t = default_hub();
    let response = send(&t.hub.web, get("/")).await;
    assert_eq!(
        header_of(&response, "content-type"),
        Some("text/html; charset=utf-8")
    );
    let csp = header_of(&response, "content-security-policy")
        .unwrap()
        .to_owned();
    let html = text(response).await;
    assert!(html.contains("<script type=\"module\" nonce=\""));
    assert!(!html.contains("__CSP_NONCE__"));
    assert!(csp.contains(&format!(
        "'nonce-{}'",
        html.split("nonce=\"").nth(1).unwrap().split('"').next().unwrap()
    )));
}

#[tokio::test]
async fn client_side_routes_get_the_app_but_missing_files_and_api_paths_do_not() {
    let t = default_hub();
    let route = send(&t.hub.web, get("/swimlanes/sit1/files")).await;
    assert_eq!(route.status(), StatusCode::OK);
    assert!(text(route).await.contains("<div id=\"root\">"));
    for path in [
        "/missing.js",
        "/assets/missing-abc.js",
        "/api/v1/nothing",
        "/api",
        "/hooks/other",
    ] {
        let response = send(&t.hub.web, get(path)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            header_of(&response, "content-type"),
            Some("application/problem+json"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn assets_have_their_type_and_head_has_no_body() {
    let t = default_hub();
    let js = send(&t.hub.web, get("/assets/app-abc123.js")).await;
    assert_eq!(
        header_of(&js, "content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(text(js).await, "console.log(1)");
    let css = send(&t.hub.web, get("/assets/app-abc123.css")).await;
    assert_eq!(header_of(&css, "content-type"), Some("text/css; charset=utf-8"));
    let head = send(
        &t.hub.web,
        request(Method::HEAD, "/assets/app-abc123.js", Body::empty()),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(header_of(&head, "content-length"), Some("14"));
    assert_eq!(text(head).await, "");
}

#[tokio::test]
async fn only_get_and_head_are_served() {
    let t = default_hub();
    let response = send(&t.hub.web, request(Method::POST, "/", Body::empty())).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header_of(&response, "allow"), Some("GET, HEAD"));
}

#[tokio::test]
async fn path_traversal_in_every_spelling_is_a_404() {
    let t = default_hub();
    // A secret file next to the web root, which no path may reach.
    let outside = t.dist.path().parent().unwrap().join("lk-outside-secret.txt");
    std::fs::write(&outside, "TOP SECRET").unwrap();
    for path in [
        "/../lk-outside-secret.txt",
        "/assets/../../lk-outside-secret.txt",
        "/%2e%2e/lk-outside-secret.txt",
        "/assets/%2e%2e/%2e%2e/lk-outside-secret.txt",
        "/..%2flk-outside-secret.txt",
        "/assets/..%5c..%5clk-outside-secret.txt",
        "//lk-outside-secret.txt",
        "/assets//app-abc123.js",
        "/assets/",
        "/./index.html/",
        "/app-abc123.js%00.html",
        "/%c0%ae%c0%ae/lk-outside-secret.txt",
    ] {
        let response = send(&t.hub.web, get(path)).await;
        let status = response.status();
        let body = text(response).await;
        assert!(!body.contains("TOP SECRET"), "{path} leaked a file");
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "{path}: {status}"
        );
    }
    std::fs::remove_file(outside).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_inside_the_web_root_is_never_followed() {
    let t = default_hub();
    let secret_dir = tempfile::tempdir().unwrap();
    std::fs::write(secret_dir.path().join("secret.js"), "TOP SECRET").unwrap();
    std::os::unix::fs::symlink(
        secret_dir.path().join("secret.js"),
        t.dist.path().join("assets/link.js"),
    )
    .unwrap();
    std::os::unix::fs::symlink(secret_dir.path(), t.dist.path().join("assets/linkdir")).unwrap();
    for path in ["/assets/link.js", "/assets/linkdir/secret.js"] {
        let response = send(&t.hub.web, get(path)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!text(response).await.contains("TOP SECRET"));
    }
}
