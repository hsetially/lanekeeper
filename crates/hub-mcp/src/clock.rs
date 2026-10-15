//! Time source. Code that checks expiry or ages a cache entry takes a [`Clock`], so tests move time instead of sleeping.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use domain::Timestamp;

/// Wall-clock and monotonic time.
pub trait Clock: Send + Sync + std::fmt::Debug + 'static {
    /// Wall-clock time, for comparing with stored expiry times.
    fn now(&self) -> Timestamp;

    /// Monotonic time, for intervals and rate limits. Follows `tokio::time::pause`.
    fn instant(&self) -> tokio::time::Instant;
}

/// The real clocks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            // A clock set before 1970 is a broken node, not a reason to stop: report it as negative time.
            Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
        };
        Timestamp::from_unix_millis(millis)
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

/// A wall clock that only moves when told to. The monotonic side follows tokio's (pausable) clock. For tests.
#[derive(Debug)]
pub struct ManualClock {
    millis: AtomicI64,
}

impl ManualClock {
    #[must_use]
    pub fn at(start: Timestamp) -> Self {
        Self {
            millis: AtomicI64::new(start.unix_millis()),
        }
    }

    pub fn set(&self, to: Timestamp) {
        self.millis.store(to.unix_millis(), Ordering::SeqCst);
    }

    pub fn advance_secs(&self, secs: i64) {
        self.millis.fetch_add(secs.saturating_mul(1000), Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.millis.load(Ordering::SeqCst))
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}
