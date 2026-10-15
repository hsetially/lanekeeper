//! The write routes (S4, S11, S21): the REST face of [`ports::WriteService`] that this crate owns, registered through
//! [`hub_platform::GuardedRouter`] so that every route has its guard declared and the route table can be checked against
//! `api/openapi.yaml`.
//!
//! The file and service actions, and the approvals and drafts:
//!
//! | Route | Role | Action |
//! |---|---|---|
//! | `PUT /api/v1/swimlanes/{id}/files` | Editor | edit |
//! | `DELETE /api/v1/swimlanes/{id}/files` | Editor | delete |
//! | `POST /api/v1/swimlanes/{id}/files/preview` | Editor | preview the effect of an upload or a delete |
//! | `POST /api/v1/swimlanes/{id}/files/upload` | Editor | upload |
//! | `POST /api/v1/swimlanes/{id}/files/revert` | Editor | revert |
//! | `POST /api/v1/swimlanes/{id}/files/reveal` | Editor | reveal a masked value (audited, rate limited, never cached) |
//! | `POST /api/v1/swimlanes/{id}/restart` | Operator | restart a service |
//! | `GET /api/v1/proposals` | Viewer | list proposals |
//! | `POST /api/v1/proposals/{id}/approve` | Admin | approve a proposal |
//! | `POST /api/v1/proposals/{id}/reject` | Admin | reject a proposal |
//! | `POST /api/v1/drafts` | Editor | create a draft |
//! | `POST /api/v1/drafts/{id}/apply` | Editor | apply a draft |
//! | `PUT /api/v1/me/github-token` | Editor | store or replace the caller's GitHub token |
//! | `DELETE /api/v1/me/github-token` | Editor | remove the caller's GitHub token |
//! | `POST /api/v1/prs` | Editor | raise a pull request (accepts a job, 202) |
//! | `GET /api/v1/prs` | Viewer | list pull-request links |
//! | `GET /api/v1/pr-jobs/{id}` | Viewer | a pull-request job |
//!
//! A route does four things and nothing else: read the request into typed values (a bad one is 400 and says nothing about why),
//! build the [`WriteCtx`], call the service and map its answer to a response. Authorization is the guard's and the service's; the
//! handlers never check a role. Every body has a limit set before it is read, and the guard runs before the body is touched, so
//! an unauthenticated request cannot make the server read a large one (S11).

mod files;
mod me;
pub mod multipart;
mod problem;
mod proposals;
mod prs;
mod reveal;
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

use crate::ProposalDesk;
use crate::RevealDesk;
use crate::prs::PrDesk;
use crate::vault::TokenDesk;

/// The path of the edit and delete routes.
pub const FILES_PATH: &str = "/api/v1/swimlanes/{id}/files";
pub const PREVIEW_PATH: &str = "/api/v1/swimlanes/{id}/files/preview";
pub const UPLOAD_PATH: &str = "/api/v1/swimlanes/{id}/files/upload";
pub const REVERT_PATH: &str = "/api/v1/swimlanes/{id}/files/revert";
pub const REVEAL_PATH: &str = "/api/v1/swimlanes/{id}/files/reveal";
pub const RESTART_PATH: &str = "/api/v1/swimlanes/{id}/restart";
pub const PROPOSALS_PATH: &str = "/api/v1/proposals";
pub const APPROVE_PATH: &str = "/api/v1/proposals/{id}/approve";
pub const REJECT_PATH: &str = "/api/v1/proposals/{id}/reject";
pub const DRAFTS_PATH: &str = "/api/v1/drafts";
pub const APPLY_DRAFT_PATH: &str = "/api/v1/drafts/{id}/apply";
pub const GITHUB_TOKEN_PATH: &str = "/api/v1/me/github-token";
pub const PRS_PATH: &str = "/api/v1/prs";
pub const PR_JOB_PATH: &str = "/api/v1/pr-jobs/{id}";

/// Every route this task mounts, with its method and the lowest role that may call it. The route-table test compares this list
/// with `api/openapi.yaml` and with what [`mount`] registers.
pub const WRITE_ROUTES: [(&str, &str, Role); 17] = [
    ("PUT", FILES_PATH, Role::Editor),
    ("DELETE", FILES_PATH, Role::Editor),
    ("POST", PREVIEW_PATH, Role::Editor),
    ("POST", UPLOAD_PATH, Role::Editor),
    ("POST", REVERT_PATH, Role::Editor),
    ("POST", REVEAL_PATH, Role::Editor),
    ("POST", RESTART_PATH, Role::Operator),
    ("GET", PROPOSALS_PATH, Role::Viewer),
    ("POST", APPROVE_PATH, Role::Admin),
    ("POST", REJECT_PATH, Role::Admin),
    ("POST", DRAFTS_PATH, Role::Editor),
    ("POST", APPLY_DRAFT_PATH, Role::Editor),
    ("PUT", GITHUB_TOKEN_PATH, Role::Editor),
    ("DELETE", GITHUB_TOKEN_PATH, Role::Editor),
    ("POST", PRS_PATH, Role::Editor),
    ("GET", PRS_PATH, Role::Viewer),
    ("GET", PR_JOB_PATH, Role::Viewer),
];

