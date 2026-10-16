//! `POST /swimlanes/{id}/notify` (T11, D85, Q37): tell the config-server which files changed. Operator only.
//!
//! The answer to an applied notification is the contract's `NotifyResult`: how many paths were sent and the audit event that records
//! the call. A user who needs approval gets the proposal's own answer (`proposal_created`), as every other write route gives.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::{User, WriteOutcome};
use http::{HeaderMap, HeaderValue, header};
use hub_platform::ApiError;
use serde_json::json;

use super::problem::{outcome, write_error};
use super::wire;
use super::{WriteState, context, is_json, refusal, stop, swimlane};

pub(super) async fn notify(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
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
    let request = match wire::notify_request(swimlane, body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    // The paths that were sent are the request's, once each (the reader refused a repeat).
    let sent = request.paths.len();
    match state.service.notify(&ctx, request).await {
        Ok(WriteOutcome::Applied { audit_id, .. }) => {
            let body = json!({ "accepted": sent, "audit_id": audit_id.get() });
            match serde_json::to_vec(&body) {
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
        Ok(other) => outcome(&other),
        Err(e) => write_error(e),
    }
}
