//! `POST .../files/upload` and `POST .../files/preview`: the two multipart forms.

use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use domain::User;
use http::{HeaderMap, HeaderValue, header};
use hub_platform::ApiError;

use super::problem::{outcome, write_error};
use super::wire;
use super::{Stop, WriteState, context, refusal, stop, swimlane};

/// The content type of the request, or the 415 that says the form needs one.
fn content_type(headers: &HeaderMap) -> Result<&str, Stop> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| stop(ApiError::UnsupportedMediaType))
}

pub(super) async fn upload(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let (swimlane, ctx) = match (swimlane(id), context(user, headers)) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return *e,
    };
    let content_type = match content_type(headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    let request = match wire::upload_request(swimlane, content_type, body, state.max_file_bytes) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.upload(&ctx, request).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}

pub(super) async fn preview(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    let (swimlane, ctx) = match (swimlane(id), context(user, headers)) {
        (Ok(s), Ok(c)) => (s, c),
        (Err(e), _) | (_, Err(e)) => return *e,
    };
    let content_type = match content_type(headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    let request = match wire::preview_request(swimlane, content_type, body, state.max_file_bytes) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.preview_effect(&ctx, request).await {
        Ok(preview) => match serde_json::to_vec(&preview) {
            Ok(json) => {
                let mut response = Response::new(axum::body::Body::from(json));
                let h = response.headers_mut();
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
                // A preview is for the person who asked, for ten minutes; nothing may cache it.
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            }
            Err(_) => ApiError::Internal.into_response(),
        },
        Err(e) => write_error(e),
    }
}
