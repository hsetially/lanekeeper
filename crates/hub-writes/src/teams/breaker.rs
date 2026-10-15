//! The circuit breaker of a Teams destination (T7, plan A10): after repeated failures the hub stops calling a webhook that is not
//! answering, for a while, and then lets one call through to see whether it is back.
//!
//! This is a plain state machine over millisecond times that the caller passes in, so its tests move time by writing a number.
//!
//! - **Closed:** every call goes through. A failure is remembered for the window; the failure that makes the threshold within
//!   the window opens the breaker. A success forgets the failures.
//! - **Open:** no call goes through until `open_for` has passed.
//! - **Half-open:** after that, exactly one call (the probe) goes through. Its success closes the breaker, its failure opens it
//!   again for another `open_for`. While the probe is out, other calls are refused. A probe that never reports (its task was
//!   dropped) is given up on after one more `open_for`, so the breaker cannot stay shut for good.

use std::collections::VecDeque;
use std::time::Duration;

/// When the breaker opens and for how long. The defaults are the plan's: 5 failures in 60 seconds open it for 30 seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerConfig {
    /// The failures within `window` that open the breaker.
    pub failures: usize,
    pub window: Duration,
    pub open_for: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failures: 5,
            window: Duration::from_secs(60),
            open_for: Duration::from_secs(30),
        }
    }
}

/// Where a breaker is, for the monitor and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
enum Phase {
    Closed { failures: VecDeque<i64> },
    Open { until: i64 },
    HalfOpen { probe_since: Option<i64> },
}

