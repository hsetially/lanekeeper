//! The scan loop (T4, D63, D27, P1, P3): walk the root, keep the tree, push what changed, answer what the hub asks.
//!
//! # Timing
//!
//! - **Every 10 s** (the hub can slow this down to 300 s, never speed it up past 5 s) the root is stat-walked: only files
//!   whose size, mtime, ctime or inode changed are read. Walks are on a fixed grid, so a slow walk does not push the next
//!   one later.
//! - **Every 15 min** everything is rehashed, because NFS attribute caching can show an old stat for a file that changed.
//! - **At the walk that finds a change**, the difference between the old tree and the new one goes to the hub as a delta
//!   (agent-initiated). So an out-of-band change reaches the hub about one walk after it was made: at most 10 s plus the
//!   walk, read and send, against the 15 s of P1. (T10 adds the quiet period that batches a burst; this loop pushes at
//!   once.)
//! - **Every 10 s**, on a connection, a heartbeat carries the root, the file count and the walk number: 6 messages a
//!   minute, which is what P3's 1 KB a minute is spent on.
//!
//! # What is kept
//!
//! One tree, and the [`RootRing`] of the roots of the last hour. A hub that asks for `RequestDelta(since_root)` gets the
//! difference from that root, or a full listing if the root is older than the ring; both are answered from the tree on
//! the connection that asked, with fresh sequence numbers, not from any queue.
//!
//! # What is never believed
//!
//! - A failed walk (a stale mount, too many entries) changes nothing.
//! - A root that suddenly lists as empty is held for the 30 s maximum deferral before it is accepted, so an NFS mount
//!   that dropped does not look like the deletion of every file.
//!
//! The scanner keeps running while no hub is connected: the tree stays current, and what changed meanwhile reaches the hub
//! through the next heartbeat's root, which differs from the one the hub knows, and the `RequestDelta` that follows. (T9
//! adds the spool, so intermediate versions survive a long outage as well.)

use std::convert::Infallible;
use std::fmt;
use std::future::pending;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use domain::{ContentHash, Heartbeat, HubCommand, ScanDelta};
use proto::convert::{FromAgent, ToAgent};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval, interval_at};
use tracing::{debug, info, warn};

use crate::clock::Clock;
use crate::config::Tunables;
use crate::transport::session::{Link, LinkHandler};
use crate::transport::{Outbox, OutboxError};
use crate::tree::delta::{DeltaBuilder, DeltaError, DeltaOutcome, DeltaSink, SeqCounter, SinkError};
use crate::tree::{MerkleTree, RootRing, ScanMode, ScanOutcome, TreeSource};

/// Hub requests answered at the same time. More than this are dropped (the hub asks again): rule 5.
const MAX_PARALLEL_REQUESTS: usize = 4;

/// What a heartbeat reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// Walks completed since the agent started.
    pub scan_seq: u64,
    pub root: ContentHash,
    pub file_count: u64,
}

impl Snapshot {
    pub fn heartbeat(&self) -> Heartbeat {
        Heartbeat {
            scan_seq: self.scan_seq,
            merkle_root: self.root,
            file_count: self.file_count,
        }
    }
}

/// Counters for metrics (T7) and tests.
#[derive(Debug, Default)]
pub struct ScannerStats {
    scans: AtomicU64,
    full_scans: AtomicU64,
    scan_errors: AtomicU64,
    unreadable_entries: AtomicU64,
    deltas_pushed: AtomicU64,
    delta_failures: AtomicU64,
    deltas_skipped_offline: AtomicU64,
    empty_root_holds: AtomicU64,
    dropped_requests: AtomicU64,
}

macro_rules! counter {
    ($($name:ident),*) => {
        impl ScannerStats {
            $(pub fn $name(&self) -> u64 { self.$name.load(Ordering::Relaxed) })*
        }
    };
}
counter!(
    scans,
    full_scans,
    scan_errors,
    unreadable_entries,
    deltas_pushed,
    delta_failures,
    deltas_skipped_offline,
    empty_root_holds,
    dropped_requests
);

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, thiserror::Error)]
pub enum ScanRequestError {
    /// The first walk has not finished: there is no tree to answer from.
    #[error("the first scan has not finished")]
    NotReady,
    #[error(transparent)]
    Delta(#[from] DeltaError),
}

struct State {
    tree: Option<MerkleTree>,
    ring: RootRing,
    scan_seq: u64,
    last_full: Option<Instant>,
    /// Since when every walk has found an empty root, while the tree holds files.
    empty_since: Option<Instant>,
}

struct Inner {
    source: Arc<dyn TreeSource>,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn DeltaSink>,
    seq: SeqCounter,
    tunables: watch::Sender<Tunables>,
    state: AsyncMutex<State>,
    snapshot: watch::Sender<Option<Snapshot>>,
    stats: ScannerStats,
}

/// The tree of the NFS root and the loop that keeps it current. Cheap to clone; all clones are the same scanner.
#[derive(Clone)]
pub struct Scanner {
    inner: Arc<Inner>,
}

impl fmt::Debug for Scanner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scanner")
            .field("snapshot", &*self.inner.snapshot.borrow())
            .finish_non_exhaustive()
    }
}

