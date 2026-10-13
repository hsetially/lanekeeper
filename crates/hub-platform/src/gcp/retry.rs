//! Bounded retries with full jitter (rule 5).
//!
//! Every Google call is retried a bounded number of times, and the wait before a retry is drawn uniformly from
//! `[0, min(cap, base * 2^(attempt - 1))]` ("full jitter"), so that three replicas that fail together do not retry together.

use std::time::Duration;

/// How often to try and how long to wait in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// The first try counts, so `3` means one try and two retries. At least 1.
    pub max_attempts: u32,
    /// The ceiling of the wait before the first retry.
    pub base: Duration,
    /// The ceiling of any wait.
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base: Duration::from_millis(100),
            cap: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    /// No retries at all.
    #[must_use]
    pub fn once() -> Self {
        Self {
            max_attempts: 1,
            base: Duration::ZERO,
            cap: Duration::ZERO,
        }
    }

    /// The largest wait before retry number `retry` (1 is the first retry).
    #[must_use]
    pub fn ceiling(&self, retry: u32) -> Duration {
        let doublings = retry.saturating_sub(1).min(32);
        let scaled = self
            .base
            .saturating_mul(1u32.checked_shl(doublings).unwrap_or(u32::MAX));
        scaled.min(self.cap)
    }

    /// The wait before retry number `retry`, for a random `unit` in `[0, 1]`. Separate from [`Self::jittered`] so that a
    /// test can pin the draw.
    #[must_use]
    pub fn delay(&self, retry: u32, unit: f64) -> Duration {
        let unit = if unit.is_finite() {
            unit.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.ceiling(retry).mul_f64(unit)
    }

    /// The wait before retry number `retry`, with a random draw.
    #[must_use]
    pub fn jittered(&self, retry: u32) -> Duration {
        self.delay(retry, fastrand::f64())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn the_ceiling_doubles_up_to_the_cap() {
        let policy = RetryPolicy {
            max_attempts: 6,
            base: Duration::from_millis(100),
            cap: Duration::from_millis(700),
        };
        let ceilings: Vec<u128> = (1..=5).map(|r| policy.ceiling(r).as_millis()).collect();
        assert_eq!(ceilings, [100, 200, 400, 700, 700]);
    }

    #[test]
    fn a_draw_of_zero_waits_nothing_and_a_draw_of_one_waits_the_ceiling() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay(2, 0.0), Duration::ZERO);
        assert_eq!(policy.delay(2, 1.0), policy.ceiling(2));
        assert_eq!(policy.delay(2, f64::NAN), policy.ceiling(2));
    }

    proptest! {
        #[test]
        fn the_wait_never_exceeds_the_cap_or_overflows(retry in 0u32..=u32::MAX, unit in any::<f64>()) {
            let policy = RetryPolicy::default();
            prop_assert!(policy.delay(retry, unit) <= policy.cap);
        }

        #[test]
        fn the_ceiling_never_shrinks(retry in 1u32..64) {
            let policy = RetryPolicy::default();
            prop_assert!(policy.ceiling(retry + 1) >= policy.ceiling(retry));
        }
    }
}
