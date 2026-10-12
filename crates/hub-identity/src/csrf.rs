//! CSRF protection (T3; S3): a double-submit token, bound to the session, plus the `Origin` check, on every state-changing
//! request that carries a session.
//!
//! For a request whose method is not GET, HEAD or OPTIONS, and which carries a live session, [`CsrfLayer`] requires all of:
//!
//! 1. exactly one `Origin` header, equal to the configured public origin (a missing header, `null`, another scheme, host or
//!    port, or a second `Origin` header is refused);
//! 2. exactly one `X-CSRF-Token` header and exactly one `__Host-lk_csrf` cookie, with equal values (a constant-time
//!    comparison);
//! 3. the SHA-256 of that token equals the hash stored with the session (a constant-time comparison). This is what makes the
//!    double submit safe against a cookie planted from a sibling subdomain: the planted pair matches itself, not the session.
//!
//! A request with no live session carries no ambient authority, so there is nothing for CSRF to protect: the layer lets it
//! through and the route guard answers 401 (or, for the few public routes, the route does its own authentication).
//!
//! A route authenticated some other way (the GitHub webhook, by an HMAC over the body) is registered with
//! [`CsrfPolicy::Exempt`], which needs a [`CsrfExemption`]: a reason, in the code, that a reviewer reads.
//!
//! The layer must sit after [`crate::middleware::AuthLayer`], which supplies the session. The refusal is always the same
//! `403` problem, so an attacker does not learn which check failed; the variant is logged.

use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::Response;
use futures::future::BoxFuture;
use subtle::{Choice, ConstantTimeEq};
use tower::{Layer, Service};

use crate::middleware::AuthContext;
use crate::problem::problem;
use crate::session::cookie::{CSRF_HEADER, RandomToken, RequestCookies, TokenHash};

/// A reason shorter than this is not a reason.
const MIN_REASON_BYTES: usize = 10;

/// Why a CSRF exemption was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExemptionError {
    #[error("a CSRF exemption needs a reason of at least {MIN_REASON_BYTES} characters")]
    NoReason,
}

/// The justification for a route that skips the CSRF check. It cannot be made without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsrfExemption {
    reason: &'static str,
}

impl CsrfExemption {
    /// `reason` says how the route is authenticated instead (for example "HMAC over the raw body").
    pub fn new(reason: &'static str) -> Result<Self, ExemptionError> {
        if reason.trim().len() < MIN_REASON_BYTES {
            return Err(ExemptionError::NoReason);
        }
        Ok(Self { reason })
    }

    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// What the layer does for a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfPolicy {
    Required,
    Exempt(CsrfExemption),
}

/// Why a request was refused. Logged, never returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    MissingOrigin,
    ForeignOrigin,
    MissingToken,
    CookieMismatch,
    UnboundToken,
}

/// The methods that do not change state. Everything else is checked, including methods this app does not serve.
pub fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether two tokens' texts are equal, in constant time. Returns a [`Choice`], not a `bool`, so that nobody combines the
/// result with a short-circuiting operator (the test for it is a type-level one).
pub fn tokens_match(a: &[u8], b: &[u8]) -> Choice {
    a.ct_eq(b)
}

/// Whether two token hashes are equal, in constant time.
pub fn hashes_match(a: &TokenHash, b: &TokenHash) -> Choice {
    a.as_bytes().ct_eq(b.as_bytes())
}

/// Judge one request. `session_csrf` is the hash of the CSRF token of the live session the auth layer found, if there was
/// one (`AuthContext::csrf_hash`). `public_origin` is lower case with no trailing slash.
pub fn check(
    method: &Method,
    headers: &HeaderMap,
    public_origin: &str,
    session_csrf: Option<&TokenHash>,
) -> Result<(), Refusal> {
    if is_safe_method(method) {
        return Ok(());
    }
    let Some(session_csrf) = session_csrf else {
        return Ok(());
    };

    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin = origins.next().ok_or(Refusal::MissingOrigin)?;
    if origins.next().is_some() || origin.as_bytes() != public_origin.as_bytes() {
        return Err(Refusal::ForeignOrigin);
    }

    let mut tokens = headers.get_all(CSRF_HEADER).iter();
    let sent = tokens.next().ok_or(Refusal::MissingToken)?;
    if tokens.next().is_some() {
        return Err(Refusal::MissingToken);
    }
    let sent = sent.to_str().map_err(|_| Refusal::MissingToken)?;
    let cookies = RequestCookies::parse(
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()),
    )
    .map_err(|_| Refusal::CookieMismatch)?;
    let cookie = cookies.csrf.ok_or(Refusal::MissingToken)?;
    if !bool::from(tokens_match(sent.as_bytes(), cookie.as_bytes())) {
        return Err(Refusal::CookieMismatch);
    }
    let token = RandomToken::from_text(sent).ok_or(Refusal::CookieMismatch)?;
    if !bool::from(hashes_match(&token.hash(), session_csrf)) {
        return Err(Refusal::UnboundToken);
    }
    Ok(())
}

