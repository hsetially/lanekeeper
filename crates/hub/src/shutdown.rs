//! Graceful shutdown (T1): readiness fails first, connections stop after a grace period, streams are told to go.
//!
//! The phases are one-way: `Running`, then `NotReady` (the load balancer sees `/readyz` fail and stops sending new work
//! while existing requests carry on), then `Stopping` (listeners stop accepting; SSE and agent streams close with a reason
//! so their clients reconnect elsewhere). Anything long-lived subscribes to the phase and ends when it reaches `Stopping`.

use std::time::Duration;

use tokio::sync::watch;

/// How far the hub has got in shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Running,
    NotReady,
    Stopping,
}

/// The shutdown switch, shared by every listener and stream.
#[derive(Debug, Clone)]
pub struct Drain {
    phase: watch::Sender<Phase>,
}

impl Default for Drain {
    fn default() -> Self {
        Self::new()
    }
}

impl Drain {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: watch::Sender::new(Phase::Running),
        }
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        *self.phase.borrow()
    }

    /// Follow the phase. A stream selects on `changed()` and ends when it reads `Stopping`.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Phase> {
        self.phase.subscribe()
    }

    /// Readiness fails from now on. Never moves the phase backwards.
    pub fn begin_not_ready(&self) {
        self.phase.send_if_modified(|p| {
            if *p < Phase::NotReady {
                *p = Phase::NotReady;
                true
            } else {
                false
            }
        });
    }

    /// Stop accepting and close the streams.
    pub fn begin_stopping(&self) {
        self.phase.send_if_modified(|p| {
            if *p < Phase::Stopping {
                *p = Phase::Stopping;
                true
            } else {
                false
            }
        });
    }

    /// The whole sequence on `SIGTERM`: fail readiness, wait `grace`, then stop.
    pub async fn shutdown(&self, grace: Duration) {
        self.begin_not_ready();
        tokio::time::sleep(grace).await;
        self.begin_stopping();
    }

    /// Resolves when the phase is `Stopping`.
    pub async fn stopping(&self) {
        let mut rx = self.subscribe();
        // `wait_for` returns an error only if the sender is gone, which cannot happen while `self` is alive.
        let _ = rx.wait_for(|p| *p == Phase::Stopping).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn readiness_fails_first_and_stopping_comes_after_the_grace_period() {
        let drain = Drain::new();
        let mut rx = drain.subscribe();
        assert_eq!(drain.phase(), Phase::Running);
        let d = drain.clone();
        let task = tokio::spawn(async move { d.shutdown(Duration::from_secs(5)).await });
        rx.changed().await.unwrap_or_default();
        assert_eq!(*rx.borrow(), Phase::NotReady);
        tokio::time::advance(Duration::from_secs(4)).await;
        assert_eq!(drain.phase(), Phase::NotReady, "still in the grace period");
        tokio::time::advance(Duration::from_secs(2)).await;
        task.await.unwrap_or_default();
        assert_eq!(drain.phase(), Phase::Stopping);
    }

    #[tokio::test]
    async fn phases_never_move_backwards() {
        let drain = Drain::new();
        drain.begin_stopping();
        drain.begin_not_ready();
        assert_eq!(drain.phase(), Phase::Stopping);
    }

    #[tokio::test]
    async fn stopping_resolves_for_a_late_subscriber() {
        let drain = Drain::new();
        drain.begin_stopping();
        drain.stopping().await;
    }
}
