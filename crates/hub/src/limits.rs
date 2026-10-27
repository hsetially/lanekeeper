//! Request limits (T1, T5; S11, rule 5; D137): per-IP and per-user rate limits, a body cap per route, a JSON depth cap and a timeout.
//!
//! There are two layers, one on each side of authentication, and the order is the point:
//!
//! 1. [`request_limits`], outside `AuthLayer`: the per-IP bucket (a flood is refused before anything is read, and before a cookie
//!    costs a database read), the **declared** `Content-Length` against the largest cap any caller could have on this route, and a
//!    deadline over the whole exchange. It reads nothing of the body.
//! 2. [`enforce_body_cap`], inside `AuthLayer` and `CsrfLayer`: the cap itself, which depends on who is asking, and the reading of
//!    the body as it arrives (so a chunked body without a length is bounded too). A request that `CsrfLayer` refuses never gets here,
//!    so a forged one buffers nothing.
//!
//! The cap is 4 MiB (S11) except where [`route_cap`] says otherwise: the GitHub webhook is 1 MiB, and the two edit routes
//! (`PUT /files`, `POST /drafts`) take [`MAX_EDIT_BODY`] of `hub-writes` **from a request that carries a live session** of at least
//! the route's role, and 4 MiB from everyone else (D137). An unauthenticated caller, a pending user and a Viewer are all "everyone
//! else", so the larger buffer is never something a stranger can ask the hub to hold. Bodies above the global cap are read at most
//! [`LARGE_BODIES_IN_FLIGHT`] at a time on a replica.
//!
//! The per-user bucket is applied after authentication, by the route guard, through [`UserThrottle`]; the user is not known here.
//!
//! JSON bodies are read whole (they are at most the cap) so their nesting depth can be checked before any parser sees them:
//! a parser's recursion limit is an error path, ours is a rejection. Other bodies stream through a length-limited wrapper,
//! and the handler that reads them gets an error it maps to 413 when the cap is crossed.
//!
//! The buckets live in bounded caches with a time-to-idle. A flood of new addresses evicts old buckets instead of growing
//! memory; the cost is that an attacker with many addresses can reset a bucket, which is why the per-IP limit is the coarse
//! control and the per-user one (keyed by an authenticated identity, not evictable by an address flood) is the fine one.

use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use domain::{Role, UserId, UserStatus};
use http::{HeaderValue, Method, header};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use hub_identity::middleware::AuthContext;
use hub_platform::routes::WEBHOOK_PATH;
use hub_platform::{ApiError, UserThrottle};
use hub_writes::routes::{DRAFTS_PATH, FILES_PATH, MAX_EDIT_BODY};
use tokio::sync::Semaphore;
use tokio::time::Instant;

use crate::client_ip::{PeerAddr, TrustedProxies};
use crate::config::{LimitSettings, Rate};

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A token-bucket limiter over a bounded table of keys.
pub struct RateLimiter<K> {
    buckets: moka::sync::Cache<K, Arc<Mutex<Bucket>>>,
    rate: Rate,
}

impl<K> std::fmt::Debug for RateLimiter<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl<K> RateLimiter<K>
where
    K: Hash + Eq + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(rate: Rate, capacity: u64, idle: Duration) -> Self {
        Self {
            buckets: moka::sync::Cache::builder()
                .max_capacity(capacity)
                .time_to_idle(idle)
                .build(),
            rate,
        }
    }

    /// Take a token for `key` at time `now`. `Err` carries how long until one is available.
    pub fn check(&self, key: K, now: Instant) -> Result<(), Duration> {
        let burst = f64::from(self.rate.burst);
        let bucket = self.buckets.get_with(key, || {
            Arc::new(Mutex::new(Bucket {
                tokens: burst,
                last: now,
            }))
        });
        let mut bucket = bucket.lock().unwrap_or_else(PoisonError::into_inner);
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate.per_second).min(burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / self.rate.per_second;
            Err(Duration::from_secs_f64(wait.clamp(0.0, 3600.0)))
        }
    }

    fn sync(&self) {
        self.buckets.run_pending_tasks();
    }

    fn len(&self) -> u64 {
        self.buckets.entry_count()
    }
}

/// The webhook's cap: GitHub's payloads for a push or a pull request are far smaller, and the route has no session to ask a larger
/// buffer of (S18).
pub const WEBHOOK_BODY_MAX: usize = 1024 * 1024;

/// How many bodies larger than the global cap may be read at once on one replica. One is up to [`MAX_EDIT_BODY`] (12 MiB), so
/// this bounds what the largest requests can hold at about 100 MiB however many sessions ask: the per-user bucket is applied by the
/// guard, after the body, and one Editor can open many connections.
pub const LARGE_BODIES_IN_FLIGHT: usize = 8;

