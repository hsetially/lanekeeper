//! HTTP routes of the identity component (T3, T4). [`identity_routes`] registers all of them on a
//! [`crate::guard::GuardedRouter`], each with its declared guard:
//!
//! | Route | Guard |
//! |---|---|
//! | `GET /auth/login`, `GET /auth/callback`, `POST /auth/logout` | public, each with a reason ([`auth`]) |
//! | `GET /api/v1/me`, `POST /api/v1/me/access-request` | viewer ([`me`]) |
//! | `GET /api/v1/admin/users`, `PATCH /api/v1/admin/users/{id}`, the access-request list, approve and reject | admin ([`admin`]) |
//!
//! `Idempotency-Key` is accepted and ignored here (B17): approve and reject answer `409` on a repeat, an update that changes
//! nothing writes nothing, and the hub-level idempotency layer wraps these routes later.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::Response;
use domain::RequestId;

use crate::guard::GuardedRouter;
use crate::problem::{problem, problem_with_detail};
use crate::users::onboarding::GithubTokenPresence;
use crate::users::rules::RuleError;
use crate::users::{AccessError, PgUsers, UserAdmin};

pub mod admin;
pub mod auth;
mod json;
pub mod me;

/// What `/me` and the admin routes need.
#[derive(Clone)]
pub struct IdentityState {
    inner: Arc<IdentityInner>,
}

struct IdentityInner {
    users: PgUsers,
    admin: UserAdmin,
    presence: Arc<dyn GithubTokenPresence>,
}

impl std::fmt::Debug for IdentityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityState").finish_non_exhaustive()
    }
}

impl IdentityState {
    /// `presence` answers "does this user have a stored GitHub token?" (B12) without reading a ciphertext.
    pub fn new(users: PgUsers, admin: UserAdmin, presence: Arc<dyn GithubTokenPresence>) -> Self {
        Self {
            inner: Arc::new(IdentityInner {
                users,
                admin,
                presence,
            }),
        }
    }

    pub(crate) fn users(&self) -> &PgUsers {
        &self.inner.users
    }

    pub(crate) fn admin(&self) -> &UserAdmin {
        &self.inner.admin
    }

    pub(crate) fn presence(&self) -> &dyn GithubTokenPresence {
        self.inner.presence.as_ref()
    }
}

/// Register the sign-in routes, `/me` and the admin routes on `guarded`.
#[must_use]
pub fn identity_routes(guarded: GuardedRouter, auth: auth::AuthState, state: IdentityState) -> GuardedRouter {
    let guarded = auth::register(guarded, auth);
    let guarded = me::register(guarded, state.clone());
    admin::register(guarded, state)
}

pub(crate) fn bad_request(id: Option<&RequestId>) -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "bad-request",
        "The request is not valid",
        id,
    )
}

pub(crate) fn unavailable(id: Option<&RequestId>) -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Service temporarily unavailable",
        id,
    )
}

/// The answer for a failed user or access-request operation. Rule violations carry the rule's own sentence as `detail`,
/// because the admin needs to know which rule stopped them; everything else is generic.
pub(crate) fn access_problem(e: AccessError, id: Option<&RequestId>) -> Response {
    match e {
        AccessError::NotFound => problem(StatusCode::NOT_FOUND, "not-found", "Not found", id),
        AccessError::AlreadyDecided | AccessError::RequestPending => problem(
            StatusCode::CONFLICT,
            "conflict",
            "The request conflicts with the current state",
            id,
        ),
        AccessError::Cooldown => problem(
            StatusCode::TOO_MANY_REQUESTS,
            "too-many-requests",
            "Too many requests, try again later",
            id,
        ),
        AccessError::BadCursor => bad_request(id),
        AccessError::Rule(rule) => {
            let (status, kind, title) = match rule {
                RuleError::EmptyChange => return bad_request(id),
                // The state of the system forbids it.
                RuleError::LastActiveAdmin
                | RuleError::TargetDisabled
                | RuleError::NotActive
                | RuleError::SameRole => (
                    StatusCode::CONFLICT,
                    "conflict",
                    "The request conflicts with the current state",
                ),
                // The change, as asked, is not allowed.
                RuleError::SelfRoleChange
                | RuleError::SelfStatusChange
                | RuleError::SelfFlagChange
                | RuleError::CannotMoveToPending
                | RuleError::ActiveNeedsRole => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "unprocessable",
                    "The change is not allowed",
                ),
            };
            // `RuleError`'s messages are fixed sentences without input (see `users::rules`).
            problem_with_detail(status, kind, title, &rule.to_string(), id)
        }
        AccessError::Audit(_) | AccessError::Bus(_) | AccessError::Unavailable => {
            tracing::warn!(error = %e, request_id = id.map(ToString::to_string), "identity operation failed");
            unavailable(id)
        }
    }
}
