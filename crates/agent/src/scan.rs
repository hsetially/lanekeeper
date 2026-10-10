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
//! The scanner keeps running while no hub is connected, and every delta it builds goes to the spool ([`crate::spool`],
//! T9, D74), not to the connection. So the intermediate versions of a file survive an outage of any length (up to the
//! spool's bounds), and the connection's pump sends them, oldest first, when it is back. What the spool had to drop is
//! reported as a gap; the heartbeat's root and the `RequestDelta` that follows it are still how the hub catches up on
//! anything the spool could not keep.

use std::convert::Infallible;
use std::fmt;
use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use domain::{ContentHash, Heartbeat, HubCommand, NfsPath, ScanDelta};
use proto::convert::{FromAgent, ToAgent};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval, interval_at, timeout};
use tracing::{debug, info, warn};

use crate::clock::Clock;
use crate::config::Tunables;
use crate::dispatch::{CommandHandler, OpLimits, failure, operation_of, request_id_of};
use crate::fileops::TreeEdits;
use crate::ops::{Metrics, Outcome, ScanKind, ScanResult};
use crate::spool::{PumpEnd, Spool};
use crate::transport::session::{Link, LinkHandler};
use crate::transport::{Outbox, OutboxError};
use crate::tree::delta::{DeltaBuilder, DeltaError, DeltaOutcome, DeltaSink, SeqCounter, SinkError};
use crate::tree::{Edited, Entry, FileLeaf, MerkleTree, RootRing, ScanMode, ScanOutcome, TreeSource};
use domain::OpError;

/// Hub requests answered at the same time. More than this are dropped (the hub asks again): rule 5.
const MAX_PARALLEL_REQUESTS: usize = 4;
/// File operations (reads, writes, deletes) running at the same time. Each holds up to 2 MiB, so this is part of the
/// memory budget (P4). One more is answered `IO` at once rather than queued without bound.
const MAX_PARALLEL_COMMANDS: usize = 4;
/// The longest a file operation waits for a scan to release the tree before it gives up on telling it (the next walk
/// finds the change by its stat).
const EDIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

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
    seq: Arc<SeqCounter>,
    tunables: watch::Sender<Tunables>,
    state: AsyncMutex<State>,
    snapshot: watch::Sender<Option<Snapshot>>,
    stats: ScannerStats,
    metrics: Arc<Metrics>,
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
    /// `sink` receives the deltas the scanner pushes on its own: the spool (T9), or whatever a test collects them in.
    /// Sequence numbers start at 1.
    pub fn new(source: Arc<dyn TreeSource>, clock: Arc<dyn Clock>, sink: Arc<dyn DeltaSink>) -> Self {
        Self::with_metrics(source, clock, sink, Metrics::detached())
    }

    /// As [`Scanner::new`], reporting walk durations, the file count and delta sizes to `metrics` (T7).
    pub fn with_metrics(
        source: Arc<dyn TreeSource>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn DeltaSink>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self::with_seq(source, clock, sink, metrics, Arc::new(SeqCounter::new(1)))
    }

    /// As [`Scanner::with_metrics`], taking the sequence numbers from `seq`: the counter the spool keeps ahead of
    /// every number ever used, so that a restart never repeats one (T9).
    pub fn with_seq(
        source: Arc<dyn TreeSource>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn DeltaSink>,
        metrics: Arc<Metrics>,
        seq: Arc<SeqCounter>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                clock,
                sink,
                seq,
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
                metrics,
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
        let kind = match mode {
            ScanMode::Full => ScanKind::Full,
            ScanMode::Stat => ScanKind::Stat,
        };
        let outcome = match inner.source.scan(previous.clone(), mode).await {
            Ok(outcome) => outcome,
            Err(error) => {
                bump(&inner.stats.scan_errors);
                inner.metrics.scan(kind, ScanResult::Failed, took(inner, now));
                warn!(%error, "the scan failed; the tree is unchanged");
                return;
            }
        };
        let walked = took(inner, now);
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
                inner.metrics.scan(kind, ScanResult::Held, walked);
                warn!(
                    "the NFS root lists as empty; holding the tree until it has been empty for the maximum deferral"
                );
                return;
            }
            info!("the NFS root has stayed empty; accepting it");
        }
        state.empty_since = None;
        inner.metrics.scan(kind, ScanResult::Done, walked);

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
        let metered = MeteredSink {
            sink,
            metrics: &inner.metrics,
        };
        DeltaBuilder::new(inner.source.as_ref(), inner.clock.as_ref(), &inner.seq)
            .with_max_file_bytes(tunables.max_file_bytes)
            .send(base, tree, &metered)
            .await
    }
}

