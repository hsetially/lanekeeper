//! The durable spool (T9, D74, P15, S16, S17, S22): every version the scanner observed is written to a small persistent
//! volume before it is sent, and stays there until the hub acknowledges it.
//!
//! # What it is for
//!
//! A hub that is away for an hour must not make the agent forget what happened meanwhile. A file that went A, then B,
//! then C is three versions, and the hub learns all three, in that order, when it is back, because the scanner kept
//! walking and every delta it built was spooled. Without the spool only the state at reconnect (C) reaches the hub.
//!
//! # The shape
//!
//! - **Segments** (`seg-<id>.lks`, [`segment`]): append-only files of checksummed frames ([`record`]). The agent writes
//!   to one segment at a time and starts a new one after each restart; a segment is deleted as a whole, when nothing in
//!   it is owed to the hub.
//! - **The unit is a delta.** The messages of one logical delta are sent together and in order, acknowledged part by
//!   part, released when the last part is, and dropped together when the spool is full ([`index`]).
//! - **Durable before sent.** A delta becomes sendable after one `fsync`, at its last message (fsync batching: one sync
//!   per logical delta, not per message). The hub is never told about a version the agent would forget in a crash.
//! - **Bounded.** `LK_SPOOL_MAX_BYTES` (512 MiB) and `LK_SPOOL_MAX_ENTRIES` (100,000). When a new delta does not fit,
//!   the oldest deltas are dropped, whole, and the loss is reported as a [`domain::SpoolGap`] on the next message sent: the
//!   time range gone (by `min` and `max`, so it never runs backwards) and how many entries. The hub then compares roots
//!   and asks for a full scan.
//! - **Crash-safe.** Opening reads every segment once and trusts a record only if its checksum matches. A cut or damaged
//!   tail ends its segment, is reported as lost, and costs nothing else; a delta that never finished is discarded.
//! - **At least once.** An acknowledgement that was in flight when the process died means the delta is sent again. A
//!   sequence number is never reused: the counter is reserved ahead in a small state file ([`state`]).
//! - **Dedup.** A version identical to the one before it that is still held is not stored twice. A, B, A keeps all
//!   three; only a repeat directly after its twin collapses (decision A7).
//!
//! # What it never holds
//!
//! A record is the wire message, so a denied file (D79) is a name, a size and a hash with no bytes, here as on the wire.
//! The walker and the delta builder already keep denied files' bytes out of a delta; the spool is the last guard, with the
//! same [`DenyList`] (T11): [`Spool::append`] strips the bytes from any entry whose path is denied before the record is
//! made, and the pump strips them again from a record written before the hub added a glob, so that bytes the agent held
//! for a path that has become denied never reach the connection. What was written to a segment file before the path was
//! denied stays in that file until the hub has acknowledged it and the segment is deleted (the threat model says so).
//! The spool cannot add bytes that were not in the delta it was given. Its files are created `0600`, on a volume the NFS
//! root does not contain.
//!
//! # Where it runs
//!
//! Every file operation is blocking. [`Spool`] runs them in `spawn_blocking` ([`IoMode::Blocking`]), one append at a time
//! and each cut off after [`IO_TIMEOUT`]; tests under virtual time run them inline ([`IoMode::Inline`]) so that time
//! cannot jump while a blocking task is still working. All of it goes through the cap-std handle of the spool directory
//! ([`SpoolVolume`]); the names of the files are made here and never come from a message.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::{ScanDelta, SpoolGap};
use proto::convert::FromAgent;
use tokio::sync::Semaphore;
use tokio::time::{MissedTickBehavior, interval, timeout};
use tracing::{debug, warn};

use crate::clock::Clock;
use crate::deny::DenyList;
use crate::ops::Metrics;
use crate::transport::{Outbox, OutboxError};
use crate::tree::delta::{DeltaSink, SeqCounter, SinkError};

mod core;
pub mod index;
pub mod record;
mod recover;
pub mod segment;
pub mod state;
mod volume;

pub use volume::SpoolVolume;

use self::core::Core;

/// Longest one spool operation may take before the caller gives up on it. A volume that hangs must not hang the scanner.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// How often housekeeping runs when nothing asks for it: deleting released segments, keeping the sequence reservation
/// ahead of the counter, writing down a changed gap.
pub const MAINTENANCE_PERIOD: Duration = Duration::from_secs(1);
/// Messages the pump sends before the hub has acknowledged any of them, and the bytes they may hold. Bounds what a slow
/// hub can make the agent keep in flight (rule 5).
const IN_FLIGHT_MESSAGES: usize = 64;
const IN_FLIGHT_BYTES: u64 = 16 * 1024 * 1024;

