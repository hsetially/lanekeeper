//! Graceful shutdown (T1): readiness fails first, connections stop after a grace period, streams are told to go.
//!
//! The phases are one-way: `Running`, then `NotReady` (the load balancer sees `/readyz` fail and stops sending new work
//! while existing requests carry on), then `Stopping` (listeners stop accepting; SSE and agent streams close with a reason
//! so their clients reconnect elsewhere). Anything long-lived subscribes to the phase and ends when it reaches `Stopping`.
//!
//! Two more pieces belong to the process (plan 18, T1). [`Drain::closing`] is the same moment in the shape the platform's
//! services take (`watch::Receiver<bool>`, true once closing). [`Stop`] is where "please stop" comes from: the operating
//! system's SIGTERM and SIGINT in `main`, a channel in tests. Its signal streams are made before anything else runs, because a
//! handler installed at the first poll leaves a window in which a signal kills the process mid-start (03b's blockers file).

use std::io;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

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
    closing: watch::Sender<bool>,
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
            closing: watch::Sender::new(false),
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
        self.closing.send_replace(true);
    }

    /// The platform services' notion of "closing": false until the phase reaches `Stopping`, true from then on, also for a
    /// receiver made after it. The services (the SSE stream, the agent and sentinel streams, the job runner) take this shape.
    #[must_use]
    pub fn closing(&self) -> watch::Receiver<bool> {
        self.closing.subscribe()
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

/// Where "please stop" comes from. Every request is one event; a second one while stopping means "now".
#[derive(Debug)]
pub struct Stop {
    source: Source,
}

#[derive(Debug)]
enum Source {
    Channel(mpsc::Receiver<()>),
    #[cfg(unix)]
    Os {
        term: tokio::signal::unix::Signal,
        interrupt: tokio::signal::unix::Signal,
    },
}

/// Asks a [`Stop`] to stop, as a signal would. For tests and for the dev example.
#[derive(Debug, Clone)]
pub struct StopHandle(mpsc::Sender<()>);

impl StopHandle {
    /// One request. A full queue already holds more requests than anyone will read, so the extra one is dropped.
    pub fn stop(&self) {
        let _ = self.0.try_send(());
    }
}

impl Stop {
    /// A source that stops when the handle says so.
    #[must_use]
    pub fn channel() -> (StopHandle, Self) {
        let (tx, rx) = mpsc::channel(8);
        (
            StopHandle(tx),
            Self {
                source: Source::Channel(rx),
            },
        )
    }

    /// SIGTERM and SIGINT. The streams exist when this returns, so a signal that arrives before anyone waits is kept and the
    /// next [`Stop::requested`] sees it. Call it inside the runtime and before anything is spawned.
    #[cfg(unix)]
    pub fn os_signals() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            source: Source::Os {
                term: signal(SignalKind::terminate())?,
                interrupt: signal(SignalKind::interrupt())?,
            },
        })
    }

    /// There are no signals to wait for on this platform.
    #[cfg(not(unix))]
    pub fn os_signals() -> io::Result<Self> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// Resolves at the next request. Safe to drop and call again. If the source is gone, it never resolves: a hub whose
    /// signal stream ended is not one that should stop on its own.
    pub async fn requested(&mut self) {
        match &mut self.source {
            Source::Channel(rx) => {
                if rx.recv().await.is_none() {
                    std::future::pending::<()>().await;
                }
            }
            #[cfg(unix)]
            Source::Os { term, interrupt } => {
                let seen = tokio::select! {
                    seen = term.recv() => seen,
                    seen = interrupt.recv() => seen,
                };
                if seen.is_none() {
                    std::future::pending::<()>().await;
                }
            }
        }
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

    #[test]
    fn closing_turns_true_at_stopping_and_not_before() {
        let drain = Drain::new();
        let closing = drain.closing();
        assert!(!*closing.borrow());
        drain.begin_not_ready();
        assert!(!*closing.borrow(), "not ready is not closing");
        drain.begin_stopping();
        assert!(*closing.borrow());
        // A receiver made late still reads true, and so does a clone of the drain's.
        assert!(*drain.closing().borrow());
        assert!(*drain.clone().closing().borrow());
    }

    #[tokio::test]
    async fn a_closing_receiver_wakes_the_task_that_waits_on_it() {
        let drain = Drain::new();
        let mut closing = drain.closing();
        let waiter = tokio::spawn(async move { closing.wait_for(|c| *c).await.is_ok() });
        tokio::task::yield_now().await;
        drain.begin_stopping();
        assert!(waiter.await.unwrap_or(false));
    }

    #[tokio::test]
    async fn each_stop_request_is_one_event() {
        let (handle, mut stop) = Stop::channel();
        handle.stop();
        handle.stop();
        tokio::time::timeout(Duration::from_secs(1), stop.requested())
            .await
            .unwrap_or_default();
        tokio::time::timeout(Duration::from_secs(1), stop.requested())
            .await
            .unwrap_or_default();
        // Two requests were made and both are used up: a third wait does not resolve.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stop.requested())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn nothing_asked_means_nothing_resolves_and_a_dropped_handle_does_not_stop() {
        let (handle, mut stop) = Stop::channel();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stop.requested())
                .await
                .is_err()
        );
        drop(handle);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stop.requested())
                .await
                .is_err(),
            "a hub does not stop because its stop handle was dropped"
        );
    }

    /// The 03b race: a SIGTERM that arrives between installing the handler and the first wait must not be lost, and must not
    /// kill the process. Needs the `kill` program, which every Linux image the tests run on has.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_signal_before_the_first_wait_is_kept() {
        let mut stop = Stop::os_signals().unwrap_or_else(|_| Stop::channel().1);
        let status = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status();
        assert!(status.is_ok_and(|s| s.success()), "could not run kill");
        tokio::time::timeout(Duration::from_secs(5), stop.requested())
            .await
            .unwrap_or_else(|_| unreachable!("the signal was lost"));
    }
}