impl Scanner {
    /// `sink` receives the deltas the scanner pushes on its own: the live connection, or the spool (T9).
    pub fn new(source: Arc<dyn TreeSource>, clock: Arc<dyn Clock>, sink: Arc<dyn DeltaSink>) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                clock,
                sink,
                seq: SeqCounter::new(1),
                tunables: watch::channel(Tunables::default()).0,
                state: AsyncMutex::new(State {
                    tree: None,
                    ring: RootRing::default(),
                    scan_seq: 0,
                    last_full: None,
                    empty_since: None,
                }),
                snapshot: watch::channel(None).0,
                stats: ScannerStats::default(),
            }),
        }
    }

    pub fn stats(&self) -> &ScannerStats {
        &self.inner.stats
    }

    /// The root, file count and walk number of the latest tree; `None` until the first walk finishes. Never waits for a
    /// walk in progress.
    pub fn snapshot(&self) -> Option<Snapshot> {
        *self.inner.snapshot.borrow()
    }

    /// Wake up when the snapshot changes.
    pub fn subscribe(&self) -> watch::Receiver<Option<Snapshot>> {
        self.inner.snapshot.subscribe()
    }

    /// The hub's settings. The walk interval takes effect at once; the file limit with the next delta.
    pub fn set_tunables(&self, tunables: Tunables) {
        self.inner.tunables.send_if_modified(|current| {
            if *current == tunables {
                false
            } else {
                *current = tunables;
                true
            }
        });
    }

    /// Walk now and then every scan interval, for as long as this future is polled. The first walk is the baseline: it
    /// sends no delta, because there is nothing to compare with.
    pub async fn run(&self) -> Infallible {
        let mut tunables = self.inner.tunables.subscribe();
        let mut period = tunables.borrow_and_update().scan_interval;
        let mut ticker = ticker_now(period);
        loop {
            tokio::select! {
                _ = ticker.tick() => self.scan_once().await,
                changed = tunables.changed() => {
                    if changed.is_err() {
                        // The sender lives in `self`; there is nothing more to wait for.
                        return pending::<Infallible>().await;
                    }
                    let wanted = tunables.borrow_and_update().scan_interval;
                    if wanted != period {
                        period = wanted;
                        ticker = ticker_after(period);
                    }
                }
            }
        }
    }

    /// One walk, and everything that follows from it. Public for tests; [`Scanner::run`] calls it.
    pub async fn scan_once(&self) {
        let inner = &self.inner;
        let mut state = inner.state.lock().await;
        let now = inner.clock.instant();
        let tunables = inner.tunables.borrow().clone();
        state.ring.evict(now);

        let full_due = state
            .last_full
            .is_none_or(|at| now.saturating_duration_since(at) >= tunables.full_rehash_interval);
        let mode = if full_due { ScanMode::Full } else { ScanMode::Stat };
        let previous = state.tree.clone();
        let outcome = match inner.source.scan(previous.clone(), mode).await {
            Ok(outcome) => outcome,
            Err(error) => {
                bump(&inner.stats.scan_errors);
                warn!(%error, "the scan failed; the tree is unchanged");
                return;
            }
        };
        let ScanOutcome {
            tree: scanned,
            new_bytes,
            stats,
        } = outcome;
        bump(&inner.stats.scans);
        if mode == ScanMode::Full {
            bump(&inner.stats.full_scans);
        }
        inner
            .stats
            .unreadable_entries
            .fetch_add(stats.errors, Ordering::Relaxed);
        state.scan_seq += 1;
        if full_due {
            state.last_full = Some(now);
        }

        // An empty root where there were files is more likely a dropped mount than a deletion of everything.
        if scanned.root().entries().is_empty() && previous.as_ref().is_some_and(|t| t.file_count() > 0) {
            let since = *state.empty_since.get_or_insert(now);
            if now.saturating_duration_since(since) < tunables.max_defer {
                bump(&inner.stats.empty_root_holds);
                warn!(
                    "the NFS root lists as empty; holding the tree until it has been empty for the maximum deferral"
                );
                return;
            }
            info!("the NFS root has stayed empty; accepting it");
        }
        state.empty_since = None;

        match previous {
            None => {
                state.ring.push(scanned.clone(), new_bytes, now);
                state.tree = Some(scanned);
            }
            Some(previous) if previous.root_hash() == scanned.root_hash() => {
                // Same content; the stat fields may have moved (a touch), and the next walk should compare with them.
                state.tree = Some(scanned);
            }
            Some(_) if !inner.sink.ready() => {
                // Nobody to send to: remember the tree and the root, read no file. The hub learns the root from the
                // heartbeat when it is back, and asks for what it is missing.
                state.ring.push(scanned.clone(), new_bytes, now);
                state.tree = Some(scanned);
                bump(&inner.stats.deltas_skipped_offline);
            }
            Some(previous) => {
                state.ring.push(scanned.clone(), new_bytes, now);
                let sent = self
                    .build(&tunables, Some(&previous), &scanned, inner.sink.as_ref())
                    .await;
                match sent {
                    Ok(outcome) => {
                        bump(&inner.stats.deltas_pushed);
                        remember(&mut state.ring, outcome.trees, now);
                        state.tree = Some(outcome.tree);
                    }
                    Err(error) => {
                        // Not delivered (no connection, or it broke). The tree is still right; the hub learns the
                        // new root from the next heartbeat and asks for the difference.
                        bump(&inner.stats.delta_failures);
                        debug!(%error, "the delta was not delivered");
                        state.tree = Some(scanned);
                    }
                }
            }
        }
        publish(inner, &state);
    }

    /// Answer a hub request: the difference from `since` (a full listing when `since` is `None` or older than the
    /// ring), sent to `sink`.
    pub async fn answer_delta(
        &self,
        since: Option<ContentHash>,
        sink: &dyn DeltaSink,
    ) -> Result<(), ScanRequestError> {
        let inner = &self.inner;
        let mut state = inner.state.lock().await;
        let Some(current) = state.tree.clone() else {
            return Err(ScanRequestError::NotReady);
        };
        let tunables = inner.tunables.borrow().clone();
        let base = since.and_then(|root| state.ring.get(&root).cloned());
        if since.is_some() && base.is_none() {
            info!("the hub asked for a root that is no longer remembered; sending a full listing");
        }
        let outcome = self.build(&tunables, base.as_ref(), &current, sink).await?;
        let now = inner.clock.instant();
        remember(&mut state.ring, outcome.trees, now);
        state.tree = Some(outcome.tree);
        publish(inner, &state);
        Ok(())
    }

    async fn build(
        &self,
        tunables: &Tunables,
        base: Option<&MerkleTree>,
        tree: &MerkleTree,
        sink: &dyn DeltaSink,
    ) -> Result<DeltaOutcome, DeltaError> {
        let inner = &self.inner;
        DeltaBuilder::new(inner.source.as_ref(), inner.clock.as_ref(), &inner.seq)
            .with_max_file_bytes(tunables.max_file_bytes)
            .send(base, tree, sink)
            .await
    }
}

