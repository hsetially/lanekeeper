//! Telling the hub about sync windows (T10, D72, D75, A21).
//!
//! The wire has one carrier for window events: the `sync_windows` list of a [`ClusterReport`]. The announcer sends them
//! as reports of their own (a delta of the cluster picture with nothing else in it), and the first full report of a
//! connection takes the same list from [`WindowAnnouncer::due`].
//!
//! # The rule for a close
//!
//! The hub holds its drift alerts while a window is open and evaluates once when it closes, so a close that arrives before
//! the last change made during the window would be evaluated too early. A close is therefore sent only when all of this
//! holds:
//!
//! 1. the scanner has finished a walk that started after the close was seen (so every file the Job wrote has been looked
//!    for, [`Progress::walked`]);
//! 2. the scanner is holding nothing back (a held batch may be tagged with this window, [`Progress::held`]);
//! 3. the spool is drained: the pump has queued every delta it holds on this connection ([`Spool::drained`]). The
//!    outbox is a queue, so a report queued after that comes after those deltas on the stream.
//!
//! The same rule covers the first report after a reconnect (A21): window events have no acknowledgement, so a hub that
//! was away may have missed any of them. Each connection starts with an empty [`Told`], hears of every window in the
//! ledger (open ones, and those closed in the last hour, at most 64), and hears of the closes once the replay of the
//! spool has drained.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use domain::{ClusterReport, SyncWindowEvent, SyncWindowKind};
use proto::convert::FromAgent;
use tracing::debug;

use super::{WindowLedger, WindowView};
use crate::scan::{Progress, Scanner};
use crate::spool::Spool;
use crate::transport::{Outbox, OutboxError};

/// What one connection has been told, by window id.
#[derive(Debug, Default)]
pub struct Told {
    opened: BTreeSet<u64>,
    closed: BTreeSet<u64>,
}

/// Decides which window events may go to the hub now, and sends them.
pub struct WindowAnnouncer {
    ledger: Arc<WindowLedger>,
    scanner: Scanner,
    spool: Spool,
}

impl std::fmt::Debug for WindowAnnouncer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowAnnouncer").finish_non_exhaustive()
    }
}

impl WindowAnnouncer {
    pub fn new(ledger: Arc<WindowLedger>, scanner: Scanner, spool: Spool) -> Self {
        Self {
            ledger,
            scanner,
            spool,
        }
    }

    /// May the hub be told that this window closed?
    fn may_close(&self, view: &WindowView, progress: &Progress) -> bool {
        let Some(closed_gen) = view.closed_gen else {
            return false;
        };
        progress.walked.is_some_and(|walked| walked >= closed_gen) && !progress.held && self.spool.drained()
    }

    /// The events this connection has not been told yet and may be told now. They are marked as told.
    pub fn due(&self, told: &mut Told) -> Vec<SyncWindowEvent> {
        let progress = self.scanner.progress().borrow().clone();
        let mut events = Vec::new();
        for view in self.ledger.views() {
            if told.opened.insert(view.id) {
                events.push(SyncWindowEvent {
                    kind: SyncWindowKind::Opened,
                    job: view.job.clone(),
                    at: view.opened_at,
                });
            }
            if let Some(closed_at) = view.closed_at {
                if !told.closed.contains(&view.id) && self.may_close(&view, &progress) {
                    told.closed.insert(view.id);
                    events.push(SyncWindowEvent {
                        kind: SyncWindowKind::Closed,
                        job: view.job.clone(),
                        at: closed_at,
                    });
                }
            }
        }
        events
    }

    /// Everything a hub that has been told nothing may be told now: for a full report that a hub asked for.
    pub fn snapshot(&self) -> Vec<SyncWindowEvent> {
        self.due(&mut Told::default())
    }

    /// Send window events to `outbox` as they become due, until the connection ends. `told` is shared with whoever puts
    /// the first events into the connection's full report.
    pub async fn run(&self, outbox: &Outbox, told: &Mutex<Told>) {
        let mut ledger = self.ledger.subscribe();
        let mut progress = self.scanner.progress();
        let mut drain = self.spool.subscribe_drain();
        // The highest close for which a walk has been asked, so that one close asks once.
        let mut asked: u64 = 0;
        loop {
            // Anything that changes after this point wakes the waits below, so nothing slips between looking and waiting.
            ledger.borrow_and_update();
            progress.borrow_and_update();
            drain.borrow_and_update();

            let events = self.due(&mut told.lock().unwrap_or_else(PoisonError::into_inner));
            if !events.is_empty() {
                let report = ClusterReport {
                    full: false,
                    deployments: Vec::new(),
                    release_hints: Vec::new(),
                    config_server_started_at: None,
                    sync_windows: events,
                };
                match outbox.send(FromAgent::Cluster(report)).await {
                    Ok(()) => {}
                    Err(OutboxError::TooLarge | OutboxError::Full) => {
                        debug!("a window report could not be queued");
                    }
                    Err(OutboxError::Closed) => return,
                }
            }

            // A close needs a walk that started after it. Do not wait for the one on the grid.
            let walked = progress.borrow().walked;
            let wanted = self
                .ledger
                .views()
                .iter()
                .filter_map(|view| view.closed_gen)
                .filter(|closed| walked.is_none_or(|walked| walked < *closed))
                .max();
            if let Some(closed) = wanted {
                if closed > asked {
                    asked = closed;
                    self.scanner.kick();
                }
            }

            tokio::select! {
                r = ledger.changed() => if r.is_err() { return },
                r = progress.changed() => if r.is_err() { return },
                r = drain.changed() => if r.is_err() { return },
            }
        }
    }
}