#[derive(Debug)]
pub(super) struct Breaker {
    cfg: BreakerConfig,
    phase: Phase,
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

impl Breaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self {
            cfg,
            phase: Phase::Closed {
                failures: VecDeque::new(),
            },
        }
    }

    /// May a call go through now?
    pub fn allow(&mut self, now: i64) -> bool {
        match &mut self.phase {
            Phase::Closed { .. } => true,
            Phase::Open { until } => {
                if now >= *until {
                    // The pause is over: this call is the probe.
                    self.phase = Phase::HalfOpen {
                        probe_since: Some(now),
                    };
                    true
                } else {
                    false
                }
            }
            Phase::HalfOpen { probe_since } => {
                let lost =
                    probe_since.is_none_or(|since| now.saturating_sub(since) >= millis(self.cfg.open_for));
                if lost {
                    *probe_since = Some(now);
                }
                lost
            }
        }
    }

    /// The call that was allowed succeeded.
    pub fn success(&mut self) {
        match &mut self.phase {
            Phase::Closed { failures } => failures.clear(),
            // The probe came back: the webhook is up again.
            Phase::HalfOpen { .. } => {
                self.phase = Phase::Closed {
                    failures: VecDeque::new(),
                };
            }
            // A call made before the breaker opened, finishing late. Only the probe closes an open breaker.
            Phase::Open { .. } => {}
        }
    }

    /// The call that was allowed failed. True when this failure opened the breaker.
    pub fn failure(&mut self, now: i64) -> bool {
        let open_until = now.saturating_add(millis(self.cfg.open_for));
        match &mut self.phase {
            Phase::Closed { failures } => {
                let oldest_kept = now.saturating_sub(millis(self.cfg.window));
                while failures.front().is_some_and(|&t| t <= oldest_kept) {
                    failures.pop_front();
                }
                failures.push_back(now);
                if failures.len() >= self.cfg.failures {
                    self.phase = Phase::Open { until: open_until };
                    true
                } else {
                    false
                }
            }
            Phase::HalfOpen { .. } => {
                self.phase = Phase::Open { until: open_until };
                true
            }
            // A late failure of a call made before it opened does not push the reopening back.
            Phase::Open { .. } => false,
        }
    }

    pub fn state(&self) -> BreakerState {
        match self.phase {
            Phase::Closed { .. } => BreakerState::Closed,
            Phase::Open { .. } => BreakerState::Open,
            Phase::HalfOpen { .. } => BreakerState::HalfOpen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000;

    fn breaker() -> Breaker {
        Breaker::new(BreakerConfig::default())
    }

    /// Fail `n` calls, one second apart, from `start`. Returns the time of the last.
    fn fail(b: &mut Breaker, n: usize, start: i64) -> i64 {
        let mut at = start;
        for i in 0..n {
            at = start + i64::try_from(i).unwrap() * S;
            assert!(b.allow(at), "failure {i} should have been allowed through");
            b.failure(at);
        }
        at
    }

    #[test]
    fn a_closed_breaker_lets_everything_through() {
        let mut b = breaker();
        for t in 0..100 {
            assert!(b.allow(t));
        }
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn five_failures_within_the_window_open_it() {
        let mut b = breaker();
        let last = fail(&mut b, 4, 0);
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.allow(last + S));
        assert!(b.failure(last + S), "the fifth failure opens it");
        assert_eq!(b.state(), BreakerState::Open);
        assert!(!b.allow(last + 2 * S));
        assert!(!b.allow(last + 29 * S));
    }

    #[test]
    fn failures_older_than_the_window_do_not_count() {
        let mut b = breaker();
        fail(&mut b, 4, 0);
        // The oldest four are 61 s or more old by now.
        let t = 70 * S;
        assert!(b.allow(t));
        assert!(
            !b.failure(t),
            "one failure after four expired ones is the first of a new run"
        );
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn a_success_forgets_the_failures() {
        let mut b = breaker();
        let last = fail(&mut b, 4, 0);
        b.success();
        let t = last + S;
        assert!(b.allow(t));
        assert!(!b.failure(t));
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn after_the_open_time_exactly_one_probe_goes_through() {
        let mut b = breaker();
        let opened = fail(&mut b, 5, 0);
        assert_eq!(b.state(), BreakerState::Open);
        let later = opened + 30 * S;
        assert!(b.allow(later), "the first call after the pause is the probe");
        assert_eq!(b.state(), BreakerState::HalfOpen);
        assert!(!b.allow(later + 1), "no second call while the probe is out");
        assert!(!b.allow(later + 10 * S));
    }

    #[test]
    fn a_probe_that_succeeds_closes_it() {
        let mut b = breaker();
        let opened = fail(&mut b, 5, 0);
        let later = opened + 30 * S;
        assert!(b.allow(later));
        b.success();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.allow(later + 1));
        assert!(b.allow(later + 2));
    }

    #[test]
    fn a_probe_that_fails_opens_it_again_for_the_full_time() {
        let mut b = breaker();
        let opened = fail(&mut b, 5, 0);
        let later = opened + 30 * S;
        assert!(b.allow(later));
        assert!(b.failure(later + S), "a failed probe reopens it");
        assert_eq!(b.state(), BreakerState::Open);
        assert!(
            !b.allow(later + 30 * S),
            "30 s from the failed probe, not from the first opening"
        );
        assert!(b.allow(later + S + 30 * S));
    }

    #[test]
    fn a_probe_that_never_reports_is_given_up_on() {
        let mut b = breaker();
        let opened = fail(&mut b, 5, 0);
        let later = opened + 30 * S;
        assert!(b.allow(later));
        assert!(!b.allow(later + 29 * S));
        assert!(
            b.allow(later + 30 * S),
            "the lost probe is replaced after another open time"
        );
    }

    #[test]
    fn a_late_result_of_a_call_made_before_it_opened_changes_nothing() {
        let mut b = breaker();
        let opened = fail(&mut b, 5, 0);
        b.success();
        assert_eq!(
            b.state(),
            BreakerState::Open,
            "only a probe closes an open breaker"
        );
        assert!(
            !b.failure(opened + S),
            "a late failure does not push the reopening back"
        );
        assert!(b.allow(opened + 30 * S));
    }

    #[test]
    fn the_thresholds_are_the_configured_ones() {
        let mut b = Breaker::new(BreakerConfig {
            failures: 2,
            window: Duration::from_secs(10),
            open_for: Duration::from_secs(5),
        });
        assert!(b.allow(0));
        assert!(!b.failure(0));
        assert!(b.allow(S));
        assert!(b.failure(S));
        assert!(!b.allow(5 * S));
        assert!(b.allow(6 * S));
    }

    #[test]
    fn what_is_remembered_never_reaches_the_threshold() {
        // The failure that makes the threshold opens the breaker, so a closed one holds fewer than that.
        let mut b = Breaker::new(BreakerConfig {
            failures: 3,
            window: Duration::from_secs(3_600),
            open_for: Duration::from_secs(1),
        });
        for i in 0..10_000_i64 {
            let now = i * 10_000;
            if b.allow(now) {
                b.failure(now);
            }
            if let Phase::Closed { failures } = &b.phase {
                assert!(failures.len() < 3);
            }
        }
    }

    #[test]
    fn a_threshold_of_one_opens_at_the_first_failure() {
        let mut b = Breaker::new(BreakerConfig {
            failures: 1,
            ..BreakerConfig::default()
        });
        assert!(b.allow(0));
        assert!(b.failure(0));
        assert_eq!(b.state(), BreakerState::Open);
    }
}