/// Why a spool operation failed. Never carries a path or a file's content (S10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpoolError {
    #[error("the spool directory could not be opened: {0:?}")]
    Volume(io::ErrorKind),
    #[error("a spool file operation failed: {0:?}")]
    Io(io::ErrorKind),
    #[error("the message is larger than a wire message")]
    TooLarge,
    #[error("sequence number {got} does not follow {last}")]
    OutOfOrder { got: u64, last: u64 },
    /// A simulated crash (tests): the spool stays dead, as a killed process would.
    #[error("the spool stopped after a simulated crash")]
    Crashed,
    #[error("the spool operation did not finish")]
    Interrupted,
    #[error("the spool operation took longer than {} s", IO_TIMEOUT.as_secs())]
    TimedOut,
}

/// The bounds. `segment_bytes` is the size at which a segment is closed and the next started; dropping works on whole
/// deltas, and a segment is freed when all of its records are, so smaller segments free space sooner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpoolLimits {
    pub max_bytes: u64,
    pub max_entries: u64,
    pub segment_bytes: u64,
}

impl SpoolLimits {
    const SEGMENT_MIN: u64 = 64 * 1024;
    const SEGMENT_MAX: u64 = 4 * 1024 * 1024;

    /// An eighth of the byte bound per segment, between 64 KiB and 4 MiB.
    pub fn new(max_bytes: u64, max_entries: u64) -> Self {
        Self {
            max_bytes,
            max_entries,
            segment_bytes: (max_bytes / 8).clamp(Self::SEGMENT_MIN, Self::SEGMENT_MAX),
        }
    }

    #[must_use]
    pub fn with_segment_bytes(mut self, segment_bytes: u64) -> Self {
        self.segment_bytes = segment_bytes;
        self
    }

    fn as_index_limits(self) -> index::Limits {
        index::Limits {
            max_bytes: self.max_bytes,
            max_entries: self.max_entries,
            segment_bytes: self.segment_bytes,
        }
    }
}

/// Where the blocking file operations run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IoMode {
    /// On the blocking pool, one at a time, with a timeout. What the agent uses.
    #[default]
    Blocking,
    /// On the thread that asks. For tests under `tokio::time::pause`.
    Inline,
}

/// Points where a test can interfere. Production passes none.
pub trait SpoolHooks: Send + Sync + 'static {
    /// About to write a frame of `len` bytes. `Some(n)` writes only its first `n` bytes and then plays dead: what a
    /// process killed in the middle of a write leaves behind.
    fn torn_write(&self, _len: usize) -> Option<usize> {
        None
    }

    /// About to write: `true` makes this write fail with an I/O error, as a full disk would.
    fn fail_write(&self) -> bool {
        false
    }

    /// A segment was synced to the device.
    fn synced(&self) {}
}

/// How a spool is opened.
#[derive(Clone)]
pub struct SpoolOptions {
    pub limits: SpoolLimits,
    pub io: IoMode,
    pub metrics: Arc<Metrics>,
    pub hooks: Option<Arc<dyn SpoolHooks>>,
}

impl fmt::Debug for SpoolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpoolOptions")
            .field("limits", &self.limits)
            .field("io", &self.io)
            .finish_non_exhaustive()
    }
}

impl SpoolOptions {
    pub fn new(limits: SpoolLimits) -> Self {
        Self {
            limits,
            io: IoMode::Blocking,
            metrics: Metrics::detached(),
            hooks: None,
        }
    }

    #[must_use]
    pub fn with_io(mut self, io: IoMode) -> Self {
        self.io = io;
        self
    }

    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn SpoolHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }
}

/// What opening a spool found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Segment files read.
    pub segments: usize,
    /// Whole records kept (before the bounds were applied).
    pub records: usize,
    /// Complete deltas held now.
    pub groups: usize,
    /// Segments that ended in damage or could not be read.
    pub damaged_segments: usize,
    /// Entries lost: damaged, unfinished, or beyond the bounds.
    pub lost_entries: u64,
    /// What the hub is owed, from this run and from the one before.
    pub gap: Option<SpoolGap>,
}

/// What the spool holds and has done, for tests and metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpoolStats {
    pub records: u64,
    pub entries: u64,
    pub groups: u64,
    pub disk_bytes: u64,
    pub lost_entries_total: u64,
    pub damaged_total: u64,
    /// The most bytes the reader has held for one record.
    pub max_replay_buffer: u64,
}

/// What an append did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    Stored,
    /// The message could not be kept (it belongs to a delta that was dropped, or it does not fit even alone). The loss
    /// is reported as a gap.
    Discarded,
}

