//! Retry delays: exponential growth, a cap, and full jitter (S6, rule 5 of the code rules).
//!
//! Every retry loop in the agent uses [`Backoff`], so a hub that is down is never hammered and a thousand agents that
//! lost the hub together do not come back together. The delay before attempt `n` is drawn uniformly from
//! `0..=min(cap, base * 2^n)`.

use std::time::Duration;

/// The exponent stops growing here: `base * 2^40` is far beyond any sensible cap, and the shift cannot overflow.
const MAX_EXPONENT: u32 = 40;

/// The most a delay can be before attempt number `attempt` (counting from 0): `min(cap, base * 2^attempt)`.
pub fn ceiling(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let grown = base
        .as_nanos()
        .saturating_mul(1_u128 << attempt.min(MAX_EXPONENT));
    let capped = grown.min(cap.as_nanos());
    // `cap` is a `Duration`, so `capped <= cap` fits in a `Duration` again.
    Duration::new(
        u64::try_from(capped / 1_000_000_000).unwrap_or(u64::MAX),
        u32::try_from(capped % 1_000_000_000).unwrap_or(0),
    )
}

/// A retry schedule with full jitter. Call [`Backoff::next_delay`] after each failure and [`Backoff::reset`] after a
/// success.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
    rng: fastrand::Rng,
}

impl Backoff {
    /// A schedule that starts at `base` and never waits longer than `cap`.
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self::with_rng(base, cap, fastrand::Rng::new())
    }

    /// Like [`Backoff::new`], with a fixed seed, so a test sees the same delays every time.
    pub fn with_seed(base: Duration, cap: Duration, seed: u64) -> Self {
        Self::with_rng(base, cap, fastrand::Rng::with_seed(seed))
    }

    fn with_rng(base: Duration, cap: Duration, rng: fastrand::Rng) -> Self {
        Self {
            base,
            cap: cap.max(base),
            attempt: 0,
            rng,
        }
    }

    /// The most the next delay can be.
    pub fn next_ceiling(&self) -> Duration {
        ceiling(self.base, self.cap, self.attempt)
    }

    /// How long to wait before the next attempt. Each call moves the schedule one step on.
    pub fn next_delay(&mut self) -> Duration {
        let most = self.next_ceiling();
        self.attempt = self.attempt.saturating_add(1);
        let nanos = u64::try_from(most.as_nanos()).unwrap_or(u64::MAX);
        Duration::from_nanos(self.rng.u64(0..=nanos))
    }

    /// Start again from `base`, after a success.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// How many delays have been handed out since the last reset.
    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const BASE: Duration = Duration::from_secs(1);
    const CAP: Duration = Duration::from_secs(60);

    #[test]
    fn ceiling_doubles_then_stops_at_the_cap() {
        let at = |n| ceiling(BASE, CAP, n);
        assert_eq!(at(0), Duration::from_secs(1));
        assert_eq!(at(1), Duration::from_secs(2));
        assert_eq!(at(5), Duration::from_secs(32));
        assert_eq!(at(6), CAP, "64 s is capped at 60 s");
        assert_eq!(at(1_000), CAP);
        assert_eq!(at(u32::MAX), CAP);
    }

    #[test]
    fn a_cap_below_the_base_is_raised_to_the_base() {
        let mut b = Backoff::with_seed(Duration::from_secs(10), Duration::from_secs(1), 7);
        assert_eq!(b.next_ceiling(), Duration::from_secs(10));
        assert!(b.next_delay() <= Duration::from_secs(10));
    }

    #[test]
    fn reset_starts_again_from_the_base() {
        let mut b = Backoff::with_seed(BASE, CAP, 1);
        for _ in 0..4 {
            b.next_delay();
        }
        assert_eq!(b.attempts(), 4);
        assert_eq!(b.next_ceiling(), Duration::from_secs(16));
        b.reset();
        assert_eq!(b.attempts(), 0);
        assert_eq!(b.next_ceiling(), BASE);
    }

    #[test]
    fn the_same_seed_gives_the_same_delays() {
        let run = |seed| {
            let mut b = Backoff::with_seed(BASE, CAP, seed);
            (0..8).map(|_| b.next_delay()).collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43), "different seeds should differ");
    }

    proptest! {
        /// `backoff_full_jitter_capped_at_60s` (S6): every delay is between zero and the schedule's ceiling, and no
        /// delay is ever longer than the cap.
        #[test]
        fn backoff_full_jitter_capped_at_60s(seed in any::<u64>(), steps in 1_usize..200) {
            let mut b = Backoff::with_seed(BASE, CAP, seed);
            for n in 0..steps {
                let n = u32::try_from(n).unwrap_or(u32::MAX);
                let most = ceiling(BASE, CAP, n);
                prop_assert_eq!(b.next_ceiling(), most);
                let delay = b.next_delay();
                prop_assert!(delay <= most);
                prop_assert!(delay <= CAP);
            }
        }

        #[test]
        fn the_ceiling_never_shrinks_and_never_passes_the_cap(
            base_ms in 1_u64..10_000,
            cap_ms in 1_u64..1_000_000,
            n in 0_u32..100,
        ) {
            let base = Duration::from_millis(base_ms);
            let cap = Duration::from_millis(cap_ms).max(base);
            prop_assert!(ceiling(base, cap, n) <= ceiling(base, cap, n + 1));
            prop_assert!(ceiling(base, cap, n) <= cap);
            prop_assert!(ceiling(base, cap, n) >= base);
        }
    }
}
