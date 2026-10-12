//! The answers of the guard: `application/problem+json` with a fixed type per reason and no detail from the request (S21).

use axum::http::StatusCode;
use axum::response::Response;
use domain::RequestId;

use super::Denied;
use crate::problem::problem;

/// The problem for a caller who was turned away. The type tells the web app which page to show; the title says nothing about
/// which role the route needs.
pub fn denied(d: Denied, request_id: Option<&RequestId>) -> Response {
    match d {
        Denied::Unauthenticated => problem(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "Sign in required",
            request_id,
        ),
        Denied::Pending => problem(
            StatusCode::FORBIDDEN,
            "access-pending",
            "Your access request is waiting for approval",
            request_id,
        ),
        Denied::Disabled => problem(
            StatusCode::FORBIDDEN,
            "access-disabled",
            "Your account is disabled",
            request_id,
        ),
        Denied::InsufficientRole => problem(
            StatusCode::FORBIDDEN,
            "insufficient-role",
            "You are not allowed to do this",
            request_id,
        ),
    }
}

/// A path nobody registered, for a caller who is signed in and active.
pub fn not_found(request_id: Option<&RequestId>) -> Response {
    problem(StatusCode::NOT_FOUND, "not-found", "Not found", request_id)
}