/// The spool. Cheap to clone; all clones are the same spool.
#[derive(Clone)]
pub struct Spool {
    core: Arc<Core>,
    io: IoMode,
    /// One file operation that can block for long at a time.
    busy: Arc<Semaphore>,
    /// The agent's deny list (T11): what is stored and what is replayed never carries a denied file's bytes.
    deny: DenyList,
}

impl fmt::Debug for Spool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spool")
            .field("stats", &self.core.stats())
            .finish_non_exhaustive()
    }
}

impl Spool {
    /// Open the spool in `volume`: read what a previous run left, drop what is damaged or over the bounds, and make the
    /// sequence counter start above every number ever used. Blocking, and reads the whole spool once: call it before
    /// the runtime is busy, or inside `spawn_blocking`.
    pub fn open(
        volume: SpoolVolume,
        options: SpoolOptions,
        clock: Arc<dyn Clock>,
    ) -> Result<(Self, Recovery), SpoolError> {
        let io = options.io;
        let (core, recovery) = Core::open(volume, options, clock)?;
        Ok((
            Self {
                core,
                io,
                busy: Arc::new(Semaphore::new(1)),
                deny: DenyList::default(),
            },
            recovery,
        ))
    }

    /// Follow `deny`, the agent's one deny list, instead of the built-in globs alone. The returned spool is the same spool
    /// (same files, same counter); only the list it consults differs.
    #[must_use]
    pub fn with_deny(mut self, deny: DenyList) -> Self {
        self.deny = deny;
        self
    }

    /// The counter every message the agent sends takes its number from, hub-initiated answers included.
    pub fn seq(&self) -> Arc<SeqCounter> {
        self.core.seq()
    }

    pub fn stats(&self) -> SpoolStats {
        self.core.stats()
    }

    /// What the hub is still owed for dropped versions.
    pub fn gap(&self) -> Option<SpoolGap> {
        self.core.gap()
    }

    /// The connection's pump has handed every complete delta the spool holds to the connection (T10, A21). False while
    /// no pump runs, from the moment a new connection's pump starts until it has caught up, and whenever a delta has been
    /// appended since. Whatever is queued on the connection after this returns true comes after those deltas.
    pub fn drained(&self) -> bool {
        self.core.drained()
    }

    /// Wakes when [`Spool::drained`] may have changed.
    pub fn subscribe_drain(&self) -> tokio::sync::watch::Receiver<u64> {
        self.core.subscribe_drain()
    }

    /// The sink the scanner delivers its own deltas to.
    pub fn sink(&self) -> Arc<SpoolSink> {
        Arc::new(SpoolSink { spool: self.clone() })
    }

    /// Run `work` against the core where [`IoMode`] says, one at a time, and give up after [`IO_TIMEOUT`].
    async fn run<R: Send + 'static>(
        &self,
        work: impl FnOnce(&Core) -> R + Send + 'static,
    ) -> Result<R, SpoolError> {
        let core = Arc::clone(&self.core);
        match self.io {
            IoMode::Inline => Ok(work(&core)),
            IoMode::Blocking => {
                // Only one blocking task at a time. A wedged volume then holds one thread, not the whole pool, and the
                // caller learns at once that the spool is busy instead of queueing another.
                let permit = timeout(IO_TIMEOUT, Arc::clone(&self.busy).acquire_owned())
                    .await
                    .map_err(|_| SpoolError::TimedOut)?
                    .map_err(|_| SpoolError::Interrupted)?;
                let task = tokio::task::spawn_blocking(move || {
                    let out = work(&core);
                    drop(permit);
                    out
                });
                timeout(IO_TIMEOUT, task)
                    .await
                    .map_err(|_| SpoolError::TimedOut)?
                    .map_err(|_| SpoolError::Interrupted)
            }
        }
    }

    /// Store one message of a delta. Returns once it is safely written; the last message of a delta only after it has
    /// been synced.
    pub async fn append(&self, mut delta: ScanDelta) -> Result<Appended, SpoolError> {
        // The last guard (D79): whatever built this message, no denied file's bytes are written to the volume.
        let scrubbed = self.deny.scrub(&mut delta);
        if scrubbed > 0 {
            debug!(
                scrubbed,
                "denied files had bytes in a delta being spooled; stripped"
            );
        }
        self.run(move |core| core.append(delta)).await?
    }

    /// The hub acknowledged `seq`. Does no I/O; the files are cleaned up by housekeeping.
    pub fn ack(&self, seq: u64) {
        self.core.ack(seq);
    }

    /// One housekeeping pass.
    pub async fn maintain(&self) {
        if let Err(error) = self.run(Core::maintain).await {
            warn!(%error, "spool housekeeping did not finish");
        }
    }

    /// Housekeeping for as long as this future is polled: when an acknowledgement released something, and every
    /// [`MAINTENANCE_PERIOD`] otherwise.
    pub async fn run_maintenance(&self) -> Infallible {
        let mut ticker = interval(MAINTENANCE_PERIOD);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                () = self.core.maintenance_wanted().notified() => {}
            }
            self.maintain().await;
        }
    }

    /// Start sending the spool to a new connection, from the oldest delta still held.
    pub fn attach(&self, outbox: Outbox) -> Pump {
        Pump {
            spool: self.clone(),
            outbox,
            cursor: 0,
            window: VecDeque::new(),
            window_bytes: 0,
        }
    }
}

