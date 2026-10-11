//! [`GuardedRouter`]: the one way to add a route (S4).
//!
//! Layer order for a route, outside in: the auth layer (once, around the whole router) turns the cookie into a caller; the
//! CSRF layer refuses a state-changing request that carries a session without a matching token and `Origin`; the role gate
//! answers `401` or `403` before the handler; the handler. CSRF comes before the role gate so that a cross-site request is
//! refused as such whatever the victim's role, and learns nothing about it.
//!
//! Every registration problem (a bad path, a route twice, a public route without a reason) is remembered and returned by
//! [`GuardedRouter::into_router`]. None of them panics, although axum would for a bad or overlapping path.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::handler::Handler;
use axum::http::Request as HttpRequest;
use axum::response::Response;
use axum::routing::{MethodRouter, delete, get, patch, post, put};
use domain::{RequestId, Role};
use futures::future::BoxFuture;
use tower::{Layer, Service};

use super::problem::{denied, not_found};
use super::{Denied, authorize_status};
use crate::csrf::{CsrfLayer, CsrfPolicy};
use crate::middleware::{AuthContext, AuthLayer};
use crate::session::Sessions;

/// A reason shorter than this is not a reason (the same bar as a CSRF exemption).
const MIN_REASON_BYTES: usize = 10;
const MAX_PATH_BYTES: usize = 256;

/// The HTTP methods the hub serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Verb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// Who may call a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// An active user whose role is at least this.
    Role(Role),
    /// Anyone, by design, for the stated reason (sign-in, a webhook that checks its own HMAC).
    Public { reason: &'static str },
}

/// One entry of the route table: what the S4 test compares with `api/openapi.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// Upper case.
    pub method: String,
    /// As registered, with `{name}` parameters.
    pub path: String,
    pub access: Access,
}

/// Why a registration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// Empty, not starting with `/`, an empty segment, a malformed `{parameter}`, or characters outside the safe set.
    #[error("route path is not valid")]
    BadPath,
    /// The same method and path twice, or the same path shape under another parameter name.
    #[error("route is registered twice")]
    Duplicate,
    #[error("a public route needs a reason of at least {MIN_REASON_BYTES} characters")]
    PublicWithoutReason,
}

fn valid_segment(seg: &str) -> bool {
    if let Some(inner) = seg.strip_prefix('{') {
        return inner.strip_suffix('}').is_some_and(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        });
    }
    !seg.is_empty()
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

fn valid_path(path: &str) -> bool {
    if path.len() > MAX_PATH_BYTES || !path.starts_with('/') {
        return false;
    }
    path == "/" || path[1..].split('/').all(valid_segment)
}

