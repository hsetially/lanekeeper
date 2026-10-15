//! The write routes (S4, S11, S21): the REST face of [`ports::WriteService`] that this crate owns, registered through
//! [`hub_platform::GuardedRouter`] so that every route has its guard declared and the route table can be checked against
//! `api/openapi.yaml`.
//!
//! This task mounts the file and service actions:
//!
//! | Route | Role | Action |
//! |---|---|---|
//! | `PUT /api/v1/swimlanes/{id}/files` | Editor | edit |
//! | `DELETE /api/v1/swimlanes/{id}/files` | Editor | delete |
//! | `POST /api/v1/swimlanes/{id}/files/preview` | Editor | preview the effect of an upload or a delete |
//! | `POST /api/v1/swimlanes/{id}/files/upload` | Editor | upload |
//! | `POST /api/v1/swimlanes/{id}/files/revert` | Editor | revert |
//! | `POST /api/v1/swimlanes/{id}/restart` | Operator | restart a service |
//!
//! A route does four things and nothing else: read the request into typed values (a bad one is 400 and says nothing about why),
//! build the [`WriteCtx`], call the service and map its answer to a response. Authorization is the guard's and the service's; the
//! handlers never check a role. Every body has a limit set before it is read, and the guard runs before the body is touched, so
//! an unauthenticated request cannot make the server read a large one (S11).

mod files;
pub mod multipart;
mod problem;
mod upload;
pub mod wire;

use std::sync::Arc;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query};
use axum::handler::Handler;
use axum::response::{IntoResponse, Response};
use domain::{IdempotencyKey, RequestId, Role, SwimlaneId, User, Via, WriteCtx};
use http::{HeaderMap, Method, header};
use hub_platform::{ApiError, AuthenticatedUser, GuardedRouter};
use ports::WriteService;

/// The path of the edit and delete routes.
pub const FILES_PATH: &str = "/api/v1/swimlanes/{id}/files";
pub const PREVIEW_PATH: &str = "/api/v1/swimlanes/{id}/files/preview";
pub const UPLOAD_PATH: &str = "/api/v1/swimlanes/{id}/files/upload";
pub const REVERT_PATH: &str = "/api/v1/swimlanes/{id}/files/revert";
pub const RESTART_PATH: &str = "/api/v1/swimlanes/{id}/restart";

/// Every route this task mounts, with its method and the lowest role that may call it. The route-table test compares this list
/// with `api/openapi.yaml` and with what [`mount`] registers.
pub const WRITE_ROUTES: [(&str, &str, Role); 6] = [
    ("PUT", FILES_PATH, Role::Editor),
    ("DELETE", FILES_PATH, Role::Editor),
    ("POST", PREVIEW_PATH, Role::Editor),
    ("POST", UPLOAD_PATH, Role::Editor),
    ("POST", REVERT_PATH, Role::Editor),
    ("POST", RESTART_PATH, Role::Operator),
];

/// A JSON string can be six times its text (`\u0000`), so a 2 MiB text can need 12 MiB of body. The limit is that, plus the
/// room for the other fields; a request above it is 413 without being read.
pub const MAX_EDIT_BODY: usize = 12 * 1024 * 1024 + 64 * 1024;

/// The bodies of the small JSON requests (revert, restart).
pub const MAX_SMALL_BODY: usize = 16 * 1024;

/// What a multipart body may hold besides the file: the small text fields and the framing.
pub const MULTIPART_OVERHEAD: usize = 64 * 1024;

/// What the routes need: the service they call and the size they allow a file.
pub struct WriteState {
    service: Arc<dyn WriteService>,
    max_file_bytes: usize,
}

impl std::fmt::Debug for WriteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteState")
            .field("max_file_bytes", &self.max_file_bytes)
            .finish_non_exhaustive()
    }
}

