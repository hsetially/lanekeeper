//! `FakeLeases` that remember what was asked of them: the name and the time to live of each lease a request tried to take.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use ports::fakes::FakeLeases;
use ports::{LeaseError, LeaseGuard, Leases};

#[derive(Default)]
pub struct RecordingLeases {
    inner: FakeLeases,
    asked: Mutex<Vec<(String, Duration)>>,
}

impl std::fmt::Debug for RecordingLeases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingLeases").finish_non_exhaustive()
    }
}

impl RecordingLeases {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `try_acquire` so far, in order.
    pub fn asked(&self) -> Vec<(String, Duration)> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Leases for RecordingLeases {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name.to_owned(), ttl));
        self.inner.try_acquire(name, ttl).await
    }
}
