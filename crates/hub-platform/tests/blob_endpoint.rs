//! T8 (P7, S4, S11, S12, S21): `GET /api/v1/blobs/{hash}`, over the blob store port's fake so that no database is needed. The `ETag` is the
//! hash, a blob is immutable and private, a hash the registry withholds is a 404 that looks like any other, and a blob is
//! never rendered as a page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{ContentHash, Role, UserStatus};
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use hub_platform::blobs::endpoint::{BlobPolicy, BlobState, DenyHashes, mount};
use hub_platform::{FakeSessionAuth, GuardedRouter, PROBLEM_CONTENT_TYPE, TEST_USER_HEADER};
use ports::fakes::FakeBlobStore;
use ports::{BlobError, BlobStore, content_hash};
use support::log_capture::LogCapture;
use tower::ServiceExt;

const IMMUTABLE: &str = "private, max-age=31536000, immutable";

/// A store whose database is gone.
#[derive(Debug)]
struct Down;
#[async_trait]
impl BlobStore for Down {
    async fn put(&self, _: Bytes) -> Result<ContentHash, BlobError> {
        Err(BlobError::Unavailable)
    }
    async fn get(&self, _: &ContentHash) -> Result<Option<Bytes>, BlobError> {
        Err(BlobError::Unavailable)
    }
    async fn get_many(&self, _: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
        Err(BlobError::Unavailable)
    }
}

fn auth() -> Arc<FakeSessionAuth> {
    Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Admin), UserStatus::Disabled)
            .with_user("norole", None, UserStatus::Active),
    )
}

fn router(store: Arc<dyn BlobStore>, policy: Arc<dyn BlobPolicy>) -> Router {
    let state = BlobState::new(store, policy);
    let (router, specs) = mount(GuardedRouter::new(auth(), None), state)
        .into_parts()
        .unwrap();
    assert_eq!(specs.len(), 1, "the endpoint registers exactly one route");
    router
}

fn allow_all() -> Arc<dyn BlobPolicy> {
    Arc::new(DenyHashes::default())
}

fn get_as(user: Option<&str>, hash: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/blobs/{hash}"));
    if let Some(user) = user {
        request = request.header(TEST_USER_HEADER, user);
    }
    request.body(Body::empty()).unwrap()
}

fn get(hash: &str) -> Request<Body> {
    get_as(Some("viewer"), hash)
}

async fn send(router: &Router, request: Request<Body>) -> Response<Body> {
    router.clone().oneshot(request).await.unwrap()
}

async fn bytes(response: Response<Body>) -> Bytes {
    response.into_body().collect().await.unwrap().to_bytes()
}

fn header_of<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

async fn stored(store: &FakeBlobStore, content: &'static [u8]) -> ContentHash {
    store.put(Bytes::from_static(content)).await.unwrap()
}

#[tokio::test]
async fn blob_endpoint_etag_equals_hash_and_is_immutable() {
    let store = Arc::new(FakeBlobStore::new());
    let hash = stored(&store, b"a: 1\r\nb: 2\r\n").await;
    let router = router(store, allow_all());

    let response = send(&router, get(&hash.to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "etag"), Some(format!("\"{hash}\"").as_str()));
    assert_eq!(header_of(&response, "cache-control"), Some(IMMUTABLE));
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header_of(&response, "content-length"), Some("12"));
    // The bytes are exactly the stored bytes: CRLF is not normalised on the way out.
    assert_eq!(bytes(response).await, Bytes::from_static(b"a: 1\r\nb: 2\r\n"));
}

#[tokio::test]
async fn blob_endpoint_etag_is_the_hash_whatever_the_case_of_the_request() {
    let store = Arc::new(FakeBlobStore::new());
    let hash = stored(&store, b"case").await;
    let router = router(store, allow_all());
    let upper = hash.to_string().to_ascii_uppercase();
    let response = send(&router, get(&upper)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, "etag"),
        Some(format!("\"{hash}\"").as_str()),
        "one blob has one ETag, the lower-case hash"
    );
}

