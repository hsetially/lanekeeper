//! The spool's engine: the index in memory, the segment files on disk, and the order in which they change (D74, P15).
//!
//! Everything here is blocking and synchronous; [`super::Spool`] decides where it runs. Two locks, always taken in this
//! order: `files` (everything that writes or deletes a file; one append at a time) and then `index` (short, memory
//! only; the only one an acknowledgement takes).
//!
//! # An append, step by step
//!
//! 1. the index says whether the record belongs to a delta that is being kept (a part that does not follow its
//!    predecessor ends that delta and is discarded);
//! 2. versions identical to the one before them that is still held are removed ([`Index::dedupe`]);
//! 3. the record is encoded once, with its checksum;
//! 4. room is made: the oldest complete deltas are dropped, whole, until the frame fits both bounds; if there are none
//!    and the delta being written is itself too big, it is dropped and its remaining parts are discarded as they come;
//! 5. the frame is written to the active segment (a new one when the active one is full);
//! 6. on the last part of a delta the segment is synced, and only then does the delta become sendable.
//!
//! A write or sync that fails ends the delta (as lost) and the segment (nothing is appended to a segment that may hold
//! half a frame). The next append starts a new segment.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use cap_std::fs::File;
use domain::{ScanDelta, SpoolGap};
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use super::index::{Admit, Index, Limits, Next};
use super::record::{FRAME_OVERHEAD, Frame, META_LEN, Meta};
use super::recover::{self, Recovered};
use super::segment::{self, Active, SEGMENT_HEADER_LEN};
use super::state::{self, Loaded, State};
use super::{Appended, Recovery, SpoolError, SpoolHooks, SpoolOptions, SpoolStats, SpoolVolume};
use crate::clock::Clock;
use crate::ops::Metrics;
use crate::transport::wire;
use crate::tree::delta::SeqCounter;

/// How far ahead of the counter the reserved sequence number is kept, and when it is raised.
pub const SEQ_WINDOW: u64 = 4096;
pub const SEQ_HEADROOM: u64 = 2048;
/// The least time between two writes of the state file that only move the floor.
const FLOOR_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);

/// `drained_mark` before the pump of a connection has caught up. No sequence number is this large.
const NOT_DRAINED: u64 = u64::MAX;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn io_error(e: &io::Error) -> SpoolError {
    SpoolError::Io(e.kind())
}

#[derive(Debug)]
struct Files {
    active: Option<Active>,
    next_segment: u64,
    reserved_seq: u64,
    /// The floor in the state file, and when the file was last written for the floor alone.
    stored_floor: u64,
    floor_written_at: Option<tokio::time::Instant>,
    /// A simulated crash happened (tests): the spool refuses everything, as a killed process would.
    dead: bool,
}

#[derive(Debug, Default)]
struct Counters {
    damaged: AtomicU64,
    max_replay_buffer: AtomicU64,
    reported_lost: AtomicU64,
}

/// A record read back from the spool, ready to send.
#[derive(Debug)]
pub struct Outgoing {
    pub delta: ScanDelta,
    pub payload_bytes: u64,
}

pub struct Core {
    volume: SpoolVolume,
    clock: Arc<dyn Clock>,
    index: Mutex<Index>,
    files: Mutex<Files>,
    reader: Mutex<Option<(u64, Arc<File>)>>,
    seq: Arc<SeqCounter>,
    changed: watch::Sender<u64>,
    /// Raised when [`Core::drained`] may have changed (T10).
    drain: watch::Sender<u64>,
    /// The pump that is sending to the current connection (`0`: none), and the last epoch handed out.
    pump_epoch: AtomicU64,
    last_epoch: AtomicU64,
    /// `ready_through` at the moment the current pump found nothing more to send, or [`NOT_DRAINED`].
    drained_mark: AtomicU64,
    wake_maintenance: Notify,
    hooks: Option<Arc<dyn SpoolHooks>>,
    metrics: Arc<Metrics>,
    counters: Counters,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core").finish_non_exhaustive()
    }
}

impl Core {
    // -------------------------------------------------------------------------------------------- open