/// Where the scanner's own deltas go (D74): into the spool.
#[derive(Debug)]
pub struct SpoolSink {
    spool: Spool,
}

#[async_trait]
impl DeltaSink for SpoolSink {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError> {
        match self.spool.append(delta).await {
            Ok(Appended::Stored | Appended::Discarded) => Ok(()),
            Err(SpoolError::TooLarge) => Err(SinkError::TooLarge),
            Err(error) => {
                warn!(%error, "a delta could not be spooled; the hub will ask for it by root");
                Err(SinkError::Storage)
            }
        }
    }
}

/// Why a pump stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpEnd {
    /// The connection is gone.
    ConnectionClosed,
    /// The spool did not answer; the connection is dropped so that the next one starts clean.
    StorageStalled,
}

/// Sends the spool to one connection: every delta still held, oldest first and each from its first part, then each new
/// one as it becomes durable. It never has more than a window of messages unacknowledged.
#[derive(Debug)]
pub struct Pump {
    spool: Spool,
    outbox: Outbox,
    /// The sequence number of the last message sent on this connection.
    cursor: u64,
    /// Sent and not yet acknowledged: sequence number and size.
    window: VecDeque<(u64, u64)>,
    window_bytes: u64,
}

/// Tells the spool its pump has ended, however the future ends (it is dropped when the connection's handler is).
struct PumpGuard {
    core: Arc<Core>,
    epoch: u64,
}

impl Drop for PumpGuard {
    fn drop(&mut self) {
        self.core.pump_stopped(self.epoch);
    }
}

impl Pump {
    pub async fn run(mut self) -> PumpEnd {
        let epoch = self.spool.core.pump_started();
        let _guard = PumpGuard {
            core: Arc::clone(&self.spool.core),
            epoch,
        };
        let mut changes = self.spool.core.subscribe();
        loop {
            // Anything that changes after this point wakes `changed()` below, so nothing can slip between the look and
            // the wait.
            changes.borrow_and_update();
            self.spool.core.retire_settled(&mut self.window);
            self.window_bytes = self.window.iter().map(|(_, bytes)| *bytes).sum();

            if self.window.len() >= IN_FLIGHT_MESSAGES || self.window_bytes >= IN_FLIGHT_BYTES {
                if changes.changed().await.is_err() {
                    return PumpEnd::ConnectionClosed;
                }
                continue;
            }
            let cursor = self.cursor;
            let next = match self.spool.run(move |core| core.read_next(cursor, epoch)).await {
                Ok(Ok(next)) => next,
                Ok(Err(error)) | Err(error) => {
                    warn!(%error, "the spool could not be read for the hub");
                    return PumpEnd::StorageStalled;
                }
            };
            let Some((seq, outgoing)) = next else {
                if changes.changed().await.is_err() {
                    return PumpEnd::ConnectionClosed;
                }
                continue;
            };
            let bytes = outgoing.payload_bytes;
            let mut delta = outgoing.delta;
            // A record written before the hub added a deny glob may hold bytes of a file that is denied now (D79).
            let scrubbed = self.spool.deny.scrub(&mut delta);
            if scrubbed > 0 {
                debug!(
                    seq,
                    scrubbed, "a spooled record held bytes of denied files; stripped before sending"
                );
            }
            match self.outbox.send(FromAgent::Delta(delta)).await {
                Ok(()) => {
                    self.cursor = seq;
                    self.window.push_back((seq, bytes));
                    debug!(seq, "a spooled delta was sent");
                }
                Err(OutboxError::TooLarge) => {
                    // It can never be sent: treat it as damaged so that the pump does not stop here for ever.
                    warn!(
                        seq,
                        "a spooled message is too large for the outbox; dropped as lost"
                    );
                    self.spool.core.discard_unsendable(seq);
                }
                Err(OutboxError::Closed | OutboxError::Full) => return PumpEnd::ConnectionClosed,
            }
        }
    }
}
