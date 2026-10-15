//! `POST /swimlanes/{id}/files/reveal` (D79, S14b): the one audited way to see a value the secret scan flags.
//!
//! The answer is the value, or a fixed problem, and it is never cached: every response of this handler, the failures too, carries
//! `Cache-Control: no-store`, so no proxy or browser cache keeps a secret or a refusal that mentions one. The route reads the
//! request into typed values and calls the desk; the role, the rate limit and the audit event are the desk's (`crate::reveal`).
//! Nothing here keeps, logs or formats the value: it is a [`domain::Secret`] until the moment it is written into the body.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::User;
use http::{HeaderMap, HeaderValue, header};
use hub_platform::ApiError;
use serde::Serialize;

use super::{WriteState, context, is_json, refusal, stop, swimlane};
use crate::{RevealError, Revealed};

/// `RevealedValue` of the contract.
#[derive(Serialize)]
struct Shown<'a> {
    value: &'a str,
}

pub(super) async fn reveal(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let mut response = run(state, id, user, headers, body).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn run(state: &WriteState, id: &str, user: User, headers: &HeaderMap, body: &[u8]) -> Response {
    let swimlane = match swimlane(id) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let ctx = match context(user, headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    if !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let request = match super::wire::reveal_request(swimlane, body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.reveal.reveal(&ctx, request).await {
        Ok(revealed) => shown(&revealed),
        Err(e) => failed(e),
    }
}

fn shown(revealed: &Revealed) -> Response {
    match serde_json::to_vec(&Shown {
        value: revealed.value.expose(),
    }) {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            response
        }
        Err(_) => ApiError::Internal.into_response(),
    }
}

/// A refusal of the desk as the response the contract promises. The words are this file's, not the request's.
fn failed(error: RevealError) -> Response {
    match error {
        RevealError::Forbidden => ApiError::Forbidden.into_response(),
        RevealError::NotFound => ApiError::NotFound.into_response(),
        RevealError::Throttled { retry_after_secs } => {
            let mut response = ApiError::TooManyRequests.into_response();
            if let Ok(wait) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, wait);
            }
            response
        }
        RevealError::Unavailable => {
            let mut response = ApiError::Unavailable.into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            response
        }
    }
}
