//! Request limits (T1, S11): the body cap, the time to a response head, the JSON depth and the two token buckets.

use std::time::Duration;

use super::ConfigError;
use super::reader::Reader;

/// How fast one client may call, as a token bucket: `per_second` refills, up to `burst` at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub per_second: f64,
    pub burst: u32,
}

/// Everything about request limits (T1). The defaults are the ones in the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitSettings {
    /// Largest request body (S11). Applies to chunked bodies too.
    pub body_max: usize,
    /// Longest time to produce a response head (the body of an SSE stream is not covered).
    pub request_timeout: Duration,
    /// Deepest JSON nesting a request body may have.
    pub json_depth: usize,
    pub per_ip: Rate,
    pub per_user: Rate,
    /// How many buckets each limiter keeps. A flood of new addresses evicts old buckets instead of growing memory.
    pub bucket_capacity: u64,
    /// A bucket nobody has used for this long is dropped.
    pub bucket_idle: Duration,
}

impl Default for LimitSettings {
    fn default() -> Self {
        Self {
            body_max: 4 * 1024 * 1024,
            request_timeout: Duration::from_secs(30),
            json_depth: 64,
            per_ip: Rate {
                per_second: 50.0,
                burst: 200,
            },
            per_user: Rate {
                per_second: 30.0,
                burst: 120,
            },
            bucket_capacity: 100_000,
            bucket_idle: Duration::from_secs(600),
        }
    }
}

impl LimitSettings {
    pub(super) fn read(reader: &Reader<'_>) -> Result<Self, ConfigError> {
        let defaults = Self::default();
        Ok(Self {
            body_max: reader.size(
                "LK_BODY_MAX_BYTES",
                defaults.body_max as u64,
                1024,
                64 * 1024 * 1024,
            )?,
            request_timeout: Duration::from_secs(reader.number(
                "LK_REQUEST_TIMEOUT_SECS",
                defaults.request_timeout.as_secs(),
                1,
                600,
            )?),
            json_depth: reader.size("LK_JSON_DEPTH", defaults.json_depth as u64, 1, 256)?,
            per_ip: Rate {
                per_second: reader.float("LK_IP_RATE_PER_SEC", defaults.per_ip.per_second)?,
                burst: reader.count("LK_IP_BURST", u64::from(defaults.per_ip.burst), 1, 100_000)?,
            },
            per_user: Rate {
                per_second: reader.float("LK_USER_RATE_PER_SEC", defaults.per_user.per_second)?,
                burst: reader.count("LK_USER_BURST", u64::from(defaults.per_user.burst), 1, 100_000)?,
            },
            bucket_capacity: reader.number(
                "LK_BUCKET_CAPACITY",
                defaults.bucket_capacity,
                100,
                10_000_000,
            )?,
            bucket_idle: defaults.bucket_idle,
        })
    }
}