/// A JSON string can be six times its text (`\u0000`), so a 2 MiB text can need 12 MiB of body. The limit is that, plus the
/// room for the other fields; a request above it is 413 without being read.
pub const MAX_EDIT_BODY: usize = 12 * 1024 * 1024 + 64 * 1024;

/// The bodies of the small JSON requests (revert, restart).
pub const MAX_SMALL_BODY: usize = 16 * 1024;

/// The body of a token request: a token of at most 512 characters, which JSON may write as six bytes each, and the field name.
pub const MAX_TOKEN_BODY: usize = 6 * crate::github::MAX_TOKEN_LEN + 256;

/// The body of `POST /prs`: the person's text of 65,536 characters (a JSON string can be six times its text), 100 paths of up to
/// 1,024 characters (the same), the title and the framing.
pub const MAX_PR_BODY: usize = 6 * wire::MAX_PR_TEXT + 100 * (6 * 1024 + 8) + 4096;

/// What a multipart body may hold besides the file: the small text fields and the framing.
pub const MULTIPART_OVERHEAD: usize = 64 * 1024;

/// What the routes need: the services they call and the size they allow a file.
pub struct WriteState {
    service: Arc<dyn WriteService>,
    proposals: Arc<dyn ProposalDesk>,
    tokens: Arc<dyn TokenDesk>,
    prs: Arc<dyn PrDesk>,
    reveal: Arc<dyn RevealDesk>,
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
    pub fn new(
        service: Arc<dyn WriteService>,
        proposals: Arc<dyn ProposalDesk>,
        tokens: Arc<dyn TokenDesk>,
        prs: Arc<dyn PrDesk>,
        reveal: Arc<dyn RevealDesk>,
        max_file_bytes: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            service,
            proposals,
            tokens,
            prs,
            reveal,
            max_file_bytes,
        })
    }
}

/// Register the routes of this task. Every one is guarded; a role below its floor never reaches the handler.
#[must_use]
// One registration for each route, in the order of the table above; splitting it would hide which routes exist.
#[allow(clippy::too_many_lines)]
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
        .route(&Method::POST, REVEAL_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => reveal::reveal(&s, &id, user.0.0, &headers, &body).await,
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
        .route(&Method::GET, PROPOSALS_PATH, Role::Viewer, {
            let s = Arc::clone(&s);
            move |user: Extension<AuthenticatedUser>,
                  query: Result<Query<proposals::ListQuery>, QueryRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match query {
                        Ok(Query(query)) => proposals::list(&s, user.0.0, query).await,
                        Err(_) => ApiError::BadRequest.into_response(),
                    }
                }
            }
        })
        .route(&Method::POST, APPROVE_PATH, Role::Admin, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => proposals::approve(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_SMALL_BODY))
        })
        .route(&Method::POST, REJECT_PATH, Role::Admin, {
            let s = Arc::clone(&s);
            (move |Path(id): Path<String>,
                   user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => proposals::reject(&s, &id, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_SMALL_BODY))
        })
        .route(&Method::POST, DRAFTS_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => proposals::create_draft(&s, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_EDIT_BODY))
        })
        .route(&Method::POST, APPLY_DRAFT_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            move |Path(id): Path<String>, user: Extension<AuthenticatedUser>, headers: HeaderMap| {
                let s = Arc::clone(&s);
                async move { proposals::apply_draft(&s, &id, user.0.0, &headers).await }
            }
        })
        .route(&Method::PUT, GITHUB_TOKEN_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => me::put(&s, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_TOKEN_BODY))
        })
        .route(&Method::DELETE, GITHUB_TOKEN_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            move |user: Extension<AuthenticatedUser>, headers: HeaderMap| {
                let s = Arc::clone(&s);
                async move { me::delete(&s, user.0.0, &headers).await }
            }
        })
        .route(&Method::POST, PRS_PATH, Role::Editor, {
            let s = Arc::clone(&s);
            (move |user: Extension<AuthenticatedUser>,
                   headers: HeaderMap,
                   body: Result<Bytes, BytesRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match body_of(body) {
                        Ok(body) => prs::create(&s, user.0.0, &headers, &body).await,
                        Err(e) => *e,
                    }
                }
            })
            .layer(DefaultBodyLimit::max(MAX_PR_BODY))
        })
        .route(&Method::GET, PRS_PATH, Role::Viewer, {
            let s = Arc::clone(&s);
            move |user: Extension<AuthenticatedUser>, query: Result<Query<prs::ListQuery>, QueryRejection>| {
                let s = Arc::clone(&s);
                async move {
                    match query {
                        Ok(Query(query)) => prs::list(&s, user.0.0, query).await,
                        Err(_) => ApiError::BadRequest.into_response(),
                    }
                }
            }
        })
        .route(&Method::GET, PR_JOB_PATH, Role::Viewer, {
            let s = Arc::clone(&s);
            move |Path(id): Path<String>, user: Extension<AuthenticatedUser>| {
                let s = Arc::clone(&s);
                async move { prs::job(&s, &id, user.0.0).await }
            }
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