    /// Read every segment, rebuild the index, drop what the bounds no longer allow and what is damaged, and make the
    /// sequence counter start above everything ever used. Blocking, and reads the whole spool once.
    pub fn open(
        volume: SpoolVolume,
        options: SpoolOptions,
        clock: Arc<dyn Clock>,
    ) -> Result<(Arc<Self>, Recovery), SpoolError> {
        let dir = volume.dir();
        let now = clock.now().unix_millis();
        let ids = recover::list_segments(dir).map_err(|e| io_error(&e))?;
        let loaded = state::load(dir).map_err(|e| io_error(&e))?;
        let floor = match loaded {
            Loaded::Valid(state) => state.floor_seq,
            Loaded::Missing | Loaded::Invalid => 0,
        };
        let Recovered {
            mut index,
            mut recovery,
            max_seq,
            repairs,
        } = recover::read_all(
            dir,
            &ids,
            options.limits.as_index_limits(),
            floor,
            now,
            &options.metrics,
        );
        index.finish_recovery();
        index.enforce_limits();
        index.reap_dead_segments();
        if let Loaded::Valid(State { gap: Some(gap), .. }) = loaded {
            index.restore_gap(gap);
        }

        let first_seq = match loaded {
            Loaded::Valid(state) => state.reserved_seq.max(max_seq + 1),
            Loaded::Missing if ids.is_empty() => 1,
            // The state is gone or damaged: skip a whole window beyond anything the segments show.
            Loaded::Missing | Loaded::Invalid => max_seq + 1 + SEQ_WINDOW,
        }
        .max(1);
        index.skip(max_seq);
        let reserved_seq = first_seq.saturating_add(SEQ_WINDOW);
        let floor_seq = index.floor().max(floor);
        // The loss is written down before anything is repaired or deleted, so a crash in between reports it twice at
        // worst and never not at all.
        state::store(
            dir,
            &State {
                reserved_seq,
                floor_seq,
                gap: index.gap(),
            },
        )
        .map_err(|e| io_error(&e))?;
        index.take_gap_dirty();
        for id in index.take_freed() {
            let _ = dir.remove_file(segment::name(id));
        }
        for (id, keep) in repairs {
            recover::repair(dir, id, keep);
        }

        recovery.groups = usize::try_from(index.stats().groups).unwrap_or(usize::MAX);
        recovery.lost_entries = index.lost_total();
        recovery.gap = index.gap();
        let next_segment = ids.last().map_or(1, |last| last + 1);
        let (changed, _) = watch::channel(0);
        let core = Arc::new(Self {
            volume,
            clock,
            index: Mutex::new(index),
            files: Mutex::new(Files {
                active: None,
                next_segment,
                reserved_seq,
                stored_floor: floor_seq,
                floor_written_at: None,
                dead: false,
            }),
            reader: Mutex::new(None),
            seq: Arc::new(SeqCounter::new(first_seq)),
            changed,
            drain: watch::channel(0).0,
            pump_epoch: AtomicU64::new(0),
            last_epoch: AtomicU64::new(0),
            drained_mark: AtomicU64::new(NOT_DRAINED),
            wake_maintenance: Notify::new(),
            hooks: options.hooks,
            metrics: options.metrics,
            counters: Counters {
                damaged: AtomicU64::new(recovery.damaged_segments as u64),
                ..Counters::default()
            },
        });
        core.publish_metrics();
        info!(
            segments = recovery.segments,
            records = recovery.records,
            deltas = recovery.groups,
            damaged_segments = recovery.damaged_segments,
            "the spool is open"
        );
        Ok((core, recovery))
    }

