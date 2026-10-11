//! The one way to register a route (S4, rule 9): every route is declared with its guard, so the route table cannot
//! disagree with what is mounted, and a route without a guard cannot be written.
//!
//! A [`GuardedRouter`] wraps each handler in the guard middleware and records a [`RouteSpec`]. `hub` collects the specs of
//! every module into the route table that the S4 test checks against `api/openapi.yaml`.

use std::fmt;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};
use domain::Role;
use http::Method;
use ports::AuthError;

use crate::error::ApiError;
use crate::session_auth::{AuthenticatedUser, SessionAuth, UserThrottle};

/// How a registered route is protected. There is no "none" variant: an unguarded route cannot be declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteGuard {
    /// An active user with at least this role.
    Role(Role),
    /// Open by design, with the reason (the GitHub webhook proves itself with an HMAC).
    Public { reason: String },
}

/// One route of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    pub method: String,
    pub path: String,
    pub guard: RouteGuard,
}

impl RouteSpec {
    #[must_use]
    pub fn new(method: &Method, path: &str, guard: RouteGuard) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_owned(),
            guard,
        }
    }
}

/// `GET` server-sent events (T5).
pub const EVENTS_PATH: &str = "/api/v1/events";
/// `GET` a blob by content hash (T8).
pub const BLOB_PATH: &str = "/api/v1/blobs/{hash}";
/// `POST` the GitHub webhook (T7, T11).
pub const WEBHOOK_PATH: &str = "/hooks/github";

/// The routes this crate mounts once its tasks are done, each with its guard. The route-table test checks this list
/// against `api/openapi.yaml`, and each task that mounts a route checks the mounted table against this list, so a route
/// cannot be added, dropped or re-guarded without both agreeing (S4).
#[must_use]
pub fn platform_routes() -> Vec<RouteSpec> {
    vec![
        RouteSpec::new(&Method::GET, EVENTS_PATH, RouteGuard::Role(Role::Viewer)),
        RouteSpec::new(&Method::GET, BLOB_PATH, RouteGuard::Role(Role::Viewer)),
        RouteSpec::new(
            &Method::POST,
            WEBHOOK_PATH,
            RouteGuard::Public {
                reason:
                    "no session: verified by an HMAC over the raw body and a replay guard keyed on that HMAC"
                        .to_owned(),
            },
        ),
    ]
}

/// The routes of the operations listener. They are not API operations and are never on the web listener.
#[must_use]
pub fn ops_routes() -> Vec<RouteSpec> {
    ["/healthz", "/readyz", "/metrics"]
        .into_iter()
        .map(|path| {
            RouteSpec::new(
                &Method::GET,
                path,
                RouteGuard::Public {
                    reason: "internal listener only: no secrets, read-only, reachable by the kubelet and the scraper".to_owned(),
                },
            )
        })
        .collect()
}

#[cfg(any(test, feature = "conformance"))]
impl RouteSpec {
    /// The entry for `ports::conformance::route_guard`.
    #[must_use]
    pub fn registered(&self) -> ports::conformance::route_guard::RegisteredRoute {
        use ports::conformance::route_guard::{Guard, RegisteredRoute};
        let guard = match &self.guard {
            RouteGuard::Role(role) => Guard::Role(*role),
            RouteGuard::Public { reason } => Guard::Public {
                reason: reason.clone(),
            },
        };
        RegisteredRoute::new(&self.method, &self.path, guard)
    }
}

/// A route that could not be registered. Found when the router is built, never at the first request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("{method} {path} is registered twice")]
    Duplicate { method: String, path: String },
    #[error("{method} is not a method a route may use")]
    Method { method: String },
    #[error("a public route needs a reason: {method} {path}")]
    PublicWithoutReason { method: String, path: String },
    #[error("a path must start with / and hold no whitespace: {path}")]
    Path { path: String },
}

#[derive(Clone)]
struct GuardState {
    auth: Arc<dyn SessionAuth>,
    throttle: Option<Arc<dyn UserThrottle>>,
    min: Role,
}

impl fmt::Debug for GuardState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardState")
            .field("min", &self.min)
            .finish_non_exhaustive()
    }
}

/// The guard: authenticate, check the role, count the request, then run the handler with the user in the extensions.
async fn guard(State(state): State<GuardState>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let user = match state.auth.authenticate(&parts).await {
        Ok(user) => user,
        Err(error) => return auth_failure(error).into_response(),
    };
    // The user's own record decides, whatever the implementation returned: pending and disabled users never pass (S4).
    if !user.allows(state.min) {
        return ApiError::Forbidden.into_response();
    }
    if let Some(throttle) = &state.throttle {
        if !throttle.allow(&user.id) {
            return ApiError::TooManyRequests.into_response();
        }
    }
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(AuthenticatedUser(user));
    next.run(request).await
}