fn remember(ring: &mut RootRing, trees: Vec<crate::tree::delta::Remembered>, now: Instant) {
    for remembered in trees {
        ring.push(remembered.tree, remembered.new_bytes, now);
    }
}

fn publish(inner: &Inner, state: &State) {
    if let Some(tree) = &state.tree {
        inner.snapshot.send_replace(Some(Snapshot {
            scan_seq: state.scan_seq,
            root: tree.root_hash(),
            file_count: tree.file_count(),
        }));
    }
}

/// A ticker whose first tick is now, and which waits rather than bursting when a tick is missed.
fn ticker_now(period: std::time::Duration) -> Interval {
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

/// The same, but the first tick is one period from now.
fn ticker_after(period: std::time::Duration) -> Interval {
    let mut ticker = interval_at(Instant::now() + period, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker
}

// ------------------------------------------------------------------------------------------ sinks

/// A delta sink in front of one connection's outbox.
#[derive(Debug, Clone)]
pub struct OutboxSink(pub Outbox);

#[async_trait]
impl DeltaSink for OutboxSink {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError> {
        self.0
            .send(FromAgent::Delta(delta))
            .await
            .map_err(|error| match error {
                OutboxError::TooLarge => SinkError::TooLarge,
                OutboxError::Full | OutboxError::Closed => SinkError::Closed,
            })
    }
}

/// The delta sink for what the scanner pushes on its own: whichever connection is current. With none, a delta is
/// refused ([`SinkError::Closed`]) and the hub catches up through the heartbeat root. T9 replaces this with the spool.
#[derive(Debug, Default)]
pub struct LiveSink {
    current: Mutex<Option<(u64, Outbox)>>,
    generation: AtomicU64,
}

impl LiveSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Make `outbox` the current connection until the guard is dropped.
    pub fn attach(self: &Arc<Self>, outbox: Outbox) -> Attached {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Some((generation, outbox));
        Attached {
            sink: Arc::clone(self),
            generation,
        }
    }
}

/// Keeps an outbox attached to a [`LiveSink`]. A later connection's guard is not undone by an earlier one's.
#[derive(Debug)]
pub struct Attached {
    sink: Arc<LiveSink>,
    generation: u64,
}

impl Drop for Attached {
    fn drop(&mut self) {
        let mut current = self.sink.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_some_and(|(g, _)| *g == self.generation) {
            *current = None;
        }
    }
}

#[async_trait]
impl DeltaSink for LiveSink {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError> {
        let outbox = self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(_, outbox)| outbox.clone());
        match outbox {
            Some(outbox) => OutboxSink(outbox).deliver(delta).await,
            None => Err(SinkError::Closed),
        }
    }

    fn ready(&self) -> bool {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

// ------------------------------------------------------------------------------------------ the link handler

/// The agent's side of one connection, as far as scanning goes: attach the connection so pushed deltas reach it, send
/// the heartbeat, and answer `RequestDelta` and `RequestFullScan`.
///
/// Other commands (file operations, cluster reports, config-server calls) arrive in later tasks; they are ignored here.
pub struct ScanHandler {
    scanner: Scanner,
    live: Arc<LiveSink>,
}

impl fmt::Debug for ScanHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanHandler").finish_non_exhaustive()
    }
}

