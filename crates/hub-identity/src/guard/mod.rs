//! Authorization (T4; S4): every route declares who may call it, and the declaration is enforced, not remembered.
//!
//! - [`GuardedRouter`] is the only way to add a route. Its `get`, `post`, `put`, `patch` and `delete` take the minimum
//!   [`Role`] as a parameter; `public` takes a reason and a CSRF policy. It applies the CSRF layer and the role gate to each
//!   route, the auth layer to the whole router, and a default-deny fallback, so a path nobody registered is `401` and not `404`.
//!   It keeps the route table that the S4 test compares with `x-required-role` in `api/openapi.yaml`.
//! - [`RequireRole`] and [`ActiveUser`] are extractors that check the same rule again from inside the handler (defence in
//!   depth). A handler that asks for more than its route declared can only make the route stricter.
//! - [`authorize`] is the one rule: pending and disabled users never pass, whatever their role (D36); an active user passes
//!   when their role is at least the minimum; an active user with no role passes nothing.
//!
//! Decision B4: the prompt names `#[guarded(role = ...)]`. An attribute macro needs a proc-macro crate outside this prompt's
//! paths, so the declaration is a parameter of the registration function instead. What the macro would add (a route added
//! with no declaration) cannot be written with this API, and `testing::assert_default_deny` closes the remaining gap by
//! calling every registered path without a cookie.

use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::Response;
use domain::{RequestId, Role, User, UserStatus};

use crate::middleware::AuthContext;

pub mod problem;
pub mod router;

pub use router::{Access, GuardedRouter, RegisterError, RouteEntry, Verb};

/// Why a caller was turned away. Each one is a different problem type, so the web app can tell a user who is waiting for
/// approval from one who is disabled (B16) without a second request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    #[error("not signed in")]
    Unauthenticated,
    #[error("access is pending approval")]
    Pending,
    #[error("account is disabled")]
    Disabled,
    #[error("role is not sufficient")]
    InsufficientRole,
}

/// The rule behind every guard. Takes the parts of the caller it needs, so it can be tested without a user record.
pub fn authorize_status(status: UserStatus, role: Option<Role>, min: Role) -> Result<(), Denied> {
    match status {
        UserStatus::Pending => Err(Denied::Pending),
        UserStatus::Disabled => Err(Denied::Disabled),
        UserStatus::Active => match role {
            Some(r) if r.at_least(min) => Ok(()),
            _ => Err(Denied::InsufficientRole),
        },
    }
}

/// [`authorize_status`] for a user record.
pub fn authorize(user: &User, min: Role) -> Result<(), Denied> {
    authorize_status(user.status, user.role, min)
}

mod sealed {
    pub trait Sealed {}
}

/// A role as a type, for [`RequireRole`]. Sealed: the four roles are all there are.
pub trait RoleMarker: sealed::Sealed + Send + Sync + 'static {
    const ROLE: Role;
}

macro_rules! role_marker {
    ($($(#[$doc:meta])* $name:ident => $role:expr),+ $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl RoleMarker for $name {
            const ROLE: Role = $role;
        }
    )+};
}

role_marker! {
    /// At least `Viewer`: any active user with a role.
    Viewer => Role::Viewer,
    Editor => Role::Editor,
    Operator => Role::Operator,
    Admin => Role::Admin,
}

/// The caller, if they are an active user whose role is at least `R`. Anything else is answered before the handler runs:
/// `401` with no session, `403` with a problem type that says why otherwise.
#[derive(Debug, Clone)]
pub struct RequireRole<R: RoleMarker> {
    pub context: AuthContext,
    _role: PhantomData<fn() -> R>,
}

/// The caller, if they are an active user with any role. The same as `RequireRole<Viewer>`, named for what handlers mean by it.
#[derive(Debug, Clone)]
pub struct ActiveUser(pub AuthContext);

fn check(parts: &Parts, min: Role) -> Result<AuthContext, Denied> {
    let context = parts
        .extensions
        .get::<AuthContext>()
        .ok_or(Denied::Unauthenticated)?;
    authorize_status(context.status, context.role, min)?;
    Ok(context.clone())
}

/// The response for a refused extractor; also logs why, by variant.
fn refuse(parts: &Parts, d: Denied) -> Response {
    tracing::debug!(?d, "extractor refused the caller");
    problem::denied(d, parts.extensions.get::<RequestId>())
}

impl<R: RoleMarker, S: Send + Sync> FromRequestParts<S> for RequireRole<R> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match check(parts, R::ROLE) {
            Ok(context) => Ok(Self {
                context,
                _role: PhantomData,
            }),
            Err(d) => Err(refuse(parts, d)),
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ActiveUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match check(parts, Role::Viewer) {
            Ok(context) => Ok(Self(context)),
            Err(d) => Err(refuse(parts, d)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_name_the_four_roles() {
        assert_eq!(Viewer::ROLE, Role::Viewer);
        assert_eq!(Editor::ROLE, Role::Editor);
        assert_eq!(Operator::ROLE, Role::Operator);
        assert_eq!(Admin::ROLE, Role::Admin);
    }
}