/// Time since `started` on the scanner's clock.
fn took(inner: &Inner, started: Instant) -> std::time::Duration {
    inner.clock.instant().saturating_duration_since(started)
}

fn remember(ring: &mut RootRing, trees: Vec<crate::tree::delta::Remembered>, now: Instant) {
    for remembered in trees {
        ring.push(remembered.tree, remembered.new_bytes, now);
    }
}

fn publish(inner: &Inner, state: &State) {
    if let Some(tree) = &state.tree {
        inner.metrics.files_tracked(tree.file_count());
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

// ------------------------------------------------------------------------------------------ edits from file operations

impl Scanner {
    /// Apply an edit made by a file operation to the tree, and remember the new root. If a scan is holding the tree for
    /// longer than [`EDIT_WAIT`] the edit is dropped: the file's stat differs from the tree's leaf, so the next walk
    /// reads it and the tree catches up (a walk never loses a change, it only finds it later).
    async fn edit(&self, change: impl FnOnce(&MerkleTree) -> Edited) {
        let inner = &self.inner;
        let Ok(mut state) = timeout(EDIT_WAIT, inner.state.lock()).await else {
            debug!("a scan held the tree; the edit is left for the next walk");
            return;
        };
        // No tree yet: the first walk will see the file.
        let Some(tree) = state.tree.as_ref() else {
            return;
        };
        let edited = change(tree);
        state
            .ring
            .push(edited.tree.clone(), edited.new_bytes, inner.clock.instant());
        state.tree = Some(edited.tree);
        publish(inner, &state);
    }
}

#[async_trait]
impl TreeEdits for Scanner {
    async fn written(&self, path: &NfsPath, leaf: FileLeaf) {
        self.edit(|tree| tree.put(path.as_str(), Entry::File(leaf))).await;
    }

    async fn removed(&self, path: &NfsPath) {
        self.edit(|tree| tree.remove(path.as_str())).await;
    }
}

// ------------------------------------------------------------------------------------------ sinks

/// Records the size of every delta message that is handed to the sink it wraps.
struct MeteredSink<'a> {
    sink: &'a dyn DeltaSink,
    metrics: &'a Metrics,
}

impl fmt::Debug for MeteredSink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MeteredSink")
    }
}

#[async_trait]
impl DeltaSink for MeteredSink<'_> {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError> {
        let (entries, bytes) = (delta.entries.len(), delta.payload_bytes() as u64);
        self.sink.deliver(delta).await?;
        self.metrics.delta(entries, bytes);
        Ok(())
    }

    fn ready(&self) -> bool {
        self.sink.ready()
    }
}

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

// ------------------------------------------------------------------------------------------ the link handler

/// The agent's side of one connection, as far as scanning goes: start the spool's pump on the connection so that what
/// the scanner spooled reaches the hub, pass the hub's acknowledgements to the spool, send the heartbeat, and answer
/// `RequestDelta` and `RequestFullScan`.
///
/// The other commands go to the [`CommandHandler`] if there is one (T5: file operations; cluster reports and the
/// config-server calls follow in later tasks). Without one they are ignored.
pub struct ScanHandler {
    scanner: Scanner,
    spool: Spool,
    commands: Option<Arc<dyn CommandHandler>>,
    on_config: Option<ConfigHook>,
}

/// Called with the hub's settings each time an `AgentConfig` arrives while connected, before the next message from the
/// hub is read, so a command that follows a configuration always sees its effect.
pub type ConfigHook = Arc<dyn Fn(&Tunables) + Send + Sync>;

