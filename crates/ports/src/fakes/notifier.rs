use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::util::lock;
use crate::conformance::NotifierProbe;
use crate::{Notification, Notifier, NotifyError};

/// Most notifications the fake remembers; older ones are dropped.
const KEEP: usize = 1024;

/// Records what it was asked to send.
#[derive(Clone, Default)]
pub struct FakeNotifier {
    sent: Arc<Mutex<VecDeque<Notification>>>,
}

impl std::fmt::Debug for FakeNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeNotifier").finish_non_exhaustive()
    }
}

impl FakeNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sent(&self) -> Vec<Notification> {
        lock(&self.sent).iter().cloned().collect()
    }
}

#[async_trait]
impl Notifier for FakeNotifier {
    async fn notify(&self, n: Notification) -> Result<(), NotifyError> {
        let mut q = lock(&self.sent);
        if q.len() >= KEEP {
            q.pop_front();
        }
        q.push_back(n);
        Ok(())
    }
}

#[async_trait]
impl NotifierProbe for FakeNotifier {
    async fn delivered(&self) -> Vec<Notification> {
        self.sent()
    }
}