#[tokio::test]
async fn blob_endpoint_if_none_match_is_304() {
    let store = Arc::new(FakeBlobStore::new());
    let hash = stored(&store, b"unchanged").await;
    let other = content_hash(b"something else");
    let router = router(store, allow_all());
    let etag = format!("\"{hash}\"");

    for matching in [
        etag.clone(),
        format!("W/{etag}"),
        format!("\"{other}\", {etag}"),
        "*".to_owned(),
    ] {
        let mut request = get(&hash.to_string());
        request
            .headers_mut()
            .insert(header::IF_NONE_MATCH, matching.parse().unwrap());
        let response = send(&router, request).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{matching}");
        assert_eq!(header_of(&response, "etag"), Some(etag.as_str()));
        assert_eq!(header_of(&response, "cache-control"), Some(IMMUTABLE));
        assert!(bytes(response).await.is_empty(), "a 304 has no body");
    }

    for not_matching in [format!("\"{other}\""), "garbage".to_owned(), String::new()] {
        let mut request = get(&hash.to_string());
        request
            .headers_mut()
            .insert(header::IF_NONE_MATCH, not_matching.parse().unwrap());
        let response = send(&router, request).await;
        assert_eq!(response.status(), StatusCode::OK, "{not_matching:?}");
        assert_eq!(bytes(response).await, Bytes::from_static(b"unchanged"));
    }
}

#[tokio::test]
async fn a_matching_if_none_match_does_not_confirm_a_blob_that_does_not_exist() {
    let router = router(Arc::new(FakeBlobStore::new()), allow_all());
    let unknown = content_hash(b"never stored");
    let mut request = get(&unknown.to_string());
    request
        .headers_mut()
        .insert(header::IF_NONE_MATCH, format!("\"{unknown}\"").parse().unwrap());
    let response = send(&router, request).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "an ETag the client asserts is not proof that the hub holds the blob"
    );
}