impl fmt::Debug for ScanHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanHandler").finish_non_exhaustive()
    }
}

impl ScanHandler {
    /// `spool` must be the spool the scanner's sink writes to.
    pub fn new(scanner: Scanner, spool: Spool) -> Self {
        Self {
            scanner,
            spool,
            commands: None,
            on_config: None,
        }
    }

    /// Call `hook` for every configuration the hub sends after the first (T7: the environment allowlist).
    #[must_use]
    pub fn with_config_hook(mut self, hook: ConfigHook) -> Self {
        self.on_config = Some(hook);
        self
    }

    /// Hand every command other than `RequestDelta` and `RequestFullScan` to `commands`.
    #[must_use]
    pub fn with_commands(mut self, commands: Arc<dyn CommandHandler>) -> Self {
        self.commands = Some(commands);
        self
    }

    /// Carry out one command in its own task and send its answer. At most [`MAX_PARALLEL_COMMANDS`] run at once; one
    /// more is answered `IO` (busy) so the hub does not wait for a timeout.
    fn command(&self, command: HubCommand, max_file_bytes: u64, outbox: &Outbox, tasks: &mut JoinSet<()>) {
        let Some(handler) = &self.commands else {
            debug!("a command for a part of the agent that is not running; ignored");
            return;
        };
        if tasks.len() >= MAX_PARALLEL_COMMANDS {
            bump(&self.scanner.inner.stats.dropped_requests);
            warn!("too many commands at once; this one is answered busy");
            if let Some(operation) = operation_of(&command) {
                self.scanner.inner.metrics.operation(operation, Outcome::Io);
            }
            if let Some(request_id) = request_id_of(&command) {
                let _ = outbox.try_send(failure(request_id.clone(), OpError::Io, None));
            }
            return;
        }
        let handler = Arc::clone(handler);
        let outbox = outbox.clone();
        tasks.spawn(async move {
            if let Some(reply) = handler.handle(command, OpLimits { max_file_bytes }).await {
                if outbox.send(reply).await.is_err() {
                    debug!("the connection ended before the answer could be sent");
                }
            }
        });
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
        // The pump sends everything the spool holds, oldest first, then each new delta as it becomes durable. It lives as
        // long as this connection does.
        let mut pump = AbortOnDrop(tokio::spawn(self.spool.attach(link.outbox.clone()).run()));
        self.scanner.set_tunables(link.tunables.clone());
        let mut heartbeat_period = link.tunables.heartbeat_interval;
        let mut heartbeat = ticker_now(heartbeat_period);
        let mut snapshots = self.scanner.subscribe();
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut commands: JoinSet<()> = JoinSet::new();
        let mut max_file_bytes = link.tunables.max_file_bytes;
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
                            max_file_bytes = tunables.max_file_bytes;
                            if let Some(hook) = &self.on_config {
                                hook(&tunables);
                            }
                            self.scanner.set_tunables(tunables);
                        }
                        ToAgent::Command(HubCommand::RequestDelta { since_root }) => {
                            self.request(Some(since_root), &link.outbox, &mut tasks);
                        }
                        ToAgent::Command(HubCommand::RequestFullScan) => {
                            self.request(None, &link.outbox, &mut tasks);
                        }
                        ToAgent::Ack(seq) => self.spool.ack(seq),
                        ToAgent::CertRenewal(_) => {}
                        ToAgent::Command(command) => {
                            self.command(command, max_file_bytes, &link.outbox, &mut commands);
                        }
                    }
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                Some(_) = commands.join_next(), if !commands.is_empty() => {}
                ended = &mut pump.0 => {
                    // The connection is closed, or the spool's volume stopped answering. Either way this link is over,
                    // and the next one starts a fresh pump from what is still unacknowledged.
                    match ended {
                        Ok(PumpEnd::StorageStalled) => warn!("the spool stopped answering; ending the connection"),
                        Ok(PumpEnd::ConnectionClosed) | Err(_) => debug!("the pump ended with the connection"),
                    }
                    return;
                }
            }
        }
    }
}

/// Stops a task when dropped, so that nothing the connection started outlives it.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
