//! Limits (rule 5, S14): what one caller, and the whole replica, may ask of the server.
//!
//! - **Rate**, per user and per class (reads and writes are separate budgets): a token bucket. Reads 60 a minute with a burst of
//!   20, writes 12 a minute with a burst of 4. A call over the limit fails with `rate_limited` and `retry_after_secs`.
//! - **Concurrency**: 8 calls at once per user, 256 per replica. Over the per-user limit is `rate_limited` (wait a second), over the
//!   replica's is `unavailable`.
//! - **In-flight file bytes**: 64 MiB per replica, a weighted semaphore taken before a file is decoded and scanned. A file costs
//!   three times its size while it is decoded (the bytes, the text and the scan's working set), so a 2 MiB file costs 6 MiB.
//! - **Timeouts**: 5 s for a read, 30 s for a write, 120 s for a confirmation (T3).
//! - **Sessions**: 5 per user, 200 per replica, 30 minutes idle (`server::session`).
//!
//! Every table is a bounded moka cache with idle expiry (10,000 buckets), and every number is configuration. No queue is
//! unbounded: a call that cannot be admitted is refused at once and never waits in line.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::UserId;
use moka::sync::Cache;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::clock::Clock;
use crate::config::ConfigError;

/// Which budget a tool call draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Read,
    Write,
}

/// The numbers. Defaults are the plan's (A15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitSettings {
    pub read_per_minute: u32,
    pub read_burst: u32,
    pub write_per_minute: u32,
    pub write_burst: u32,
    pub calls_per_user: u32,
    pub calls_per_replica: u32,
    /// Bytes of file content being decoded or scanned at once, across the replica.
    pub inflight_bytes: u64,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub confirm_timeout: Duration,
    pub sessions_per_user: u32,
    pub sessions_per_replica: u32,
    pub session_idle: Duration,
    /// Memory for the scan and settings-index caches together (P11). Zero turns them off.
    pub cache_bytes: u64,
    /// Rate buckets kept at once.
    pub bucket_capacity: u64,
}

impl Default for LimitSettings {
    fn default() -> Self {
        Self {
            read_per_minute: 60,
            read_burst: 20,
            write_per_minute: 12,
            write_burst: 4,
            calls_per_user: 8,
            calls_per_replica: 256,
            inflight_bytes: 64 * 1024 * 1024,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(30),
            confirm_timeout: Duration::from_secs(120),
            sessions_per_user: 5,
            sessions_per_replica: 200,
            session_idle: Duration::from_secs(30 * 60),
            cache_bytes: 96 * 1024 * 1024,
            bucket_capacity: 10_000,
        }
    }
}

impl LimitSettings {
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `limits` when a number is zero where it must not be, or beyond what a replica can hold.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = ConfigError::Invalid("limits");
        let in_range = |n: u32, max: u32| (1..=max).contains(&n);
        // The ceilings are far above any real deployment (the defaults are 60 and 12 a minute); they exist so that a mistyped number
        // is an error, and so that a benchmark can take the rate limit out of the way.
        if !(in_range(self.read_per_minute, 60_000_000)
            && in_range(self.read_burst, 1_000_000)
            && in_range(self.write_per_minute, 6_000_000)
            && in_range(self.write_burst, 1_000_000)
            && in_range(self.calls_per_user, 1_000)
            && in_range(self.calls_per_replica, 100_000)
            && in_range(self.sessions_per_user, 1_000)
            && in_range(self.sessions_per_replica, 100_000))
        {
            return Err(bad);
        }
        let secs = |d: Duration, max: u64| (1..=max).contains(&d.as_secs());
        if !(secs(self.read_timeout, 120)
            && secs(self.write_timeout, 600)
            && secs(self.confirm_timeout, 900)
            && secs(self.session_idle, 24 * 3600))
        {
            return Err(bad);
        }
        // The byte budget must hold at least one largest file at its decoded cost, or no such file could ever be read.
        if self.inflight_bytes < 6 * 1024 * 1024 || self.inflight_bytes > 4 * 1024 * 1024 * 1024 {
            return Err(bad);
        }
        if self.cache_bytes > 4 * 1024 * 1024 * 1024 || !(1..=1_000_000).contains(&self.bucket_capacity) {
            return Err(bad);
        }
        Ok(())
    }
}

/// Why a call was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The caller is over its rate or concurrency; try again after this many seconds.
    RateLimited { retry_after_secs: u32 },
    /// The replica is full. Retryable.
    Overloaded,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: tokio::time::Instant,
}

/// What a running call holds. Dropping it frees the slots.
#[derive(Debug)]
pub struct CallPermit {
    _user: OwnedSemaphorePermit,
    _replica: OwnedSemaphorePermit,
}