    pub fn seq(&self) -> Arc<SeqCounter> {
        Arc::clone(&self.seq)
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn maintenance_wanted(&self) -> &Notify {
        &self.wake_maintenance
    }

    fn bump(&self) {
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn stats(&self) -> SpoolStats {
        let (stats, lost) = {
            let index = lock(&self.index);
            (index.stats(), index.lost_total())
        };
        SpoolStats {
            records: stats.records,
            entries: stats.entries,
            groups: stats.groups,
            disk_bytes: stats.disk_bytes,
            lost_entries_total: lost,
            damaged_total: self.counters.damaged.load(Ordering::Relaxed),
            max_replay_buffer: self.counters.max_replay_buffer.load(Ordering::Relaxed),
        }
    }

    pub fn gap(&self) -> Option<SpoolGap> {
        lock(&self.index).gap()
    }

    fn publish_metrics(&self) {
        let stats = self.stats();
        self.metrics.spool_held(stats.entries, stats.disk_bytes);
        let reported = self
            .counters
            .reported_lost
            .swap(stats.lost_entries_total, Ordering::Relaxed);
        if stats.lost_entries_total > reported {
            self.metrics.spool_lost(stats.lost_entries_total - reported);
        }
    }

    // -------------------------------------------------------------------------------------------- append

    /// Store one message of a delta. See the module documentation.
    pub fn append(&self, mut delta: ScanDelta) -> Result<Appended, SpoolError> {
        let mut files = lock(&self.files);
        if files.dead {
            return Err(SpoolError::Crashed);
        }
        self.collect(&mut files);
        let now = self.clock.now().unix_millis();

        // 1. Is this message part of a delta that is being kept?
        let raw = meta_of(&delta, now);
        let admit = lock(&self.index)
            .begin(&raw)
            .map_err(|e| SpoolError::OutOfOrder {
                got: e.got,
                last: e.last,
            })?;
        if admit == Admit::Discard {
            lock(&self.index).reject(&raw);
            return Ok(self.settled(&mut files, Appended::Discarded));
        }

        // 2. and 3. Without the repeats, encoded once.
        let claims = lock(&self.index).dedupe(&mut delta);
        let meta = meta_of(&delta, now);
        let mut buf = Frame::begin(0);
        wire::encode_delta_into(delta, &mut buf);
        let Ok(frame) = Frame::seal(buf, &meta) else {
            lock(&self.index).reject(&meta);
            self.settled(&mut files, Appended::Discarded);
            return Err(SpoolError::TooLarge);
        };
        let frame_len = frame.len() as u64;

        // 4. Room.
        loop {
            let rolls = Self::rolls(&files, &self.limits(), frame_len);
            let need = frame_len + if rolls { SEGMENT_HEADER_LEN } else { 0 };
            let mut index = lock(&self.index);
            if index.fits(need, u64::from(meta.entries)) {
                break;
            }
            if index.drop_oldest() {
                drop(index);
                self.collect(&mut files);
                continue;
            }
            if index.has_open_records() {
                index.poison_open(&meta);
            }
            index.reject(&meta);
            drop(index);
            self.collect(&mut files);
            return Ok(self.settled(&mut files, Appended::Discarded));
        }

        // 5. Write.
        if let Err(error) = self.write(&mut files, &frame, &meta, &claims) {
            if !files.dead {
                self.fail(&mut files);
            }
            return Err(error);
        }

        // 6. Durable, then sendable.
        if !meta.more {
            let synced = files.active.as_ref().map_or(Ok(()), Active::sync);
            if let Err(e) = synced {
                self.fail(&mut files);
                return Err(io_error(&e));
            }
            if let Some(hooks) = &self.hooks {
                hooks.synced();
            }
            lock(&self.index).finish_group();
        }
        Ok(self.settled(&mut files, Appended::Stored))
    }

    fn limits(&self) -> Limits {
        lock(&self.index).limits()
    }

    /// Would this frame start a new segment?
    fn rolls(files: &Files, limits: &Limits, frame_len: u64) -> bool {
        match &files.active {
            None => true,
            Some(active) => active.len > SEGMENT_HEADER_LEN && active.len + frame_len > limits.segment_bytes,
        }
    }

    fn write(
        &self,
        files: &mut Files,
        frame: &Frame,
        meta: &Meta,
        claims: &[(super::index::Key, super::index::Version)],
    ) -> Result<(), SpoolError> {
        let dir = self.volume.dir();
        let frame_len = frame.len() as u64;
        if Self::rolls(files, &self.limits(), frame_len) {
            if let Some(old) = files.active.take() {
                // The delta being written may have parts in it.
                old.sync().map_err(|e| io_error(&e))?;
                if let Some(hooks) = &self.hooks {
                    hooks.synced();
                }
                let id = old.id;
                let mut index = lock(&self.index);
                index.set_active(None);
                if index.segment_is_dead(id) {
                    index.free_segment(id);
                }
            }
            let id = files.next_segment;
            files.next_segment += 1;
            let created = Active::create(dir, id).map_err(|e| io_error(&e))?;
            segment::sync_dir(dir).map_err(|e| io_error(&e))?;
            let mut index = lock(&self.index);
            index.register_segment(id, SEGMENT_HEADER_LEN);
            index.set_active(Some(id));
            files.active = Some(created);
        }
        let Some(active) = files.active.as_mut() else {
            return Err(SpoolError::Io(io::ErrorKind::Other));
        };
        if self.hooks.as_ref().is_some_and(|h| h.fail_write()) {
            return Err(SpoolError::Io(io::ErrorKind::Other));
        }
        if let Some(keep) = self.hooks.as_ref().and_then(|h| h.torn_write(frame.len())) {
            let _ = active.append_torn(frame.as_bytes(), keep);
            files.dead = true;
            return Err(SpoolError::Crashed);
        }
        let id = active.id;
        let offset = active.append(frame.as_bytes()).map_err(|e| io_error(&e))?;
        let mut index = lock(&self.index);
        index.grow_segment(id, frame_len);
        index.commit(meta, id, offset, frame_len, claims);
        Ok(())
    }

    /// A write or a sync failed: the delta is lost, and the segment may hold half a frame.
    fn fail(&self, files: &mut Files) {
        let mut index = lock(&self.index);
        index.abort_open();
        if let Some(active) = files.active.take() {
            index.set_active(None);
            if index.segment_is_dead(active.id) {
                index.free_segment(active.id);
            }
        }
        drop(index);
        self.collect(files);
        self.after_change(files);
        warn!("a spool write failed; the delta is lost and a new segment will be started");
    }

    fn settled(&self, files: &mut Files, outcome: Appended) -> Appended {
        self.after_change(files);
        outcome
    }

    /// After any change: write down what must survive a crash, tell the pumps, update the gauges.
    fn after_change(&self, files: &mut Files) {
        self.persist(files);
        self.publish_metrics();
        self.bump();
    }

    fn persist(&self, files: &mut Files) {
        let (gap, dirty, floor) = {
            let mut index = lock(&self.index);
            let dirty = index.take_gap_dirty();
            (index.gap(), dirty, index.floor().max(files.stored_floor))
        };
        let now = self.clock.instant();
        let next = self.seq.peek();
        let raise = next.saturating_add(SEQ_HEADROOM) > files.reserved_seq;
        // A floor that only moved because of acknowledgements is written at most once a second: it saves duplicates
        // after a restart, and a sync for every acknowledgement would cost more than it saves.
        let floor_due = floor > files.stored_floor
            && files
                .floor_written_at
                .is_none_or(|at| now.saturating_duration_since(at) >= FLOOR_PERIOD);
        if !dirty && !raise && !floor_due {
            return;
        }
        let reserved_seq = if raise {
            next.saturating_add(SEQ_WINDOW)
        } else {
            files.reserved_seq
        };
        match state::store(
            self.volume.dir(),
            &State {
                reserved_seq,
                floor_seq: floor,
                gap,
            },
        ) {
            Ok(()) => {
                files.reserved_seq = reserved_seq;
                files.stored_floor = floor;
                files.floor_written_at = Some(now);
            }
            Err(e) => {
                // Not fatal, but the guarantee behind it is weaker until a later write succeeds.
                warn!(kind = ?e.kind(), "the spool's state file could not be written");
                if dirty {
                    lock(&self.index).restore_dirty();
                }
            }
        }
    }

    // -------------------------------------------------------------------------------------------- housekeeping

    /// Delete the files nothing is owed from any more, and close the active segment when it is one of them.
    fn collect(&self, files: &mut Files) {
        {
            let mut index = lock(&self.index);
            if let Some(active) = &files.active {
                if index.segment_is_dead(active.id) {
                    let id = active.id;
                    files.active = None;
                    index.free_segment(id);
                }
            }
        }
        let freed = lock(&self.index).take_freed();
        for id in freed {
            match self.volume.dir().remove_file(segment::name(id)) {
                Ok(()) => debug!(segment = id, "a spool segment was deleted"),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => warn!(segment = id, kind = ?e.kind(), "a spool segment could not be deleted"),
            }
            let mut reader = lock(&self.reader);
            if reader.as_ref().is_some_and(|(cached, _)| *cached == id) {
                *reader = None;
            }
        }
    }

    /// One housekeeping pass: collect, keep the sequence reservation ahead of the counter, write down a changed gap.
    pub fn maintain(&self) {
        let mut files = lock(&self.files);
        if files.dead {
            return;
        }
        self.collect(&mut files);
        self.persist(&mut files);
        self.publish_metrics();
    }

    // -------------------------------------------------------------------------------------------- acknowledgements

    /// The hub acknowledged `seq`. No I/O: a released segment is deleted by the next [`Core::maintain`].
    pub fn ack(&self, seq: u64) {
        let (released, gap_changed) = {
            let mut index = lock(&self.index);
            let released = index.ack(seq);
            (released, index.gap_is_dirty())
        };
        if released || gap_changed {
            self.wake_maintenance.notify_one();
        }
        if released {
            self.publish_metrics();
        }
        self.bump();
    }

    /// A stored message that can never be sent (it is larger than the outbox can take): drop its delta, as lost.
    pub fn discard_unsendable(&self, seq: u64) {
        lock(&self.index).note_damaged(seq);
        self.wake_maintenance.notify_one();
        self.publish_metrics();
        self.bump();
    }

    /// A connection's pump starts: the loss is owed to it afresh, and the spool is not drained until it has caught up.
    /// Returns the pump's epoch, which it passes to [`Core::read_next`] and [`Core::pump_stopped`].
    pub fn pump_started(&self) -> u64 {
        lock(&self.index).reset_carrier();
        let epoch = self.last_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.pump_epoch.store(epoch, Ordering::SeqCst);
        self.drained_mark.store(NOT_DRAINED, Ordering::SeqCst);
        self.drain.send_modify(|n| *n = n.wrapping_add(1));
        epoch
    }

    /// The pump of `epoch` has ended. A later pump's state is not touched.
    pub fn pump_stopped(&self, epoch: u64) {
        if self
            .pump_epoch
            .compare_exchange(epoch, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.drained_mark.store(NOT_DRAINED, Ordering::SeqCst);
            self.drain.send_modify(|n| *n = n.wrapping_add(1));
        }
    }

    /// The pump has handed the connection every complete delta the spool holds, and none has been added since.
    pub fn drained(&self) -> bool {
        // The mark is only ever written under the index lock at the moment the pump finds nothing; reading `ready_through`
        // under the same lock means a delta appended after that moment makes the two differ.
        let index = lock(&self.index);
        self.pump_epoch.load(Ordering::SeqCst) != 0
            && self.drained_mark.load(Ordering::SeqCst) == index.ready_through()
    }

    pub fn subscribe_drain(&self) -> watch::Receiver<u64> {
        self.drain.subscribe()
    }

    /// Drop from `window` every message the hub has acknowledged (or that is gone).
    pub fn retire_settled(&self, window: &mut std::collections::VecDeque<(u64, u64)>) {
        let index = lock(&self.index);
        window.retain(|(seq, _)| !index.is_settled(*seq));
    }

    // -------------------------------------------------------------------------------------------- reading

    fn reader_for(&self, segment_id: u64) -> io::Result<Arc<File>> {
        let mut reader = lock(&self.reader);
        if let Some((cached, file)) = reader.as_ref() {
            if *cached == segment_id {
                return Ok(Arc::clone(file));
            }
        }
        let file = Arc::new(self.volume.dir().open(segment::name(segment_id))?);
        *reader = Some((segment_id, Arc::clone(&file)));
        Ok(file)
    }

    /// The next message to send after `cursor`, read back and checked. A record that cannot be read back whole is
    /// dropped with its delta, as lost, and the next one is tried.
    pub fn read_next(&self, cursor: u64, epoch: u64) -> Result<Option<(u64, Outgoing)>, SpoolError> {
        loop {
            let next = {
                let mut index = lock(&self.index);
                let next = index.next_after(cursor);
                if next.is_none() {
                    // Nothing more to send to this connection, as of this moment: written under the lock that appends
                    // take, so a later append cannot be missed.
                    let through = index.ready_through();
                    if self.pump_epoch.load(Ordering::SeqCst) == epoch
                        && self.drained_mark.swap(through, Ordering::SeqCst) != through
                    {
                        self.drain.send_modify(|n| *n = n.wrapping_add(1));
                    }
                }
                next
            };
            let Some(next) = next else {
                return Ok(None);
            };
            let seq = next.meta.seq;
            match self.load(&next) {
                Ok(outgoing) => return Ok(Some((seq, outgoing))),
                Err(Unreadable::Transient(kind)) => return Err(SpoolError::Io(kind)),
                Err(Unreadable::Damaged) if !lock(&self.index).holds(seq) => {
                    // Released or dropped while it was being read (its segment may be gone): nothing is wrong with it.
                    debug!(seq, "a spooled record was released while it was being read");
                }
                Err(Unreadable::Damaged) => {
                    self.counters.damaged.fetch_add(1, Ordering::Relaxed);
                    self.metrics.spool_damaged();
                    warn!(
                        seq,
                        "a spooled record cannot be read back whole; its delta is dropped as lost"
                    );
                    lock(&self.index).note_damaged(seq);
                    self.wake_maintenance.notify_one();
                    self.publish_metrics();
                    self.bump();
                }
            }
        }
    }

    fn load(&self, next: &Next) -> Result<Outgoing, Unreadable> {
        let file = self.reader_for(next.seg).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => Unreadable::Damaged,
            kind => Unreadable::Transient(kind),
        })?;
        let (meta, payload) =
            segment::read_record(&file, next.offset, next.frame_len).map_err(|e| match e {
                segment::ReadFailure::Io(kind) => Unreadable::Transient(kind),
                segment::ReadFailure::Damaged(_) => Unreadable::Damaged,
            })?;
        if meta != next.meta {
            return Err(Unreadable::Damaged);
        }
        let payload_bytes = payload.len() as u64;
        self.counters
            .max_replay_buffer
            .fetch_max(payload_bytes, Ordering::Relaxed);
        let body = Bytes::from(payload).slice(META_LEN..);
        let mut delta = wire::decode_delta(body).map_err(|_| Unreadable::Damaged)?;
        if delta.seq != meta.seq || delta.part != meta.part || delta.more != meta.more {
            return Err(Unreadable::Damaged);
        }
        delta.gap = next.gap;
        Ok(Outgoing { delta, payload_bytes })
    }
}

enum Unreadable {
    /// The read failed; the record may be fine.
    Transient(io::ErrorKind),
    /// The record is not what was written.
    Damaged,
}

/// What the spool needs to know about a message without reading its body again.
fn meta_of(delta: &ScanDelta, now_ms: i64) -> Meta {
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for entry in &delta.entries {
        let at = entry.observed_at.unix_millis();
        first = first.min(at);
        last = last.max(at);
    }
    if first > last {
        first = now_ms;
        last = now_ms;
    }
    let entries = delta.entries.len() + delta.removed.len() + delta.skipped.len();
    Meta {
        seq: delta.seq,
        part: delta.part,
        more: delta.more,
        entries: u32::try_from(entries).unwrap_or(u32::MAX),
        first_ms: first,
        last_ms: last,
    }
}

const _: () = assert!(FRAME_OVERHEAD == 44);
