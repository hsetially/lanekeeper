//! A clock the test moves by hand, so a certificate's lifetime and a renewal schedule never depend on the wall clock.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use domain::Timestamp;
use hub_platform::clock::Clock;

/// 2027-01-15T08:00:00Z, in Unix milliseconds. Any fixed instant would do.
pub const T0_MS: i64 = 1_800_000_000_000;

#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicI64,
}

impl ManualClock {
    pub fn new(now_ms: i64) -> Arc<Self> {
        Arc::new(Self {
            now_ms: AtomicI64::new(now_ms),
        })
    }

    pub fn at_t0() -> Arc<Self> {
        Self::new(T0_MS)
    }

    pub fn advance_secs(&self, secs: i64) {
        self.now_ms.fetch_add(secs * 1000, Ordering::SeqCst);
    }

    pub fn set_ms(&self, ms: i64) {
        self.now_ms.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(self.now_ms.load(Ordering::SeqCst))
    }
}
