//! The routes of pull requests: `POST /prs` (accepts a job, 202), `GET /prs` (the links) and `GET /pr-jobs/{id}`.

use axum::response::{IntoResponse, Response};
use domain::{PrJobId, PrJobState, User, WriteOutcome};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use hub_platform::ApiError;
use serde::Deserialize;
use serde_json::json;

use super::problem::{duplicate_pr, outcome, write_error};
use super::proposals::json_response;
use super::{WriteState, context, is_json, refusal, stop, wire};
use crate::prs::Raised;

/// The query of `GET /prs`. Unknown names are refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    #[serde(default)]
    swimlane: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

pub(super) async fn create(state: &WriteState, user: User, headers: &HeaderMap, body: &[u8]) -> Response {
    let ctx = match context(user, headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    if !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let request = match wire::pr_request(body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.prs.raise(&ctx, request).await {
        Ok(Raised::Job(view)) => {
            // Accepted: the job runs in the background. A retry that finds the job finished starts nothing.
            if !matches!(view.state, PrJobState::Completed | PrJobState::Failed) {
                state.prs.start(PrJobId::from(view.id));
            }
            json_response(StatusCode::ACCEPTED, &view)
        }
        Ok(Raised::Duplicate(link)) => duplicate_pr(&link),
        Ok(Raised::Proposal(id)) => outcome(&WriteOutcome::ProposalCreated { id }),
        Ok(Raised::Refused(refused)) => outcome(&refused),
        Err(e) => write_error(e),
    }
}

pub(super) async fn list(state: &WriteState, user: User, query: ListQuery) -> Response {
    let q = match wire::pr_link_query(
        query.swimlane.as_deref(),
        query.state.as_deref(),
        query.cursor.as_deref(),
        query.limit,
    ) {
        Ok(q) => q,
        Err(why) => return refusal(why),
    };
    match state.prs.links(&user, q).await {
        Ok(page) => {
            let mut response = json_response(
                StatusCode::OK,
                &json!({
                    "items": page.items,
                    "next": page.next.as_ref().map(domain::Cursor::as_str),
                }),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => write_error(e),
    }
}

pub(super) async fn job(state: &WriteState, id: &str, user: User) -> Response {
    let id = match wire::pr_job_id(id) {
        Ok(id) => id,
        Err(why) => return refusal(why),
    };
    match state.prs.job(&user, id).await {
        Ok(Some(view)) => {
            let mut response = json_response(StatusCode::OK, &view);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        // A 404 can be kept by a cache that nobody told otherwise, and the number may be allocated a moment later.
        Ok(None) => {
            let mut response = ApiError::NotFound.into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => write_error(e),
    }
}
