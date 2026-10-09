//! A plain timestamp, so the domain crate needs no date library and tests never read a clock.

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_unix_millis(ms: i64) -> Self {
        Self(ms)
    }

    pub const fn unix_millis(self) -> i64 {
        self.0
    }
}
