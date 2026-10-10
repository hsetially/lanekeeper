//! When to renew the certificate, and when to give up on renewing and join again (S5).
//!
//! A certificate is renewed over the stream at half its lifetime. If renewal keeps failing, the agent stops trying once
//! less than a tenth of the lifetime is left and joins again from scratch, which needs no working certificate. A
//! certificate that has already expired is in the same state: only a join can replace it.

use domain::Timestamp;

/// What to do with a certificate at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Too early to renew. Look again at `until`.
    Wait { until: Timestamp },
    /// Renew over the stream, and keep retrying until `rejoin_at`.
    Renew { rejoin_at: Timestamp },
    /// Less than a tenth of the lifetime is left, or none: join again.
    Rejoin,
}

/// The renewal and re-join times of one certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenewalSchedule {
    renew_at: Timestamp,
    rejoin_at: Timestamp,
}

impl RenewalSchedule {
    /// Renew when this share of the lifetime has passed.
    pub const RENEW_AT_PERCENT: i64 = 50;
    /// Join again when no more than this share of the lifetime is left.
    pub const REJOIN_BELOW_PERCENT: i64 = 10;

    /// The schedule of a certificate valid from `not_before` to `not_after`.
    pub fn new(not_before: Timestamp, not_after: Timestamp) -> Self {
        let start = not_before.unix_millis();
        let end = not_after.unix_millis().max(start);
        let lifetime = end - start;
        Self {
            renew_at: Timestamp::from_unix_millis(start + lifetime * Self::RENEW_AT_PERCENT / 100),
            rejoin_at: Timestamp::from_unix_millis(end - lifetime * Self::REJOIN_BELOW_PERCENT / 100),
        }
    }

    /// Half of the lifetime has passed.
    pub fn renew_at(&self) -> Timestamp {
        self.renew_at
    }

    /// Nine tenths of the lifetime have passed.
    pub fn rejoin_at(&self) -> Timestamp {
        self.rejoin_at
    }

    pub fn phase(&self, now: Timestamp) -> Phase {
        if now < self.renew_at {
            Phase::Wait { until: self.renew_at }
        } else if now < self.rejoin_at {
            Phase::Renew {
                rejoin_at: self.rejoin_at,
            }
        } else {
            Phase::Rejoin
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const HOUR_MS: i64 = 3_600_000;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms)
    }

    /// A 24-hour certificate starting at 0.
    fn day() -> RenewalSchedule {
        RenewalSchedule::new(ts(0), ts(24 * HOUR_MS))
    }

    #[test]
    fn renewal_is_due_at_half_the_lifetime() {
        let s = day();
        assert_eq!(s.renew_at(), ts(12 * HOUR_MS));
        assert_eq!(
            s.phase(ts(0)),
            Phase::Wait {
                until: ts(12 * HOUR_MS)
            }
        );
        assert_eq!(
            s.phase(ts(12 * HOUR_MS - 1)),
            Phase::Wait {
                until: ts(12 * HOUR_MS)
            }
        );
        assert!(matches!(s.phase(ts(12 * HOUR_MS)), Phase::Renew { .. }));
    }

    #[test]
    fn re_joining_starts_when_a_tenth_is_left() {
        let s = day();
        // 10% of 24 h is 2.4 h, so 21.6 h.
        assert_eq!(s.rejoin_at(), ts(21 * HOUR_MS + 36 * 60_000));
        assert_eq!(
            s.phase(ts(s.rejoin_at().unix_millis() - 1)),
            Phase::Renew {
                rejoin_at: s.rejoin_at()
            }
        );
        assert_eq!(s.phase(s.rejoin_at()), Phase::Rejoin);
        assert_eq!(s.phase(ts(23 * HOUR_MS)), Phase::Rejoin);
    }

    #[test]
    fn an_expired_certificate_can_only_be_replaced_by_a_join() {
        let s = day();
        assert_eq!(s.phase(ts(24 * HOUR_MS)), Phase::Rejoin);
        assert_eq!(s.phase(ts(1_000 * HOUR_MS)), Phase::Rejoin);
    }

    #[test]
    fn a_backdated_start_moves_both_points_with_the_whole_lifetime() {
        // Issued at 0 but valid from -5 minutes: the lifetime is 24 h 5 min.
        let s = RenewalSchedule::new(ts(-300_000), ts(24 * HOUR_MS));
        let lifetime = 24 * HOUR_MS + 300_000;
        assert_eq!(s.renew_at(), ts(-300_000 + lifetime / 2));
        assert_eq!(s.rejoin_at(), ts(24 * HOUR_MS - lifetime / 10));
    }

    #[test]
    fn a_degenerate_certificate_is_never_waited_for() {
        let s = RenewalSchedule::new(ts(100), ts(100));
        assert_eq!(s.phase(ts(100)), Phase::Rejoin);
        let inverted = RenewalSchedule::new(ts(200), ts(100));
        assert_eq!(inverted.phase(ts(200)), Phase::Rejoin);
    }

    proptest! {
        #[test]
        fn the_phases_are_ordered_in_time(
            start in -1_000_000_000_000_i64..1_000_000_000_000,
            lifetime in 1_i64..400_000_000,
            at in -1_000_000_i64..500_000_000,
        ) {
            let s = RenewalSchedule::new(ts(start), ts(start + lifetime));
            prop_assert!(s.renew_at() <= s.rejoin_at());
            prop_assert!(s.renew_at().unix_millis() >= start);
            prop_assert!(s.rejoin_at().unix_millis() <= start + lifetime);
            let now = ts(start + at);
            match s.phase(now) {
                Phase::Wait { until } => prop_assert!(now < until && until == s.renew_at()),
                Phase::Renew { rejoin_at } => {
                    prop_assert!(now >= s.renew_at() && now < rejoin_at && rejoin_at == s.rejoin_at());
                }
                Phase::Rejoin => prop_assert!(now >= s.rejoin_at()),
            }
        }
    }
}
