//! The notifier conformance suite fails for a notifier that does not deliver, even when the probe already holds what an earlier
//! run delivered, and it accepts a probe that keeps only the last few notifications.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use ports::conformance::{self, NotifierProbe};
use ports::fakes::FakeNotifier;
use ports::{Notification, Notifier, NotifyError};

/// Says `Ok` and delivers nothing; what the probe shows is whatever was in the shared fake before.
struct Drops {
    inner: FakeNotifier,
}

#[async_trait]
impl Notifier for Drops {
    async fn notify(&self, _: Notification) -> Result<(), NotifyError> {
        Ok(())
    }
}

#[async_trait]
impl NotifierProbe for Drops {
    async fn delivered(&self) -> Vec<Notification> {
        self.inner.sent()
    }
}

/// A correct notifier whose probe remembers only the last `keep` notifications.
struct Bounded {
    inner: FakeNotifier,
    keep: usize,
}

#[async_trait]
impl Notifier for Bounded {
    async fn notify(&self, n: Notification) -> Result<(), NotifyError> {
        self.inner.notify(n).await
    }
}

#[async_trait]
impl NotifierProbe for Bounded {
    async fn delivered(&self) -> Vec<Notification> {
        let all = self.inner.sent();
        all[all.len().saturating_sub(self.keep)..].to_vec()
    }
}

#[tokio::test]
async fn a_second_run_on_the_same_probe_passes() {
    let probe = FakeNotifier::new();
    conformance::notifier(&probe).await;
    conformance::notifier(&probe).await;
}

#[tokio::test]
#[should_panic(expected = "delivered once, in order")]
async fn a_notifier_that_drops_everything_does_not_pass_on_what_an_earlier_run_left() {
    let shared = FakeNotifier::new();
    conformance::notifier(&shared).await;
    conformance::notifier(&Drops { inner: shared }).await;
}

#[tokio::test]
async fn a_probe_that_keeps_only_the_last_few_notifications_passes() {
    for keep in [1, 3, 9, 50] {
        conformance::notifier(&Bounded {
            inner: FakeNotifier::new(),
            keep,
        })
        .await;
    }
}
