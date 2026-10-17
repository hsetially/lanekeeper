//! The registry's notion of "now", behind a trait.
//!
//! Nothing in this crate reads the wall clock directly: every instant that goes into data (an audit event's time, a window's
//! open time, the clamp on a future `observed_at`) comes from a [`Clock`], so a test moves time by hand. Waiting (timeouts,
//! intervals) uses tokio's clock, which tests pause.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use domain::Timestamp;

pub trait Clock: Send + Sync + fmt::Debug + 'static {
    fn now(&self) -> Timestamp;
}

/// The wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        // A clock before 1970 reads as the epoch: nothing here can use it for a decision that matters more than being early.
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Timestamp::from_unix_millis(ms)
    }
}

/// A clock that only moves when told to. For tests and benchmarks.
#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicI64,
}

impl ManualClock {
    /// 2027-01-15T08:00:00Z in Unix milliseconds: any fixed instant would do.
    pub const T0_MS: i64 = 1_800_000_000_000;

    pub fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    pub fn at_t0() -> Self {
        Self::new(Self::T0_MS)
    }

    pub fn set_ms(&self, ms: i64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }

    pub fn advance_secs(&self, secs: i64) {
        self.now_ms.fetch_add(secs.saturating_mul(1000), Ordering::SeqCst);
    }

    pub fn advance_ms(&self, ms: i64) {
        self.now_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.now_ms.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_when_told() {
        let clock = ManualClock::at_t0();
        assert_eq!(clock.now().unix_millis(), ManualClock::T0_MS);
        assert_eq!(clock.now(), clock.now());
        clock.advance_secs(2);
        clock.advance_ms(5);
        assert_eq!(clock.now().unix_millis(), ManualClock::T0_MS + 2_005);
        clock.set_ms(7);
        assert_eq!(clock.now().unix_millis(), 7);
    }

    #[test]
    fn the_system_clock_is_after_2026() {
        assert!(SystemClock.now().unix_millis() > 1_767_225_600_000);
    }
}