impl WriteState {
    /// `max_file_bytes` is the most one file may weigh (the service's own limit, [`crate::WriteConfig::max_content_bytes`]).
    #[must_use]
    pub fn new(service: Arc<dyn WriteService>, max_file_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            service,
            max_file_bytes,
        })
    }
}

/// Register the routes of this task. Every one is guarded; a role below its floor never reaches the handler.
#[must_use]
pub fn mount(router: GuardedRouter, state: Arc<WriteState>) -> GuardedRouter {
    let multipart_limit = state.max_file_bytes.saturating_add(MULTIPART_OVERHEAD);
    let s = state;
    router
        .route(&Method::PUT, FILES_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => files::edit(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_EDIT_BODY))
        })
        .route(&Method::DELETE, FILES_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            move |Path(id): Path<String>,
                  user: Extension<AuthenticatedUser>,
                  headers: HeaderMap,
                  query: Result<Query<files::DeleteQuery>, QueryRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match query {
                        Ok(Query(query)) => files::delete(&s, &id, user.0.0, &headers, query).await,
                        // The extractor's message quotes the input; the client gets the status and nothing else.
                        Err(_) => ApiError::BadRequest.into_response(),
                    }
                }
            }
        })
        .route(&Method::POST, PREVIEW_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => upload::preview(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(multipart_limit))
        })
        .route(&Method::POST, UPLOAD_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => upload::upload(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(multipart_limit))
        })
        .route(&Method::POST, REVERT_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => files::revert(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_SMALL_BODY))
        })
        .route(&Method::POST, RESTART_PATH, Role::Operator, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => files::restart(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_SMALL_BODY))
        })
}

/// The body, or the problem for why it could not be read: over its limit is 413, anything else 400. The extractor's own text is
/// dropped (S21).
fn body_of(body: Result<Bytes, BytesRejection>) -> Result<Bytes, Stop> {
    body.map_err(|e| stop(ApiError::from_status(e.status())))
}

/// The response that ends a request early. Boxed: a response is large and the happy path should not carry its size.
pub(crate) type Stop = Box<Response>;

#[allow(clippy::unnecessary_box_returns)]
pub(crate) fn stop(error: ApiError) -> Stop {
    Box::new(error.into_response())
}

/// The swimlane of the path, or the 400 that says nothing about why.
fn swimlane(id: &str) -> Result<SwimlaneId, Stop> {
    SwimlaneId::parse(id).map_err(|_| stop(ApiError::BadRequest))
}

/// Who is calling and under which key. The `Idempotency-Key` header is required on every write (D68); a missing or malformed
/// one is a 400. The request id correlates this call across logs, audit and agent commands; the edge's own id is on the response.
fn context(user: User, headers: &HeaderMap) -> Result<WriteCtx, Stop> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| IdempotencyKey::parse(v).ok())
        .ok_or_else(|| stop(ApiError::BadRequest))?;
    Ok(WriteCtx {
        user,
        via: Via::Ui,
        idempotency_key: key,
        request_id: new_request_id().ok_or_else(|| stop(ApiError::Internal))?,
    })
}

fn new_request_id() -> Option<RequestId> {
    let mut bytes = [0_u8; 8];
    aws_lc_rs::rand::fill(&mut bytes).ok()?;
    RequestId::parse(&format!("w-{}", crate::hex::encode(&bytes))).ok()
}

/// Is the body `application/json`? Anything else is a 415 (the contract promises one media type).
pub(crate) fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            let media = v.split(';').next().unwrap_or("").trim();
            media.eq_ignore_ascii_case("application/json")
        })
}

/// The response for a refusal: the status and the fixed problem, nothing of the request.
fn refusal(why: wire::Refusal) -> Response {
    match why {
        wire::Refusal::BadRequest => ApiError::BadRequest,
        wire::Refusal::UnsupportedMediaType => ApiError::UnsupportedMediaType,
        wire::Refusal::TooLarge => ApiError::PayloadTooLarge,
    }
    .into_response()
}
