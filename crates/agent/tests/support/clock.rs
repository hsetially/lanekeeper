//! A clock for tests that run under `tokio::time::pause`.
//!
//! Wall time is a fixed starting instant plus the virtual time that has passed, so sleeping a virtual hour moves the
//! wall clock by an hour too, and a certificate's validity window moves with the loops that watch it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use agent::clock::Clock;
use domain::Timestamp;
use tokio::time::Instant;

#[derive(Debug)]
pub struct TestClock {
    start_ms: i64,
    started: Instant,
}

impl TestClock {
    /// Wall time starts at `start_ms` (Unix milliseconds) now.
    pub fn starting_at(start_ms: i64) -> Self {
        Self {
            start_ms,
            started: Instant::now(),
        }
    }

    /// How much virtual time has passed since the clock started.
    pub fn elapsed(&self) -> std::time::Duration {
        Instant::now() - self.started
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        let elapsed = i64::try_from(self.elapsed().as_millis()).unwrap();
        Timestamp::from_unix_millis(self.start_ms + elapsed)
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }
}