/// The limits of one replica.
pub struct Limits {
    settings: LimitSettings,
    clock: Arc<dyn Clock>,
    buckets: Cache<(UserId, Class), Arc<Mutex<Bucket>>>,
    per_user: Cache<UserId, Arc<Semaphore>>,
    replica: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}

impl std::fmt::Debug for Limits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Limits").finish_non_exhaustive()
    }
}

impl Limits {
    #[must_use]
    pub fn new(settings: LimitSettings, clock: Arc<dyn Clock>) -> Self {
        let idle = Duration::from_secs(10 * 60);
        Self {
            buckets: Cache::builder()
                .max_capacity(settings.bucket_capacity)
                .time_to_idle(idle)
                .build(),
            per_user: Cache::builder()
                .max_capacity(settings.bucket_capacity)
                .time_to_idle(idle)
                .build(),
            replica: Arc::new(Semaphore::new(settings.calls_per_replica as usize)),
            bytes: Arc::new(Semaphore::new(
                usize::try_from(settings.inflight_bytes / BYTE_UNIT).unwrap_or(usize::MAX >> 4),
            )),
            settings,
            clock,
        }
    }

    #[must_use]
    pub fn settings(&self) -> &LimitSettings {
        &self.settings
    }

    /// How long a call of this class may run.
    #[must_use]
    pub fn timeout(&self, class: Class) -> Duration {
        match class {
            Class::Read => self.settings.read_timeout,
            Class::Write => self.settings.write_timeout,
        }
    }

    fn bucket_for(&self, user: &UserId, class: Class) -> Arc<Mutex<Bucket>> {
        let burst = match class {
            Class::Read => self.settings.read_burst,
            Class::Write => self.settings.write_burst,
        };
        let now = self.clock.instant();
        self.buckets.get_with((user.clone(), class), || {
            Arc::new(Mutex::new(Bucket {
                tokens: f64::from(burst),
                last: now,
            }))
        })
    }

    /// Take one token of the user's rate budget.
    fn take_token(&self, user: &UserId, class: Class) -> Result<(), Refusal> {
        let (per_minute, burst) = match class {
            Class::Read => (self.settings.read_per_minute, self.settings.read_burst),
            Class::Write => (self.settings.write_per_minute, self.settings.write_burst),
        };
        let rate = f64::from(per_minute) / 60.0;
        let capacity = f64::from(burst);
        let bucket = self.bucket_for(user, class);
        let mut bucket = bucket.lock().unwrap_or_else(PoisonError::into_inner);
        let now = self.clock.instant();
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / rate;
            // At least one second, and never more than a day: the contract's `retry_after_secs`.
            let secs = wait.ceil().clamp(1.0, 86_400.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Err(Refusal::RateLimited {
                retry_after_secs: secs as u32,
            })
        }
    }

    /// Admit a call: the user's rate, then their concurrency, then the replica's.
    ///
    /// # Errors
    ///
    /// [`Refusal`] when the call is over a limit. A refused call has used nothing.
    pub fn begin(&self, user: &UserId, class: Class) -> Result<CallPermit, Refusal> {
        let user_slots = self.per_user.get_with(user.clone(), || {
            Arc::new(Semaphore::new(self.settings.calls_per_user as usize))
        });
        // Concurrency first: a call that is turned away for being too many at once must not spend a rate token.
        let user_permit = Arc::clone(&user_slots)
            .try_acquire_owned()
            .map_err(|_| Refusal::RateLimited { retry_after_secs: 1 })?;
        let replica_permit = Arc::clone(&self.replica)
            .try_acquire_owned()
            .map_err(|_| Refusal::Overloaded)?;
        self.take_token(user, class)?;
        Ok(CallPermit {
            _user: user_permit,
            _replica: replica_permit,
        })
    }

    /// Reserve `bytes` of the in-flight budget for as long as the permit lives, waiting at most `within`.
    ///
    /// # Errors
    ///
    /// [`Refusal::Overloaded`] when the budget does not free up in time, or `bytes` is more than the budget.
    pub async fn reserve_bytes(&self, bytes: u64, within: Duration) -> Result<OwnedSemaphorePermit, Refusal> {
        let units = u32::try_from(bytes.div_ceil(BYTE_UNIT).max(1)).map_err(|_| Refusal::Overloaded)?;
        if u64::from(units) * BYTE_UNIT > self.settings.inflight_bytes {
            return Err(Refusal::Overloaded);
        }
        match tokio::time::timeout(within, Arc::clone(&self.bytes).acquire_many_owned(units)).await {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(Refusal::Overloaded),
        }
    }

    /// Bytes of the in-flight budget that are free right now (for tests and the metrics).
    #[must_use]
    pub fn free_bytes(&self) -> u64 {
        self.bytes.available_permits() as u64 * BYTE_UNIT
    }
}

/// The in-flight budget is counted in units of this many bytes, so that it fits a semaphore's permits whatever its size.
const BYTE_UNIT: u64 = 1024;
