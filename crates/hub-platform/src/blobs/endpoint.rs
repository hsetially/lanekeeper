//! `GET /api/v1/blobs/{hash}`: content by address (T8, P7, S4, S11, S12).
//!
//! A blob never changes, so the response says so: the `ETag` is the hash, the cache header is `private, max-age=31536000,
//! immutable` (private because only signed-in users may have it, never a shared cache), and a request that already has it
//! (`If-None-Match`) gets a 304 with no body. A blob is always a download: `application/octet-stream`, `nosniff` and
//! `Content-Disposition: attachment`, so nothing in it is ever rendered as a page (S12). Decoded text for the screen comes from
//! the file endpoints, not from here.
//!
//! **Withheld content.** Whether a hash may be served is decided by the registry (denied files and files whose content is
//! withheld, D79), which is not in this crate, so the endpoint asks a [`BlobPolicy`] first. A hash the policy refuses is a 404
//! identical to an unknown one: the answer never says "exists but not for you". The policy is asked before the store is read, so
//! withheld bytes are never even loaded for the request. A 304 is answered only for a blob that exists and is allowed: an `ETag` the
//! client asserts proves nothing about what the hub holds.
//!
//! **Authorization (S4).** The route is registered through [`GuardedRouter`] at Viewer, so pending, disabled and unknown users
//! never reach the handler. **Validation (S11).** The hash is parsed to a [`ContentHash`] (64 hex digits, either case) before
//! anything else; a bad one is a 400 that does not echo it. **Errors (S21).** A failing store is a 503 problem with a fixed
//! code, never the driver's message.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use domain::{ContentHash, Role};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use ports::{BlobError, BlobStore};
use tracing::debug;

use crate::error::ApiError;
use crate::routes::{BLOB_PATH, GuardedRouter};

/// What `api/openapi.yaml` promises for a blob.
const IMMUTABLE: &str = "private, max-age=31536000, immutable";
/// The longest `If-None-Match` looked at. The contract allows 200 bytes; a longer one is ignored, not an error.
const MAX_IF_NONE_MATCH: usize = 200;

/// Whether a blob may be served at all.
#[async_trait]
pub trait BlobPolicy: Send + Sync + 'static {
    /// `false` for a hash the registry marks denied or withheld. `true` for everything else, including hashes it has never heard
    /// of (a blob can be stored before anything refers to it).
    async fn allowed(&self, hash: &ContentHash) -> bool;
}

/// A policy that refuses a fixed set of hashes and allows everything else: the test double. It is not constructible in a release
/// build of the hub, so the binary cannot start with a policy that denies nothing by accident (plan A13).
#[cfg(any(test, feature = "fakes"))]
#[derive(Debug, Default)]
pub struct DenyHashes {
    denied: std::collections::HashSet<ContentHash>,
}

#[cfg(any(test, feature = "fakes"))]
impl DenyHashes {
    #[must_use]
    pub fn new(denied: impl IntoIterator<Item = ContentHash>) -> Self {
        Self {
            denied: denied.into_iter().collect(),
        }
    }
}

#[cfg(any(test, feature = "fakes"))]
#[async_trait]
impl BlobPolicy for DenyHashes {
    async fn allowed(&self, hash: &ContentHash) -> bool {
        !self.denied.contains(hash)
    }
}

/// Everything the endpoint needs.
pub struct BlobState {
    store: Arc<dyn BlobStore>,
    policy: Arc<dyn BlobPolicy>,
}

impl std::fmt::Debug for BlobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobState").finish_non_exhaustive()
    }
}

impl BlobState {
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, policy: Arc<dyn BlobPolicy>) -> Arc<Self> {
        Arc::new(Self { store, policy })
    }
}

/// Register `GET /api/v1/blobs/{hash}` for Viewers and above.
#[must_use]
pub fn mount(router: GuardedRouter, state: Arc<BlobState>) -> GuardedRouter {
    router.route(
        &Method::GET,
        BLOB_PATH,
        Role::Viewer,
        move |Path(hash): Path<String>, headers: HeaderMap| {
            let state = Arc::clone(&state);
            async move { serve(&state, &hash, &headers).await }
        },
    )
}

async fn serve(state: &BlobState, hash: &str, headers: &HeaderMap) -> Response {
    let Ok(hash) = ContentHash::parse(hash) else {
        return ApiError::BadRequest.into_response();
    };
    // Withheld content is not read, and is answered exactly as an unknown hash is.
    if !state.policy.allowed(&hash).await {
        return ApiError::NotFound.into_response();
    }
    let bytes = match state.store.get(&hash).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return ApiError::NotFound.into_response(),
        Err(error) => {
            debug!(?error, "blob read failed");
            return match error {
                BlobError::Unavailable | BlobError::Full => ApiError::Unavailable.into_response(),
                BlobError::TooLarge | BlobError::TooMany => ApiError::Internal.into_response(),
            };
        }
    };
    // `Display` is the lower-case hex, so one blob has one `ETag` whatever the case of the request.
    let etag = format!("\"{hash}\"");
    if matches(headers, &etag) {
        return with_validators(Response::new(Body::empty()), StatusCode::NOT_MODIFIED, &etag);
    }
    download(bytes, &etag)
}

/// A 200 or 304 with the headers that make a blob immutable.
fn with_validators(mut response: Response, status: StatusCode, etag: &str) -> Response {
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
    response
}

fn download(bytes: Bytes, etag: &str) -> Response {
    let mut response = with_validators(Response::new(Body::from(bytes)), StatusCode::OK, etag);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"blob\""),
    );
    response
}

/// Whether `If-None-Match` names `etag` (RFC 9110: a weak comparison, so `W/` is ignored; `*` matches any blob that exists).
fn matches(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers.get(header::IF_NONE_MATCH) else {
        return false;
    };
    if value.len() > MAX_IF_NONE_MATCH {
        return false;
    }
    let Ok(text) = value.to_str() else {
        return false;
    };
    text.split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag)
}
