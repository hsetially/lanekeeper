//! `application/problem+json` for the write routes (rule 3, S21).
//!
//! A problem names a status and a stable code, and for the outcomes of a write the few fields the contract promises: the current
//! hash of a conflict, the place of a lock, the reason and the places of a rejection. It never carries the content of a file, a
//! path of the server, SQL or a stack trace; the edge adds the request id.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::{ContentHash, RejectReason, Timestamp, ValidationIssue, WriteOutcome};
use http::{HeaderValue, StatusCode, header};
use hub_platform::ApiError;
use hub_platform::error::{PROBLEM_CONTENT_TYPE, ProblemCode};
use ports::WriteError;
use serde_json::{Map, Value, json};

/// A problem with the given status, code and title, and extra fields.
pub(crate) fn problem(
    status: StatusCode,
    code: &'static str,
    title: &'static str,
    extra: Map<String, Value>,
) -> Response {
    let mut object = Map::new();
    object.insert(
        "type".to_owned(),
        Value::String(format!("urn:lanekeeper:problem:{code}")),
    );
    object.insert("title".to_owned(), Value::String(title.to_owned()));
    object.insert("status".to_owned(), Value::from(status.as_u16()));
    object.insert("code".to_owned(), Value::String(code.to_owned()));
    for (k, v) in extra {
        object.entry(k).or_insert(v);
    }
    let body = serde_json::to_vec(&Value::Object(object)).unwrap_or_default();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROBLEM_CONTENT_TYPE),
    );
    response.extensions_mut().insert(ProblemCode(code));
    response
}

/// A problem from the closed set of API errors, with extra fields.
fn api(error: ApiError, extra: Map<String, Value>) -> Response {
    problem(error.status(), error.code(), error.title(), extra)
}

fn issues(list: &[ValidationIssue]) -> Value {
    serde_json::to_value(list).unwrap_or_else(|_| json!([]))
}

fn current(hash: Option<ContentHash>) -> Value {
    hash.map_or(Value::Null, |h| Value::String(h.to_string()))
}

fn until(at: Option<Timestamp>) -> Value {
    at.and_then(|t| serde_json::to_value(t).ok())
        .unwrap_or(Value::Null)
}

/// A second pull request for a change that has an open one (D77): 409, with the link of the one that is there.
pub(crate) fn duplicate_pr(link: &crate::prs::PrLinkView) -> Response {
    let mut extra = Map::new();
    extra.insert("reason".to_owned(), Value::String("duplicate_pr".to_owned()));
    extra.insert(
        "link".to_owned(),
        serde_json::to_value(link).unwrap_or(Value::Null),
    );
    api(ApiError::Conflict, extra)
}

/// An error of the write service, as the response the contract promises.
pub(crate) fn write_error(error: WriteError) -> Response {
    match error {
        WriteError::Forbidden => ApiError::Forbidden.into_response(),
        WriteError::NotFound => ApiError::NotFound.into_response(),
        WriteError::BadRequest => ApiError::BadRequest.into_response(),
        WriteError::IdempotencyConflict => problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_conflict",
            "Idempotency key used for another request",
            Map::new(),
        ),
        WriteError::Unavailable => {
            let mut response = ApiError::Unavailable.into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            response
        }
    }
}

/// The outcome of a write: applied and proposed are answers (200); a conflict is 409 with the hash found instead, a lock 423, and
/// a rejection 413 (too large) or 422 with the reason and the places (lines and rule names, never the text).
pub(crate) fn outcome(outcome: &WriteOutcome) -> Response {
    match outcome {
        WriteOutcome::Applied { .. } | WriteOutcome::ProposalCreated { .. } => {
            match serde_json::to_vec(outcome) {
                Ok(body) => {
                    let mut response = Response::new(Body::from(body));
                    response
                        .headers_mut()
                        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    response
                }
                Err(_) => ApiError::Internal.into_response(),
            }
        }
        WriteOutcome::Conflict { current: hash } => {
            let mut extra = Map::new();
            extra.insert("current".to_owned(), current(*hash));
            api(ApiError::Conflict, extra)
        }
        WriteOutcome::Locked { until: at } => {
            let mut extra = Map::new();
            extra.insert("until".to_owned(), until(*at));
            problem(StatusCode::LOCKED, "locked", "Locked", extra)
        }
        WriteOutcome::Rejected {
            reason,
            issues: found,
        } => {
            let mut extra = Map::new();
            extra.insert(
                "reason".to_owned(),
                serde_json::to_value(reason).unwrap_or(Value::Null),
            );
            if !found.is_empty() {
                extra.insert("issues".to_owned(), issues(found));
            }
            match reason {
                RejectReason::TooLarge => api(ApiError::PayloadTooLarge, extra),
                _ => api(ApiError::Unprocessable, extra),
            }
        }
    }
}