fn auth_failure(error: AuthError) -> ApiError {
    match error {
        AuthError::InvalidToken | AuthError::Expired | AuthError::WrongAudience | AuthError::UnknownUser => {
            ApiError::Unauthorized
        }
        AuthError::Pending | AuthError::Disabled | AuthError::InsufficientRole => ApiError::Forbidden,
        AuthError::Unavailable => ApiError::Unavailable,
    }
}

fn method_filter(method: &Method) -> Option<MethodFilter> {
    MethodFilter::try_from(method.clone()).ok()
}

/// A router whose every route has a declared guard.
pub struct GuardedRouter<S = ()> {
    router: Router<S>,
    specs: Vec<RouteSpec>,
    errors: Vec<RouteError>,
    auth: Arc<dyn SessionAuth>,
    throttle: Option<Arc<dyn UserThrottle>>,
}

impl<S> fmt::Debug for GuardedRouter<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuardedRouter")
            .field("routes", &self.specs.len())
            .field("errors", &self.errors.len())
            .finish_non_exhaustive()
    }
}

impl<S> GuardedRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(auth: Arc<dyn SessionAuth>, throttle: Option<Arc<dyn UserThrottle>>) -> Self {
        Self {
            router: Router::new(),
            specs: Vec::new(),
            errors: Vec::new(),
            auth,
            throttle,
        }
    }

    fn check(&mut self, method: &Method, path: &str) -> Option<MethodFilter> {
        if !path.starts_with('/') || path.chars().any(char::is_whitespace) {
            self.errors.push(RouteError::Path {
                path: path.to_owned(),
            });
            return None;
        }
        let Some(filter) = method_filter(method) else {
            self.errors.push(RouteError::Method {
                method: method.to_string(),
            });
            return None;
        };
        if self
            .specs
            .iter()
            .any(|s| s.method == method.as_str() && s.path == path)
        {
            self.errors.push(RouteError::Duplicate {
                method: method.to_string(),
                path: path.to_owned(),
            });
            return None;
        }
        Some(filter)
    }

    /// Register `handler` for `method` and `path`, for active users with at least `role`.
    #[must_use]
    pub fn route<H, T>(mut self, method: &Method, path: &str, role: Role, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        let Some(filter) = self.check(method, path) else {
            return self;
        };
        let state = GuardState {
            auth: Arc::clone(&self.auth),
            throttle: self.throttle.clone(),
            min: role,
        };
        let layered = handler.layer(from_fn_with_state(state, guard));
        self.router = self.router.route(path, on(filter, layered));
        self.specs.push(RouteSpec {
            method: method.to_string(),
            path: path.to_owned(),
            guard: RouteGuard::Role(role),
        });
        self
    }

    /// Register a route that anyone may call, with the reason it is safe (the handler proves the caller itself).
    #[must_use]
    pub fn public<H, T>(mut self, method: &Method, path: &str, reason: &str, handler: H) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
    {
        if reason.trim().is_empty() {
            self.errors.push(RouteError::PublicWithoutReason {
                method: method.to_string(),
                path: path.to_owned(),
            });
            return self;
        }
        let Some(filter) = self.check(method, path) else {
            return self;
        };
        self.router = self.router.route(path, on(filter, handler));
        self.specs.push(RouteSpec {
            method: method.to_string(),
            path: path.to_owned(),
            guard: RouteGuard::Public {
                reason: reason.to_owned(),
            },
        });
        self
    }

    /// Merge another guarded router's routes into this one.
    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
        let Self {
            router,
            specs,
            errors,
            ..
        } = other;
        for spec in &specs {
            if self
                .specs
                .iter()
                .any(|s| s.method == spec.method && s.path == spec.path)
            {
                self.errors.push(RouteError::Duplicate {
                    method: spec.method.clone(),
                    path: spec.path.clone(),
                });
                return self;
            }
        }
        self.router = self.router.merge(router);
        self.specs.extend(specs);
        self.errors.extend(errors);
        self
    }

    /// The router and its table.
    ///
    /// # Errors
    ///
    /// The first registration that was not valid: a duplicate route, a method axum cannot route, a public route without a
    /// reason, or a path that does not start with `/`.
    pub fn into_parts(self) -> Result<(Router<S>, Vec<RouteSpec>), RouteError> {
        if let Some(error) = self.errors.into_iter().next() {
            return Err(error);
        }
        Ok((self.router, self.specs))
    }
}
