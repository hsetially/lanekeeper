//! Time source (T1). Loops take a [`Clock`] so tests control time with `tokio::time::pause` instead of sleeping.

use std::time::{SystemTime, UNIX_EPOCH};

use domain::Timestamp;

/// Wall-clock and monotonic time.
pub trait Clock: Send + Sync + std::fmt::Debug + 'static {
    /// Wall-clock time, for stamps on messages and for comparing with file times.
    fn now(&self) -> Timestamp;

    /// Monotonic time, for intervals and deadlines. Follows `tokio::time::pause`.
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// 2025-01-01T00:00:00Z. Any real clock is later than this.
    const JAN_2025_MS: i64 = 1_735_689_600_000;

    #[test]
    fn system_clock_reports_a_plausible_wall_time() {
        let now = SystemClock.now().unix_millis();
        assert!(now > JAN_2025_MS, "wall clock reads {now}");
        assert!(
            SystemClock.now().unix_millis() >= now,
            "wall clock went backwards"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn system_clock_instant_follows_tokio_time() {
        let clock = SystemClock;
        let before = clock.instant();
        tokio::time::advance(Duration::from_secs(90)).await;
        assert_eq!(clock.instant() - before, Duration::from_secs(90));
    }
}