/// The shape of a path: parameters reduced to `{}`, so `/a/{id}` and `/a/{name}` are the same route to axum.
fn shape(path: &str) -> String {
    path.split('/')
        .map(|seg| if seg.starts_with('{') { "{}" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// A router in which every route has a declared guard.
pub struct GuardedRouter {
    router: Router,
    table: Vec<RouteEntry>,
    /// Shape of each registered path to the spelling that registered it.
    spellings: BTreeMap<String, String>,
    origin: String,
    sessions: Arc<Sessions>,
    error: Option<RegisterError>,
}

impl std::fmt::Debug for GuardedRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedRouter")
            .field("routes", &self.table.len())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

macro_rules! role_route {
    ($($(#[$doc:meta])* $name:ident => $verb:expr),+ $(,)?) => {$(
        $(#[$doc])*
        #[must_use]
        pub fn $name<H, T, S>(self, path: &str, role: Role, state: S, handler: H) -> Self
        where
            H: Handler<T, S>,
            T: 'static,
            S: Clone + Send + Sync + 'static,
        {
            self.add($verb, path, Access::Role(role), CsrfPolicy::Required, state, handler)
        }
    )+};
}

impl GuardedRouter {
    /// `public_origin` is `https://host[:port]`, the one `Origin` the CSRF layer accepts.
    pub fn new(sessions: Arc<Sessions>, public_origin: &str) -> Self {
        Self {
            router: Router::new(),
            table: Vec::new(),
            spellings: BTreeMap::new(),
            origin: public_origin.to_owned(),
            sessions,
            error: None,
        }
    }

    role_route! {
        /// A `GET` route for an active user with at least `role`. `state` is handed to the handler.
        get => Verb::Get,
        post => Verb::Post,
        put => Verb::Put,
        patch => Verb::Patch,
        delete => Verb::Delete,
    }

    /// A route that anyone may call, for the stated `reason`. `csrf` says how a state-changing request that carries a
    /// session is checked: [`CsrfPolicy::Required`] unless the route is authenticated another way (a signed webhook).
    #[must_use]
    pub fn public<H, T, S>(
        self,
        verb: Verb,
        path: &str,
        reason: &'static str,
        csrf: CsrfPolicy,
        state: S,
        handler: H,
    ) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
        S: Clone + Send + Sync + 'static,
    {
        self.add(verb, path, Access::Public { reason }, csrf, state, handler)
    }

    fn add<H, T, S>(
        mut self,
        verb: Verb,
        path: &str,
        access: Access,
        csrf: CsrfPolicy,
        state: S,
        handler: H,
    ) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
        S: Clone + Send + Sync + 'static,
    {
        if self.error.is_some() {
            return self;
        }
        if let Err(e) = self.register(verb, path, &access) {
            self.error = Some(e);
            return self;
        }
        let method_router: MethodRouter = match verb {
            Verb::Get => get(handler).with_state(state),
            Verb::Post => post(handler).with_state(state),
            Verb::Put => put(handler).with_state(state),
            Verb::Patch => patch(handler).with_state(state),
            Verb::Delete => delete(handler).with_state(state),
        };
        let method_router = match access {
            Access::Role(min) => method_router.layer(RoleGateLayer { min }),
            Access::Public { .. } => method_router,
        }
        .layer(CsrfLayer::new(&self.origin, csrf));
        self.router = self.router.route(path, method_router);
        self.table.push(RouteEntry {
            method: verb.as_str().to_owned(),
            path: path.to_owned(),
            access,
        });
        self
    }

    /// The checks that must pass before axum is asked to take the route, because axum would panic.
    fn register(&mut self, verb: Verb, path: &str, access: &Access) -> Result<(), RegisterError> {
        if let Access::Public { reason } = access {
            if reason.trim().len() < MIN_REASON_BYTES {
                return Err(RegisterError::PublicWithoutReason);
            }
        }
        if !valid_path(path) {
            return Err(RegisterError::BadPath);
        }
        if self
            .table
            .iter()
            .any(|r| r.method == verb.as_str() && shape(&r.path) == shape(path))
        {
            return Err(RegisterError::Duplicate);
        }
        match self.spellings.get(&shape(path)) {
            Some(spelling) if spelling != path => return Err(RegisterError::Duplicate),
            Some(_) => {}
            None => {
                self.spellings.insert(shape(path), path.to_owned());
            }
        }
        Ok(())
    }

    /// Every route registered so far.
    pub fn route_table(&self) -> &[RouteEntry] {
        &self.table
    }

    /// The finished router: the auth layer around everything, and a fallback that answers `401` (or `403`) before it says
    /// `404`. Returns the first registration error, if there was one.
    ///
    /// Do not add routes to the result with `Router::route`: a route added afterwards has no auth layer and is not in the
    /// table. Add them here.
    pub fn into_router(self) -> Result<Router, RegisterError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        Ok(self
            .router
            .fallback(unmatched)
            .layer(AuthLayer::new(self.sessions)))
    }
}

/// A path nobody registered. Without a session the answer is `401`, so that the site's layout is not revealed to someone who
/// is not signed in; pending and disabled users get their `403`; an active user gets a plain `404`.
async fn unmatched(req: Request) -> Response {
    let request_id = req.extensions().get::<RequestId>();
    match req.extensions().get::<AuthContext>() {
        None => denied(Denied::Unauthenticated, request_id),
        Some(ctx) => match authorize_status(ctx.status, ctx.role, Role::Viewer) {
            Ok(()) => not_found(request_id),
            Err(d) => denied(d, request_id),
        },
    }
}

/// Answers `401` or `403` unless the caller is an active user with at least `min`.
#[derive(Debug, Clone, Copy)]
struct RoleGateLayer {
    min: Role,
}

impl<S> Layer<S> for RoleGateLayer {
    type Service = RoleGate<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RoleGate { inner, min: self.min }
    }
}

#[derive(Debug, Clone)]
struct RoleGate<S> {
    inner: S,
    min: Role,
}

impl<S> Service<HttpRequest<Body>> for RoleGate<S>
where
    S: Service<HttpRequest<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest<Body>) -> Self::Future {
        let verdict = match req.extensions().get::<AuthContext>() {
            None => Err(Denied::Unauthenticated),
            Some(ctx) => authorize_status(ctx.status, ctx.role, self.min),
        };
        if let Err(d) = verdict {
            tracing::debug!(?d, "role gate refused the caller");
            let response = denied(d, req.extensions().get::<RequestId>());
            return Box::pin(std::future::ready(Ok(response)));
        }
        // `poll_ready` was called on `self.inner`; the clone is the fresh one, so swap them.
        let ready = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, ready);
        Box::pin(inner.call(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_checked_the_way_axum_needs() {
        for good in [
            "/",
            "/a",
            "/a/b",
            "/a/{id}",
            "/a/{id}/approve",
            "/api/v1/docs/fs/read",
            "/a-b/c_d/e.f",
        ] {
            assert!(valid_path(good), "{good}");
        }
        for bad in [
            "", "a", "/a/", "/a//b", "/a/{}", "/a/{x", "/a/x}", "/a/{*r}", "/a/*", "/a b", "/a?b",
            "/a/{id}x", "/ä", "/a/{Id}",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&format!("/{}", "a".repeat(MAX_PATH_BYTES))));
    }

    #[test]
    fn shapes_ignore_parameter_names() {
        assert_eq!(shape("/a/{id}/b"), shape("/a/{name}/b"));
        assert_ne!(shape("/a/{id}"), shape("/a/b"));
    }
}