impl ScanHandler {
    /// `live` must be the sink the scanner was built with.
    pub fn new(scanner: Scanner, live: Arc<LiveSink>) -> Self {
        Self { scanner, live }
    }

    /// Send a heartbeat if there is a tree to report. A heartbeat that does not fit the queue is dropped: a late one is
    /// worth nothing.
    fn heartbeat(&self, outbox: &Outbox) -> bool {
        let Some(snapshot) = self.scanner.snapshot() else {
            return false;
        };
        outbox
            .try_send(FromAgent::Heartbeat(snapshot.heartbeat()))
            .is_ok()
    }

    fn request(&self, since: Option<ContentHash>, outbox: &Outbox, tasks: &mut JoinSet<()>) {
        if tasks.len() >= MAX_PARALLEL_REQUESTS {
            bump(&self.scanner.inner.stats.dropped_requests);
            warn!("too many scan requests at once; this one is dropped");
            return;
        }
        let scanner = self.scanner.clone();
        let sink = OutboxSink(outbox.clone());
        tasks.spawn(async move {
            match scanner.answer_delta(since, &sink).await {
                Ok(()) => {}
                Err(ScanRequestError::NotReady) => debug!("asked for a delta before the first scan"),
                Err(ScanRequestError::Delta(error)) => {
                    debug!(%error, "the requested delta was not delivered");
                }
            }
        });
    }
}

#[async_trait]
impl LinkHandler for ScanHandler {
    async fn handle(&self, mut link: Link) {
        let _attached = self.live.attach(link.outbox.clone());
        self.scanner.set_tunables(link.tunables.clone());
        let mut heartbeat_period = link.tunables.heartbeat_interval;
        let mut heartbeat = ticker_now(heartbeat_period);
        let mut snapshots = self.scanner.subscribe();
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut reported = false;
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    reported |= self.heartbeat(&link.outbox);
                }
                // The first walk finished after the link came up: do not wait for the next tick to say so.
                changed = snapshots.changed(), if !reported => {
                    if changed.is_err() {
                        return;
                    }
                    reported |= self.heartbeat(&link.outbox);
                }
                message = link.inbound.recv() => {
                    let Some(message) = message else { return };
                    match message {
                        ToAgent::Config(config) => {
                            let tunables = Tunables::from_hub(&config);
                            if tunables.heartbeat_interval != heartbeat_period {
                                heartbeat_period = tunables.heartbeat_interval;
                                heartbeat = ticker_after(heartbeat_period);
                            }
                            self.scanner.set_tunables(tunables);
                        }
                        ToAgent::Command(HubCommand::RequestDelta { since_root }) => {
                            self.request(Some(since_root), &link.outbox, &mut tasks);
                        }
                        ToAgent::Command(HubCommand::RequestFullScan) => {
                            self.request(None, &link.outbox, &mut tasks);
                        }
                        // Acknowledgements matter once there is a spool (T9); nothing waits for them yet.
                        ToAgent::Ack(_) | ToAgent::CertRenewal(_) => {}
                        ToAgent::Command(_) => debug!("a command for a later part of the agent; ignored here"),
                    }
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    }
}