/// The role a session must have for the larger cap of a route, and the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionCap {
    limit: usize,
    role: Role,
}

/// What a route's request body may weigh (S11, D137).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyCap {
    /// For every caller: the global cap, or a stricter one.
    base: usize,
    /// For a caller with a live session of at least `role`: more.
    session: Option<SessionCap>,
}

impl BodyCap {
    /// One cap for everybody.
    fn flat(limit: usize) -> Self {
        Self {
            base: limit,
            session: None,
        }
    }

    /// The most any caller could be given on this route: what the first layer lets a declared length claim.
    #[must_use]
    pub fn ceiling(&self) -> usize {
        self.session.map_or(self.base, |s| s.limit.max(self.base))
    }

    /// The cap for this caller. `session` is what `AuthLayer` found: the larger cap needs a session that is live and active, of a
    /// role the route accepts. Nothing else is looked at, and the guard still decides whether the request is allowed at all.
    #[must_use]
    pub fn for_caller(&self, session: Option<&AuthContext>) -> usize {
        match (self.session, session) {
            (Some(larger), Some(context))
                if context.status == UserStatus::Active
                    && context.role.is_some_and(|role| role.at_least(larger.role)) =>
            {
                larger.limit.max(self.base)
            }
            _ => self.base,
        }
    }
}

/// The caps of the routes that differ from the global one, keyed on the method and the matched template (a request that matched no
/// route has none, so it gets the global cap). `global` is `LK_BODY_MAX_BYTES`.
///
/// - `POST /hooks/github`: 1 MiB, for everybody. Stricter than the global cap, so it needs no session.
/// - `PUT /api/v1/swimlanes/{id}/files` and `POST /api/v1/drafts`: [`MAX_EDIT_BODY`] (a file of up to 2 MiB whose JSON escaping can
///   multiply it, plus headroom) for a live session of at least an Editor, which is the role the routes' guard asks for.
#[must_use]
pub fn route_cap(method: &Method, matched: Option<&str>, global: usize) -> BodyCap {
    match (method.as_str(), matched) {
        ("POST", Some(WEBHOOK_PATH)) => BodyCap::flat(global.min(WEBHOOK_BODY_MAX)),
        ("PUT", Some(FILES_PATH)) | ("POST", Some(DRAFTS_PATH)) => BodyCap {
            base: global,
            session: Some(SessionCap {
                limit: MAX_EDIT_BODY,
                role: Role::Editor,
            }),
        },
        _ => BodyCap::flat(global),
    }
}

fn matched_of(request: &Request) -> Option<&str> {
    request.extensions().get::<MatchedPath>().map(MatchedPath::as_str)
}

/// Everything the limits layers need.
#[derive(Debug)]
pub struct Limits {
    settings: LimitSettings,
    proxies: TrustedProxies,
    ip: RateLimiter<IpAddr>,
    user: RateLimiter<UserId>,
    /// One permit per body above the global cap that is being read or handled.
    large_bodies: Arc<Semaphore>,
}

impl Limits {
    #[must_use]
    pub fn new(settings: &LimitSettings, proxies: TrustedProxies) -> Arc<Self> {
        Arc::new(Self {
            ip: RateLimiter::new(settings.per_ip, settings.bucket_capacity, settings.bucket_idle),
            user: RateLimiter::new(settings.per_user, settings.bucket_capacity, settings.bucket_idle),
            settings: settings.clone(),
            proxies,
            large_bodies: Arc::new(Semaphore::new(LARGE_BODIES_IN_FLIGHT)),
        })
    }

    /// Run the caches' pending maintenance (for tests that count buckets).
    pub fn sync(&self) {
        self.ip.sync();
        self.user.sync();
    }

    /// How many per-IP buckets are held.
    #[must_use]
    pub fn ip_buckets(&self) -> u64 {
        self.ip.len()
    }

    /// How many per-user buckets are held.
    #[must_use]
    pub fn user_buckets(&self) -> u64 {
        self.user.len()
    }
}

impl UserThrottle for Limits {
    fn allow(&self, user: &UserId) -> bool {
        self.user.check(user.clone(), Instant::now()).is_ok()
    }
}

/// True if the JSON in `bytes` nests deeper than `max`. A scan, not a parse: brackets inside strings do not count, and a
/// malformed body is the parser's to reject. It never recurses, so a million open brackets cost one pass.
#[must_use]
pub fn json_depth_exceeds(bytes: &[u8], max: usize) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &b in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > max {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

fn is_json(request: &Request) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .is_some_and(|t| t == "application/json" || t.ends_with("+json"))
}