/// The CSRF layer. `public_origin` is `https://host[:port]`, as the configuration holds it.
#[derive(Debug, Clone)]
pub struct CsrfLayer {
    origin: Arc<str>,
    policy: CsrfPolicy,
}

impl CsrfLayer {
    pub fn new(public_origin: &str, policy: CsrfPolicy) -> Self {
        // Browsers send the origin in lower case with no trailing slash.
        Self {
            origin: Arc::from(public_origin.trim_end_matches('/').to_ascii_lowercase()),
            policy,
        }
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CsrfService {
            inner,
            origin: Arc::clone(&self.origin),
            policy: self.policy,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CsrfService<S> {
    inner: S,
    origin: Arc<str>,
    policy: CsrfPolicy,
}

impl<S> Service<Request<Body>> for CsrfService<S>
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

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        if matches!(self.policy, CsrfPolicy::Required) {
            let session = req.extensions().get::<AuthContext>();
            if let Err(refusal) = check(
                req.method(),
                req.headers(),
                &self.origin,
                session.map(|s| &s.csrf_hash),
            ) {
                let request_id = session.and_then(|s| s.request_id.as_ref());
                tracing::warn!(
                    ?refusal,
                    request_id = request_id.map(ToString::to_string),
                    "state-changing request refused"
                );
                let response = problem(StatusCode::FORBIDDEN, "csrf", "Request not allowed", request_id);
                return Box::pin(std::future::ready(Ok(response)));
            }
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
    fn csrf_exemption_needs_a_reason() {
        for bad in ["", "   ", "short", "hmac"] {
            assert_eq!(CsrfExemption::new(bad), Err(ExemptionError::NoReason), "{bad:?}");
        }
        let ok = CsrfExemption::new("HMAC over the raw body, constant time").unwrap();
        assert_eq!(ok.reason(), "HMAC over the raw body, constant time");
    }

    #[test]
    fn csrf_comparison_is_constant_time_by_construction() {
        // The comparisons return `subtle::Choice`: a value that cannot be branched on without an explicit conversion, so a
        // short-circuiting `==` on token bytes does not type-check in their place.
        let _: fn(&[u8], &[u8]) -> Choice = tokens_match;
        let _: fn(&TokenHash, &TokenHash) -> Choice = hashes_match;
        assert!(bool::from(tokens_match(b"abc", b"abc")));
        assert!(!bool::from(tokens_match(b"abc", b"abd")));
        assert!(!bool::from(tokens_match(b"abc", b"ab")));
        // And the module never compares secret bytes with `==` in non-test code.
        let source = include_str!("csrf.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for line in production.lines().filter(|l| !l.trim_start().starts_with("//")) {
            assert!(
                !(line.contains("hash ==")
                    || line.contains("hash !=")
                    || line.contains("sent ==")
                    || line.contains("cookie ==")),
                "a secret is compared with ==: {line}"
            );
        }
    }

    #[test]
    fn safe_methods_are_exactly_get_head_options() {
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(is_safe_method(&m));
        }
        for m in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::TRACE,
            Method::CONNECT,
        ] {
            assert!(!is_safe_method(&m), "{m}");
        }
        assert!(!is_safe_method(&Method::from_bytes(b"PROPFIND").unwrap()));
    }

    #[test]
    fn the_layer_normalises_the_configured_origin() {
        let layer = CsrfLayer::new("https://LaneKeeper.example.com/", CsrfPolicy::Required);
        assert_eq!(&*layer.origin, "https://lanekeeper.example.com");
    }
}