#[tokio::test]
async fn blob_hash_must_be_64_hex() {
    let router = router(Arc::new(FakeBlobStore::new()), allow_all());
    for bad in [
        "abc".to_owned(),
        "a".repeat(63),
        "a".repeat(65),
        "g".repeat(64),
        "G".repeat(64),
        format!("{}%2e", "a".repeat(62)),
        format!("{}%00", "a".repeat(62)),
    ] {
        let response = send(&router, get(&bad)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        assert_eq!(header_of(&response, "content-type"), Some(PROBLEM_CONTENT_TYPE));
        let body = String::from_utf8(bytes(response).await.to_vec()).unwrap();
        assert!(body.contains("\"code\":\"bad_request\""), "{body}");
        assert!(
            !body.contains(&bad),
            "the problem does not echo the input: {body}"
        );
    }
    // Paths with a slash in the "hash" match no route at all: the router's own 404, never the handler.
    for stray in ["", "../abc", "a/b"] {
        let response = send(&router, get(stray)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{stray:?}");
    }
}

#[tokio::test]
async fn an_unknown_blob_is_a_404_problem() {
    let router = router(Arc::new(FakeBlobStore::new()), allow_all());
    let response = send(&router, get(&content_hash(b"nope").to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(header_of(&response, "content-type"), Some(PROBLEM_CONTENT_TYPE));
    assert!(
        header_of(&response, "etag").is_none(),
        "a 404 carries no ETag and no immutable cache header"
    );
    assert!(header_of(&response, "cache-control") != Some(IMMUTABLE));
}

#[tokio::test]
async fn a_denied_or_withheld_blob_is_a_404_that_looks_like_an_unknown_one() {
    let store = Arc::new(FakeBlobStore::new());
    let secret = stored(&store, b"password: hunter2\n").await;
    let unknown = content_hash(b"never stored");
    let router = router(store, Arc::new(DenyHashes::new([secret])));

    let denied = send(&router, get(&secret.to_string())).await;
    let missing = send(&router, get(&unknown.to_string())).await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    for name in ["content-type", "content-length", "cache-control", "etag"] {
        assert_eq!(
            header_of(&denied, name),
            header_of(&missing, name),
            "{name} must not tell a withheld blob from an unknown one"
        );
    }
    assert_eq!(bytes(denied).await, bytes(missing).await);

    // A matching ETag does not get a denied blob a 304 either.
    let mut request = get(&secret.to_string());
    request
        .headers_mut()
        .insert(header::IF_NONE_MATCH, format!("\"{secret}\"").parse().unwrap());
    assert_eq!(send(&router, request).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_policy_is_asked_before_the_store_is_read() {
    // A store that fails the test if it is read for a denied hash: withheld content never leaves the store.
    #[derive(Debug)]
    struct ReadsAreForbidden;
    #[async_trait]
    impl BlobStore for ReadsAreForbidden {
        async fn put(&self, _: Bytes) -> Result<ContentHash, BlobError> {
            Err(BlobError::Unavailable)
        }
        async fn get(&self, _: &ContentHash) -> Result<Option<Bytes>, BlobError> {
            panic!("a withheld blob was read from the store");
        }
        async fn get_many(&self, _: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
            panic!("a withheld blob was read from the store");
        }
    }
    let hash = content_hash(b"withheld");
    let router = router(Arc::new(ReadsAreForbidden), Arc::new(DenyHashes::new([hash])));
    let response = send(&router, get(&hash.to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blob_is_served_as_a_download_never_inline() {
    let store = Arc::new(FakeBlobStore::new());
    // The bytes of a page: if anything rendered a blob, this would run.
    let hash = stored(&store, b"<html><script>alert(1)</script></html>").await;
    let router = router(store, allow_all());
    let response = send(&router, get(&hash.to_string())).await;
    assert_eq!(
        header_of(&response, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(header_of(&response, "x-content-type-options"), Some("nosniff"));
    let disposition = header_of(&response, "content-disposition").unwrap();
    assert!(disposition.starts_with("attachment"), "{disposition}");
}

#[tokio::test]
async fn sse_and_blob_require_viewer_and_active_user_blob_side() {
    let store = Arc::new(FakeBlobStore::new());
    let hash = stored(&store, b"content").await;
    let router = router(store, allow_all());
    let path = hash.to_string();
    assert_eq!(
        send(&router, get_as(Some("viewer"), &path)).await.status(),
        StatusCode::OK
    );
    for (user, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some("nobody"), StatusCode::UNAUTHORIZED),
        (Some("pending"), StatusCode::FORBIDDEN),
        (Some("disabled"), StatusCode::FORBIDDEN),
        (Some("norole"), StatusCode::FORBIDDEN),
    ] {
        let response = send(&router, get_as(user, &path)).await;
        assert_eq!(response.status(), expected, "{user:?}");
        assert_eq!(header_of(&response, "content-type"), Some(PROBLEM_CONTENT_TYPE));
        assert!(header_of(&response, "etag").is_none());
    }
}

#[tokio::test]
async fn a_store_failure_is_a_503_problem_without_internals() {
    let router = router(Arc::new(Down), allow_all());
    let response = send(&router, get(&content_hash(b"x").to_string())).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(header_of(&response, "content-type"), Some(PROBLEM_CONTENT_TYPE));
    assert!(header_of(&response, "etag").is_none());
    let body = String::from_utf8(bytes(response).await.to_vec()).unwrap();
    assert!(body.contains("\"code\":\"unavailable\""), "{body}");
    assert!(!body.to_ascii_lowercase().contains("blob store"), "{body}");
}

#[tokio::test]
async fn log_capture_has_no_secret_content_or_env_value_blob() {
    let (logs, _guard) = LogCapture::install();
    let store = Arc::new(FakeBlobStore::new());
    let planted: &'static [u8] = b"db_password: PLANTED-BLOB-SECRET-9f2c\n";
    let hash = stored(&store, planted).await;
    let denied = stored(&store, b"token: PLANTED-DENIED-SECRET-77aa\n").await;
    let router = router(store, Arc::new(DenyHashes::new([denied])));
    for path in [
        hash.to_string(),
        denied.to_string(),
        content_hash(b"unknown").to_string(),
        "zz".to_owned(),
    ] {
        let _ = send(&router, get(&path)).await;
    }
    let mut revalidate = get(&hash.to_string());
    revalidate
        .headers_mut()
        .insert(header::IF_NONE_MATCH, format!("\"{hash}\"").parse().unwrap());
    let _ = send(&router, revalidate).await;
    // A failing store, so that the capture is shown to be live: it must have seen the failure, and still hold no content.
    let down = self::router(Arc::new(Down), allow_all());
    let _ = send(&down, get(&hash.to_string())).await;
    assert!(!logs.is_empty(), "the capture saw nothing, so it proves nothing");
    let text = logs.text();
    for secret in [
        "PLANTED-BLOB-SECRET",
        "PLANTED-DENIED-SECRET",
        "hunter2",
        "db_password",
    ] {
        assert!(!text.contains(secret), "the log holds blob content: {text}");
    }
}
