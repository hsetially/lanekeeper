//! The routes of approvals and drafts: `GET /proposals`, `POST /proposals/{id}/approve`, `POST /proposals/{id}/reject`,
//! `POST /drafts` and `POST /drafts/{id}/apply`.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::{DraftId, User};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use hub_platform::ApiError;
use serde::Deserialize;
use serde_json::json;

use super::problem::{outcome, write_error};
use super::wire;
use super::{WriteState, context, is_json, refusal, stop};
use crate::RejectResult;

/// The query of `GET /proposals`. Unknown names are refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    swimlane: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

fn json_response(status: StatusCode, value: &impl serde::Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            response
        }
        Err(_) => ApiError::Internal.into_response(),
    }
}

pub(super) async fn list(state: &WriteState, user: User, query: ListQuery) -> Response {
    let q = match wire::proposal_query(
        query.status.as_deref(),
        query.swimlane.as_deref(),
        query.cursor.as_deref(),
        query.limit,
    ) {
        Ok(q) => q,
        Err(why) => return refusal(why),
    };
    match state.proposals.list_proposals(&user, q).await {
        Ok(page) => {
            let mut response = json_response(
                StatusCode::OK,
                &json!({
                    "items": page.items,
                    "next": page.next.as_ref().map(domain::Cursor::as_str),
                }),
            );
            // Who may approve what is for the person who asked, now.
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => write_error(e),
    }
}

pub(super) async fn approve(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let (id, ctx) = match (wire::proposal_id(id), context(user, headers)) {
        (Ok(id), Ok(ctx)) => (id, ctx),
        (Err(why), _) => return refusal(why),
        (_, Err(e)) => return *e,
    };
    // The body is optional; when there is one it has to say it is JSON.
    if !body.is_empty() && !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let comment = match wire::approve_comment(body) {
        Ok(c) => c,
        Err(why) => return refusal(why),
    };
    match state.proposals.approve_proposal(&ctx, id, comment).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}

pub(super) async fn reject(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let (id, ctx) = match (wire::proposal_id(id), context(user, headers)) {
        (Ok(id), Ok(ctx)) => (id, ctx),
        (Err(why), _) => return refusal(why),
        (_, Err(e)) => return *e,
    };
    if !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let reason = match wire::reject_reason(body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.proposals.reject_proposal(&ctx, id, reason).await {
        Ok(RejectResult::Rejected(view)) => json_response(StatusCode::OK, &*view),
        Ok(RejectResult::Refused(done)) => outcome(&done),
        Err(e) => write_error(e),
    }
}

pub(super) async fn create_draft(
    state: &WriteState,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let ctx = match context(user, headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    if !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let request = match wire::draft_request(body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.propose_draft(&ctx, request).await {
        Ok(draft) => json_response(StatusCode::CREATED, &draft),
        Err(e) => write_error(e),
    }
}

pub(super) async fn apply_draft(state: &WriteState, id: &str, user: User, headers: &HeaderMap) -> Response {
    let (id, ctx) = match (DraftId::parse(id), context(user, headers)) {
        (Ok(id), Ok(ctx)) => (id, ctx),
        (Err(_), _) => return ApiError::BadRequest.into_response(),
        (_, Err(e)) => return *e,
    };
    match state.service.apply_draft(&ctx, id).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}
