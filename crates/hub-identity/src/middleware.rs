//! The authentication layer (T3; S2, S4, P7): from the session cookie to the user, on every request.
//!
//! [`AuthLayer`] reads the `__Host-lk_session` cookie, asks [`crate::session::Sessions`] for the live session (the cache
//! answers in microseconds; a miss is one indexed read), and puts an [`AuthContext`] in the request extensions. It also gives
//! every request a fresh [`RequestId`], which it puts in the extensions and in the `X-Request-Id` response header. An id sent
//! by the client is ignored, so the id in an audit event can be trusted to be ours.
//!
//! The layer decides only who the caller is, never what they may do:
//!
//! - no cookie, an unreadable cookie (duplicated, oversize, malformed), an unknown, expired or idle session: no context, as
//!   if the caller had sent nothing. The route guard (T4) answers 401, except on the few public routes.
//! - a pending or disabled user does get a context, with their status: the guard answers 403 and says which kind (D36).
//! - the database failing is `503`, not "anonymous": a lookup that cannot be answered must not look like "not signed in".
//!
//! Put it outside [`crate::csrf::CsrfLayer`], which needs the context.

use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::Response;
use domain::{RequestId, Role, UserId, UserStatus};
use futures::future::BoxFuture;
use tower::{Layer, Service};

use crate::problem::problem;
use crate::session::Sessions;
use crate::session::cookie::{RequestCookies, TokenHash};

/// The response header that carries the request id.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Who the caller is, from a live session. Injected into the request extensions by [`AuthLayer`].
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user: UserId,
    /// `None` is the "none" role: signed in, granted nothing yet.
    pub role: Option<Role>,
    pub status: UserStatus,
    /// The hash of the session id, to end this very session (logout).
    pub session_hash: TokenHash,
    /// The hash of the CSRF token the session was made with.
    pub csrf_hash: TokenHash,
    pub request_id: Option<RequestId>,
}

/// A request id made by the hub, never taken from the client. It is a correlation id for logs and audit events, not a secret: it
/// grants nothing, so it comes from `fastrand` (fast, not cryptographically secure) and is only made unlikely to repeat. Anything
/// that must be unguessable (session ids, CSRF tokens) comes from the aws-lc-rs generator, in `session::cookie`. `None` only if the
/// identifier were somehow refused, which it cannot be.
fn new_request_id() -> Option<RequestId> {
    RequestId::parse(&format!("req-{:032x}", fastrand::u128(..))).ok()
}

#[derive(Debug, Clone)]
pub struct AuthLayer {
    sessions: Arc<Sessions>,
}

impl AuthLayer {
    pub fn new(sessions: Arc<Sessions>) -> Self {
        Self { sessions }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            sessions: Arc::clone(&self.sessions),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthService<S> {
    inner: S,
    sessions: Arc<Sessions>,
}

impl<S> Service<Request<Body>> for AuthService<S>
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        // `poll_ready` was called on `self.inner`; the clone is the fresh one, so swap them.
        let ready = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, ready);
        let sessions = Arc::clone(&self.sessions);
        Box::pin(async move {
            let request_id = new_request_id();
            if let Some(id) = &request_id {
                req.extensions_mut().insert(id.clone());
            }
            let cookie = session_cookie(&req);
            let context = match cookie {
                None => None,
                Some(value) => match sessions.authenticate_hashed(&value).await {
                    Ok(found) => found.map(|(hash, s)| AuthContext {
                        user: s.user.clone(),
                        role: s.role,
                        status: s.status,
                        session_hash: hash,
                        csrf_hash: s.csrf_hash,
                        request_id: request_id.clone(),
                    }),
                    Err(e) => {
                        tracing::warn!(error = %e, request_id = request_id.as_ref().map(ToString::to_string), "session lookup failed");
                        let mut response = problem(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "unavailable",
                            "Service temporarily unavailable",
                            request_id.as_ref(),
                        );
                        stamp(&mut response, request_id.as_ref());
                        return Ok(response);
                    }
                },
            };
            if let Some(context) = context {
                req.extensions_mut().insert(context);
            }
            let mut response = inner.call(req).await?;
            stamp(&mut response, request_id.as_ref());
            Ok(response)
        })
    }
}

/// The session cookie's value, if the request carries exactly one and its cookies parse.
fn session_cookie(req: &Request<Body>) -> Option<String> {
    let headers = req.headers().get_all(header::COOKIE);
    headers.iter().next()?;
    // A header that is not text is not a cookie header any browser sends: treat the request as having none.
    let parsed = RequestCookies::parse(headers.iter().filter_map(|v| v.to_str().ok()));
    match parsed {
        Ok(cookies) => cookies.session,
        Err(e) => {
            tracing::debug!(error = %e, "cookie header refused");
            None
        }
    }
}

fn stamp(response: &mut Response, id: Option<&RequestId>) {
    if let Some(value) = id.and_then(|id| HeaderValue::from_str(id.as_str()).ok()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}
