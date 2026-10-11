//! The hub's notion of "now", behind a trait so that certificate lifetimes, renewal schedules and expiry checks can be tested
//! with a clock the test moves by hand.
//!
//! Waiting (timeouts, intervals) uses tokio's clock, which tests pause. This clock is for *instants that go into data*: a
//! certificate's `notBefore` and `notAfter`, an audit event's time.

use std::fmt;
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
