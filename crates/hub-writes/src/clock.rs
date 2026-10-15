//! Time source. Code that stamps or expires a row takes a [`Clock`], so tests move time instead of sleeping.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use domain::Timestamp;

/// Wall-clock time, for the times that are stored (claims, proposals) and compared with stored ones.
pub trait Clock: Send + Sync + std::fmt::Debug + 'static {
    fn now(&self) -> Timestamp;
}

/// The real clock.
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
}

/// A wall clock that only moves when told to. For tests and benchmarks.
#[derive(Debug)]
pub struct ManualClock {
    millis: AtomicI64,
}

impl ManualClock {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2025-01-01T00:00:00Z. Any real clock is later than this.
    const JAN_2025_MS: i64 = 1_735_689_600_000;

    #[test]
    fn system_clock_reports_a_plausible_wall_time() {
        assert!(SystemClock.now().unix_millis() > JAN_2025_MS);
    }

    #[test]
    fn manual_clock_moves_only_when_told() {
        let clock = ManualClock::at(Timestamp::from_unix_millis(5_000));
        assert_eq!(clock.now().unix_millis(), 5_000);
        clock.advance_secs(3);
        assert_eq!(clock.now().unix_millis(), 8_000);
        clock.set(Timestamp::from_unix_millis(1));
        assert_eq!(clock.now().unix_millis(), 1);
    }
}
