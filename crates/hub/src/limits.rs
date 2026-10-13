//! Request limits (T1, S11, rule 5): per-IP and per-user rate limits, a body cap, a JSON depth cap and a timeout.
//!
//! Order, cheapest first: the per-IP bucket (a flood is refused before anything is read), the declared `Content-Length`, the
//! body itself (read as it arrives and cut at the cap, so a chunked body without a length is bounded too), and a deadline
//! over the whole exchange. The per-user bucket is applied after authentication, by the route guard, through
//! [`UserThrottle`]; the user is not known here.
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
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use domain::UserId;
use http::{HeaderValue, header};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use hub_platform::{ApiError, UserThrottle};
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

/// Everything the limits layer needs.
#[derive(Debug)]
pub struct Limits {
    settings: LimitSettings,
    proxies: TrustedProxies,
    ip: RateLimiter<IpAddr>,
    user: RateLimiter<UserId>,
}

impl Limits {
    #[must_use]
    pub fn new(settings: &LimitSettings, proxies: TrustedProxies) -> Arc<Self> {
        Arc::new(Self {
            ip: RateLimiter::new(settings.per_ip, settings.bucket_capacity, settings.bucket_idle),
            user: RateLimiter::new(settings.per_user, settings.bucket_capacity, settings.bucket_idle),
            settings: settings.clone(),
            proxies,
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

/// The limits layer: per-IP bucket, body cap, JSON depth, timeout.
pub async fn request_limits(State(limits): State<Arc<Limits>>, request: Request, next: Next) -> Response {
    let ip = limits
        .proxies
        .client_ip(request.headers(), request.extensions().get::<PeerAddr>().copied());
    if let Err(retry) = limits.ip.check(ip, Instant::now()) {
        return too_many(retry);
    }
    let max = limits.settings.body_max;
    if declared_length(&request).is_some_and(|len| len > max as u64) {
        return ApiError::PayloadTooLarge.into_response();
    }
    let work = async {
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
    };
    match tokio::time::timeout(limits.settings.request_timeout, work).await {
        Ok(response) => response,
        Err(_) => ApiError::RequestTimeout.into_response(),
    }
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
