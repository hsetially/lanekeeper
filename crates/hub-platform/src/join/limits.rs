//! Bounded join attempts (S5, rule 5): per swimlane and per peer address, in memory that cannot grow without bound.
//!
//! A counter lives for one window from its first attempt. The tables are caches with a hard maximum, so a flood of made-up
//! swimlane ids or addresses evicts old counters instead of growing memory. The tokens guarded are 128-bit random values, so
//! the limits are there to bound work (a KMS signature, a database round trip) and noise, not to make a guess infeasible.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use moka::sync::Cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinLimits {
    /// Attempts per swimlane per window, successful or not.
    pub per_swimlane: u32,
    /// Attempts per peer address per window.
    pub per_address: u32,
    pub window: Duration,
    /// Counters kept per table.
    pub max_tracked: u64,
}

impl Default for JoinLimits {
    fn default() -> Self {
        Self {
            per_swimlane: 10,
            per_address: 30,
            window: Duration::from_secs(60),
            max_tracked: 10_000,
        }
    }
}

#[derive(Debug)]
pub struct JoinLimiter {
    limits: JoinLimits,
    swimlanes: Cache<String, Arc<AtomicU32>>,
    addresses: Cache<IpAddr, Arc<AtomicU32>>,
}

impl JoinLimiter {
    #[must_use]
    pub fn new(limits: JoinLimits) -> Self {
        Self {
            limits,
            swimlanes: Cache::builder()
                .max_capacity(limits.max_tracked)
                .time_to_live(limits.window)
                .build(),
            addresses: Cache::builder()
                .max_capacity(limits.max_tracked)
                .time_to_live(limits.window)
                .build(),
        }
    }

    /// Count one attempt for `swimlane` from `address`. False when either limit is already used up: the attempt is not made.
    /// The address is counted first because it is the cheaper thing to flood.
    #[must_use]
    pub fn allow(&self, swimlane: &str, address: Option<IpAddr>) -> bool {
        let address_ok = address.is_none_or(|ip| {
            let counter = self.addresses.get_with(ip, || Arc::new(AtomicU32::new(0)));
            counter.fetch_add(1, Ordering::Relaxed) < self.limits.per_address
        });
        let swimlane_ok = {
            let counter = self
                .swimlanes
                .get_with(swimlane.to_owned(), || Arc::new(AtomicU32::new(0)));
            counter.fetch_add(1, Ordering::Relaxed) < self.limits.per_swimlane
        };
        address_ok && swimlane_ok
    }

    /// How many counters are held, for the test that shows the tables are bounded.
    #[must_use]
    pub fn tracked(&self) -> (u64, u64) {
        self.swimlanes.run_pending_tasks();
        self.addresses.run_pending_tasks();
        (self.swimlanes.entry_count(), self.addresses.entry_count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(per_swimlane: u32, per_address: u32) -> JoinLimits {
        JoinLimits {
            per_swimlane,
            per_address,
            window: Duration::from_secs(3600),
            max_tracked: 100,
        }
    }

    #[test]
    fn a_swimlane_gets_its_attempts_and_no_more() {
        let limiter = JoinLimiter::new(limits(3, 100));
        let allowed = (0..5).filter(|_| limiter.allow("sit1", None)).count();
        assert_eq!(allowed, 3);
        assert!(limiter.allow("sit2", None), "another swimlane has its own budget");
    }

    #[test]
    fn an_address_gets_its_attempts_across_swimlanes() {
        let limiter = JoinLimiter::new(limits(100, 2));
        let ip: IpAddr = "10.0.0.7".parse().unwrap_or(IpAddr::from([0, 0, 0, 0]));
        assert!(limiter.allow("a", Some(ip)));
        assert!(limiter.allow("b", Some(ip)));
        assert!(
            !limiter.allow("c", Some(ip)),
            "the third swimlane from one address is refused"
        );
        assert!(limiter.allow("c", None), "no address, no address limit");
    }

    #[test]
    fn the_tables_never_hold_more_than_their_bound() {
        let limiter = JoinLimiter::new(limits(5, 5));
        for i in 0..1_000u32 {
            let _ = limiter.allow(&format!("sl-{i}"), Some(IpAddr::from(i.to_be_bytes())));
        }
        let (swimlanes, addresses) = limiter.tracked();
        assert!(swimlanes <= 100 && addresses <= 100, "{swimlanes} {addresses}");
    }
}