fn declared_length(request: &Request) -> Option<u64> {
    request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

fn too_many(retry: Duration) -> Response {
    let mut response = ApiError::TooManyRequests.into_response();
    let seconds = retry.as_secs_f64().ceil().max(1.0);
    // Whole seconds, at least 1; a float in this range always prints as an integer.
    if let Ok(value) = HeaderValue::from_str(&format!("{seconds:.0}")) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// The first limits layer, outside authentication: the per-IP bucket, the declared length against the largest cap the route has for
/// anybody, and the deadline over everything inside. It reads no body.
pub async fn request_limits(State(limits): State<Arc<Limits>>, request: Request, next: Next) -> Response {
    let ip = limits
        .proxies
        .client_ip(request.headers(), request.extensions().get::<PeerAddr>().copied());
    if let Err(retry) = limits.ip.check(ip, Instant::now()) {
        return too_many(retry);
    }
    let ceiling = route_cap(request.method(), matched_of(&request), limits.settings.body_max).ceiling();
    if declared_length(&request).is_some_and(|len| len > ceiling as u64) {
        return ApiError::PayloadTooLarge.into_response();
    }
    match tokio::time::timeout(limits.settings.request_timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::RequestTimeout.into_response(),
    }
}

/// The second limits layer, inside `AuthLayer` and `CsrfLayer`: the cap for *this* caller on *this* route, and the body read within
/// it. `AuthLayer` has already put what it found in the request, so a caller with no live session has the global cap, whatever the
/// route would give a session.
pub async fn enforce_body_cap(State(limits): State<Arc<Limits>>, request: Request, next: Next) -> Response {
    let global = limits.settings.body_max;
    let cap = route_cap(request.method(), matched_of(&request), global);
    let max = cap.for_caller(request.extensions().get::<AuthContext>());
    let declared = declared_length(&request);
    if declared.is_some_and(|len| len > max as u64) {
        return ApiError::PayloadTooLarge.into_response();
    }
    // A body that may be larger than the global cap is bounded in number: the per-user bucket comes after this, in the guard, so it
    // cannot do it. The permit is held while the request is handled, because the buffer is.
    let _large = if max > global && declared.is_none_or(|len| len > global as u64) {
        match Arc::clone(&limits.large_bodies).try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return too_many(Duration::from_secs(1)),
        }
    } else {
        None
    };
    let json = is_json(&request);
    let (parts, body) = request.into_parts();
    let request = if json {
        match Limited::new(body, max).collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                if json_depth_exceeds(&bytes, limits.settings.json_depth) {
                    return ApiError::BadRequest.into_response();
                }
                Request::from_parts(parts, Body::from(bytes))
            }
            Err(error) if error.downcast_ref::<LengthLimitError>().is_some() => {
                return ApiError::PayloadTooLarge.into_response();
            }
            Err(_) => return ApiError::BadRequest.into_response(),
        }
    } else {
        Request::from_parts(parts, Body::new(Limited::new(body, max)))
    };
    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn depth_is_counted_per_open_bracket() {
        assert!(!json_depth_exceeds(b"[[[1]]]", 3));
        assert!(json_depth_exceeds(b"[[[1]]]", 2));
        assert!(!json_depth_exceeds(br#"{"a":[{"b":1}]}"#, 3));
        assert!(json_depth_exceeds(br#"{"a":[{"b":1}]}"#, 2));
    }

    #[test]
    fn strings_and_escapes_hide_brackets() {
        assert!(!json_depth_exceeds(br#"["[[[[[[[["]"#, 1));
        assert!(!json_depth_exceeds(br#"["\"[[[[", "]]]]\\"]"#, 1));
    }

    #[test]
    fn siblings_do_not_accumulate_depth() {
        let body = format!("[{}]", vec!["[1]"; 1000].join(","));
        assert!(!json_depth_exceeds(body.as_bytes(), 2));
    }

    proptest! {
        #[test]
        fn nested_arrays_exceed_exactly_past_the_limit(depth in 0usize..200, max in 1usize..100) {
            let body = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            prop_assert_eq!(json_depth_exceeds(body.as_bytes(), max), depth > max);
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512), max in 0usize..70) {
            let _ = json_depth_exceeds(&bytes, max);
        }
    }

    use domain::Guid;
    use hub_identity::session::cookie::TokenHash;

    const MIB: usize = 1024 * 1024;

    fn session(role: Option<Role>, status: UserStatus) -> AuthContext {
        let user = UserId::new(
            Guid::parse("11111111-1111-4111-8111-111111111111").unwrap(),
            Guid::parse("22222222-2222-4222-8222-222222222222").unwrap(),
        );
        AuthContext {
            user,
            role,
            status,
            session_hash: TokenHash::from_bytes([1; 32]),
            csrf_hash: TokenHash::from_bytes([2; 32]),
            request_id: None,
        }
    }

    #[test]
    fn only_the_two_edit_routes_and_the_webhook_differ_from_the_global_cap() {
        let global = 4 * MIB;
        let cap = |method: Method, path: &str| route_cap(&method, Some(path), global);
        // The edit routes: a live session of an Editor or above gets 12 MiB + 64 KiB; nobody else gets more than the global cap.
        for (method, path) in [(Method::PUT, FILES_PATH), (Method::POST, DRAFTS_PATH)] {
            let cap = cap(method, path);
            assert_eq!(cap.ceiling(), MAX_EDIT_BODY);
            assert_eq!(MAX_EDIT_BODY, 12 * MIB + 64 * 1024);
            assert_eq!(cap.for_caller(None), global);
            for role in [Role::Editor, Role::Operator, Role::Admin] {
                assert_eq!(
                    cap.for_caller(Some(&session(Some(role), UserStatus::Active))),
                    MAX_EDIT_BODY
                );
            }
            assert_eq!(
                cap.for_caller(Some(&session(Some(Role::Viewer), UserStatus::Active))),
                global
            );
            assert_eq!(cap.for_caller(Some(&session(None, UserStatus::Active))), global);
            for status in [UserStatus::Pending, UserStatus::Disabled] {
                assert_eq!(
                    cap.for_caller(Some(&session(Some(Role::Admin), status))),
                    global,
                    "{status:?}"
                );
            }
        }
        // The webhook is stricter for everybody, a session or not.
        let webhook = cap(Method::POST, WEBHOOK_PATH);
        assert_eq!(webhook.ceiling(), WEBHOOK_BODY_MAX);
        assert_eq!(
            webhook.for_caller(Some(&session(Some(Role::Admin), UserStatus::Active))),
            MIB
        );
        // Everything else, including the same path under another method, a path nobody registered and a request that matched no
        // route, is the global cap for everybody.
        for (method, matched) in [
            (Method::POST, Some(FILES_PATH)),
            (Method::DELETE, Some(FILES_PATH)),
            (Method::PUT, Some(DRAFTS_PATH)),
            (Method::GET, Some(WEBHOOK_PATH)),
            (Method::POST, Some("/api/v1/swimlanes/{id}/restart")),
            (Method::POST, Some("/api/v1/swimlanes/{id}/files/upload")),
            (Method::POST, None),
        ] {
            let cap = route_cap(&method, matched, global);
            assert_eq!(cap.ceiling(), global, "{method} {matched:?}");
            assert_eq!(
                cap.for_caller(Some(&session(Some(Role::Admin), UserStatus::Active))),
                global,
                "{method} {matched:?}"
            );
        }
    }

    #[test]
    fn a_global_cap_set_above_the_edit_cap_is_never_lowered_and_one_below_the_webhook_is_kept() {
        // `LK_BODY_MAX_BYTES` goes to 64 MiB: the edit routes are never given less than everybody else, and the webhook is never
        // given more than it asks for.
        let big = 20 * MIB;
        let edit = route_cap(&Method::PUT, Some(FILES_PATH), big);
        assert_eq!(edit.ceiling(), big);
        assert_eq!(
            edit.for_caller(Some(&session(Some(Role::Editor), UserStatus::Active))),
            big
        );
        assert_eq!(
            route_cap(&Method::POST, Some(WEBHOOK_PATH), big).ceiling(),
            WEBHOOK_BODY_MAX
        );
        let small = 64 * 1024;
        assert_eq!(
            route_cap(&Method::POST, Some(WEBHOOK_PATH), small).ceiling(),
            small
        );
    }

    #[test]
    fn large_bodies_are_bounded_in_number_and_a_permit_comes_back_when_it_is_dropped() {
        let limits = Limits::new(&LimitSettings::default(), TrustedProxies::new(0));
        let held: Vec<_> = (0..LARGE_BODIES_IN_FLIGHT)
            .map(|_| Arc::clone(&limits.large_bodies).try_acquire_owned().unwrap())
            .collect();
        assert!(Arc::clone(&limits.large_bodies).try_acquire_owned().is_err());
        drop(held);
        assert!(Arc::clone(&limits.large_bodies).try_acquire_owned().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn a_bucket_refills_at_its_rate_and_never_beyond_the_burst() {
        let limiter = RateLimiter::<u8>::new(
            Rate {
                per_second: 2.0,
                burst: 3,
            },
            10,
            Duration::from_secs(60),
        );
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(limiter.check(1, t0).is_ok());
        }
        let wait = limiter.check(1, t0).unwrap_err();
        assert_eq!(wait, Duration::from_millis(500));
        assert!(limiter.check(1, t0 + Duration::from_millis(500)).is_ok());
        // A long idle period gives the burst, not more.
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..3 {
            assert!(limiter.check(1, later).is_ok());
        }
        assert!(limiter.check(1, later).is_err());
    }
}
