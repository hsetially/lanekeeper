//! What the spool holds, in memory: every live record, how records group into logical deltas, which segment files carry
//! them, what has been lost, and what the dedup rule needs (D74, A7).
//!
//! Nothing here touches a file. [`super::core`] drives the I/O and calls this to decide; the unit tests below drive it
//! alone.
//!
//! # Groups
//!
//! The messages of one logical delta (`part` 0, 1, ..., last with `more = false`) are a **group**. The hub applies a
//! delta only when it has all of it, and keeps the parts per connection, so a group is the unit of everything:
//!
//! - it is sent only once complete and durable (`ready_through`), and always from its first part;
//! - it is acknowledged part by part, but released only when every part is;
//! - it is dropped whole when the spool is full, so what remains always starts at a part 0 and never at the middle of
//!   a delta;
//! - a group that was still open when the process died is discarded when the spool is opened again.
//!
//! # Loss
//!
//! Whatever is dropped (to make room, because a record was damaged, or because a group never completed) is added to one
//! pending [`SpoolGap`]: the earliest and latest time seen in what is gone (by `min` and `max`, so `from <= to` always
//! holds, which the hub requires) and how many entries. It is stamped on the first message of a connection and stays
//! pending until the hub acknowledges a message that carried it. Loss recorded after the stamp has a newer `epoch`, so
//! acknowledging the older message does not clear it.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

use domain::{ContentHash, ScanDelta, SpoolGap, Timestamp};
use sha2::{Digest, Sha256};

use super::record::Meta;

/// A path, for the dedup table: the first 16 bytes of its SHA-256. A collision would drop one version that is not a
/// duplicate; at 2^-64 per pair that is not a risk worth the memory of keeping the paths.
pub type Key = [u8; 16];

/// What a path was last seen as, among the versions still held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    File { hash: ContentHash, denied: bool },
    Removed,
}

/// The bounds. `segment_bytes` is the size at which the active segment is closed and a new one started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: u64,
    pub segment_bytes: u64,
}

#[derive(Debug)]
struct Rec {
    seg: u64,
    offset: u64,
    frame_len: u64,
    meta: Meta,
    acked: bool,
    /// Paths whose latest version this record claimed when it was stored.
    keys: Box<[Key]>,
}

#[derive(Debug, Clone, Copy)]
struct Group {
    last: u64,
    records: u32,
    acked: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// No delta is being written.
    Closed,
    /// Parts of a delta are being written; `done` once the part with `more = false` is in.
    Open { first: u64, next_part: u32, done: bool },
    /// A delta that cannot be kept: its remaining parts are discarded as they arrive.
    Poisoned { next_part: u32 },
}

#[derive(Debug, Clone, Copy)]
struct Seg {
    len: u64,
    live: u32,
}

#[derive(Debug, Clone, Copy)]
struct GapAcc {
    from: i64,
    to: i64,
    lost: u64,
    epoch: u64,
}

impl GapAcc {
    fn as_gap(self) -> SpoolGap {
        SpoolGap {
            from: Timestamp::from_unix_millis(self.from),
            to: Timestamp::from_unix_millis(self.to),
            lost_entries: self.lost,
        }
    }
}

/// A record the pump should send next.
#[derive(Debug, Clone, Copy)]
pub struct Next {
    pub seg: u64,
    pub offset: u64,
    pub frame_len: u64,
    pub meta: Meta,
    /// The loss to stamp on this message.
    pub gap: Option<SpoolGap>,
}

/// What the spool holds right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    pub records: u64,
    pub entries: u64,
    pub groups: u64,
    pub disk_bytes: u64,
}

/// A sequence number that did not increase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("sequence number {got} does not follow {last}")]
pub struct OutOfOrder {
    pub got: u64,
    pub last: u64,
}

/// Whether a record that arrives is to be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Store,
    Discard,
}

#[derive(Debug)]
pub struct Index {
    limits: Limits,
    recs: BTreeMap<u64, Rec>,
    groups: BTreeMap<u64, Group>,
    tail: Tail,
    segs: BTreeMap<u64, Seg>,
    active: Option<u64>,
    entries: u64,
    disk: u64,
    ready_through: u64,
    last_seq: u64,
    latest: HashMap<Key, (u64, Version)>,
    gap: Option<GapAcc>,
    gap_dirty: bool,
    epoch: u64,
    carrier: Option<(u64, u64)>,
    freed: Vec<u64>,
    lost_total: u64,
}

impl Index {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            recs: BTreeMap::new(),
            groups: BTreeMap::new(),
            tail: Tail::Closed,
            segs: BTreeMap::new(),
            active: None,
            entries: 0,
            disk: 0,
            ready_through: 0,
            last_seq: 0,
            latest: HashMap::new(),
            gap: None,
            gap_dirty: false,
            epoch: 0,
            carrier: None,
            freed: Vec::new(),
            lost_total: 0,
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn stats(&self) -> IndexStats {
        IndexStats {
            records: self.recs.len() as u64,
            entries: self.entries,
            groups: self.groups.len() as u64,
            disk_bytes: self.disk,
        }
    }

    /// Entries ever dropped or lost by this process.
    pub fn lost_total(&self) -> u64 {
        self.lost_total
    }

    /// The lowest sequence number that may still be live: everything below it is acknowledged, dropped or discarded.
    pub fn floor(&self) -> u64 {
        self.recs
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| self.last_seq.saturating_add(1))
    }

    /// A record that was read and not kept (it is settled): its number is used up all the same.
    pub fn skip(&mut self, seq: u64) {
        self.last_seq = self.last_seq.max(seq);
    }

    /// The highest sequence number the spool has seen, stored or not.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    // -------------------------------------------------------------------------------------------- segments

    /// A segment file exists with `len` bytes.
    pub fn register_segment(&mut self, id: u64, len: u64) {
        if let Some(old) = self.segs.insert(id, Seg { len, live: 0 }) {
            self.disk -= old.len;
        }
        self.disk += len;
    }

    /// The segment the writer is appending to. It is never reported as freed while it is the active one.
    pub fn set_active(&mut self, id: Option<u64>) {
        self.active = id;
    }

    pub fn grow_segment(&mut self, id: u64, by: u64) {
        if let Some(seg) = self.segs.get_mut(&id) {
            seg.len += by;
            self.disk += by;
        }
    }

    /// True when no live record is in segment `id` (or the spool does not know it).
    pub fn segment_is_dead(&self, id: u64) -> bool {
        self.segs.get(&id).is_none_or(|s| s.live == 0)
    }

    /// Stop counting segment `id` and ask for its file to be deleted. For the writer's own segment, once it has closed it.
    pub fn free_segment(&mut self, id: u64) {
        if let Some(seg) = self.segs.remove(&id) {
            self.disk -= seg.len;
            self.freed.push(id);
        }
        if self.active == Some(id) {
            self.active = None;
        }
    }

    /// Segments whose files can be deleted: every record in them was released.
    pub fn take_freed(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.freed)
    }

    /// Free every registered segment that holds no live record and is not the active one (used after recovery).
    pub fn reap_dead_segments(&mut self) {
        let dead: Vec<u64> = self
            .segs
            .iter()
            .filter(|(id, s)| s.live == 0 && Some(**id) != self.active)
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.free_segment(id);
        }
    }

    fn release(&mut self, seg: u64) {
        let Some(s) = self.segs.get_mut(&seg) else { return };
        s.live = s.live.saturating_sub(1);
        if s.live == 0 && self.active != Some(seg) {
            self.free_segment(seg);
        }
    }

    // -------------------------------------------------------------------------------------------- admission

    /// A record with `meta` is about to be stored. Checks the order, ends a delta that was left unfinished, and says
    /// whether the record belongs to a delta that is being kept.
    pub fn begin(&mut self, meta: &Meta) -> Result<Admit, OutOfOrder> {
        if meta.seq <= self.last_seq {
            return Err(OutOfOrder {
                got: meta.seq,
                last: self.last_seq,
            });
        }
        self.last_seq = meta.seq;
        if meta.part == 0 {
            self.abandon_open();
            self.tail = Tail::Closed;
            return Ok(Admit::Store);
        }
        match self.tail {
            Tail::Open {
                next_part,
                done: false,
                ..
            } if next_part == meta.part => Ok(Admit::Store),
            Tail::Poisoned { next_part } if next_part == meta.part => Ok(Admit::Discard),
            _ => {
                // A part that does not follow the one before it: the delta it belongs to is not whole.
                self.abandon_open();
                self.tail = Tail::Closed;
                Ok(Admit::Discard)
            }
        }
    }

    /// Is there room for a frame of `bytes` (a new segment's magic included) and `entries` more entries?
    pub fn fits(&self, bytes: u64, entries: u64) -> bool {
        self.disk + bytes <= self.limits.max_bytes && self.entries + entries <= self.limits.max_entries
    }

    /// Drop the oldest complete delta to make room. `false` when there is none.
    pub fn drop_oldest(&mut self) -> bool {
        let Some((&first, &group)) = self.groups.iter().next() else {
            return false;
        };
        self.groups.remove(&first);
        self.remove_range(first, group.last, true);
        true
    }

    /// True while a delta that is not complete has records stored.
    pub fn has_open_records(&self) -> bool {
        matches!(self.tail, Tail::Open { .. })
    }

    /// The delta being written cannot be kept: drop what it has stored and discard the rest of it. The record that
    /// caused this (`current`, not yet stored) must be [`rejected`](Index::reject) next.
    pub fn poison_open(&mut self, current: &Meta) {
        self.abandon_open();
        self.tail = Tail::Poisoned {
            next_part: current.part,
        };
    }

    /// A record that is not stored: it counts as lost, and the rest of its delta goes with it.
    pub fn reject(&mut self, meta: &Meta) {
        self.note_loss(meta.first_ms, meta.last_ms, u64::from(meta.entries));
        self.tail = if meta.more {
            Tail::Poisoned {
                next_part: meta.part + 1,
            }
        } else {
            Tail::Closed
        };
    }

    /// Store a record. `claims` are the paths whose latest version it now is (see [`Index::dedupe`]).
    pub fn commit(&mut self, meta: &Meta, seg: u64, offset: u64, frame_len: u64, claims: &[(Key, Version)]) {
        for (key, version) in claims {
            self.latest.insert(*key, (meta.seq, *version));
        }
        self.recs.insert(
            meta.seq,
            Rec {
                seg,
                offset,
                frame_len,
                meta: *meta,
                acked: false,
                keys: claims.iter().map(|(k, _)| *k).collect(),
            },
        );
        self.entries += u64::from(meta.entries);
        if let Some(s) = self.segs.get_mut(&seg) {
            s.live += 1;
        }
        self.tail = match self.tail {
            Tail::Open { first, .. } if meta.part > 0 => Tail::Open {
                first,
                next_part: meta.part + 1,
                done: !meta.more,
            },
            _ => Tail::Open {
                first: meta.seq,
                next_part: meta.part + 1,
                done: !meta.more,
            },
        };
    }

    /// The delta being written is complete and durable: from now on it can be sent.
    pub fn finish_group(&mut self) {
        let Tail::Open {
            first, done: true, ..
        } = self.tail
        else {
            return;
        };
        let Some(last) = self.recs.range(first..).next_back().map(|(s, _)| *s) else {
            self.tail = Tail::Closed;
            return;
        };
        let records = self.recs.range(first..=last).count();
        self.groups.insert(
            first,
            Group {
                last,
                records: u32::try_from(records).unwrap_or(u32::MAX),
                acked: 0,
            },
        );
        self.ready_through = last;
        self.tail = Tail::Closed;
    }

    /// The delta being written failed (a write or a sync): drop it, as lost.
    pub fn abort_open(&mut self) {
        self.abandon_open();
        self.tail = Tail::Closed;
    }

    /// Anything still open when the spool was opened is a delta that never finished.
    pub fn finish_recovery(&mut self) {
        self.abort_open();
    }

    fn abandon_open(&mut self) {
        if let Tail::Open { first, .. } = self.tail {
            self.remove_range(first, u64::MAX, true);
        }
    }

    // -------------------------------------------------------------------------------------------- recovery

    /// A record found on disk while opening. Applies the same rules as a live append, without the bounds: those are
    /// enforced once everything is read ([`Index::enforce_limits`]).
    pub fn observe(&mut self, meta: &Meta, seg: u64, offset: u64, frame_len: u64) -> Result<(), OutOfOrder> {
        match self.begin(meta)? {
            Admit::Store => {
                self.commit(meta, seg, offset, frame_len, &[]);
                if !meta.more {
                    self.finish_group();
                }
            }
            Admit::Discard => self.reject(meta),
        }
        Ok(())
    }

    /// Drop the oldest deltas until the bounds hold (the limits may have been lowered since the last run).
    pub fn enforce_limits(&mut self) {
        while (self.disk > self.limits.max_bytes || self.entries > self.limits.max_entries)
            && self.drop_oldest()
        {}
    }

    // -------------------------------------------------------------------------------------------- dedup

    /// Remove from `delta` every version identical to the one before it that is still held, and return the claims the
    /// remaining ones make. "Before it" is the previous held version of the same path, so A, B, A keeps all three and
    /// A, A keeps one. A version that was acknowledged or dropped is no longer held, so it never hides a new one.
    pub fn dedupe(&self, delta: &mut ScanDelta) -> Vec<(Key, Version)> {
        let mut claims = Vec::new();
        let mut keep = |path: &str, version: Version| {
            let key = key_of(path);
            if self.latest.get(&key).is_some_and(|(_, held)| *held == version) {
                false
            } else {
                claims.push((key, version));
                true
            }
        };
        delta.entries.retain(|e| {
            keep(
                e.path.as_str(),
                Version::File {
                    hash: e.hash,
                    denied: e.denied,
                },
            )
        });
        delta.removed.retain(|p| keep(p.as_str(), Version::Removed));
        claims
    }

    // -------------------------------------------------------------------------------------------- acknowledgements

    /// The hub acknowledged `seq`. Returns true when that released the last part of a delta.
    pub fn ack(&mut self, seq: u64) -> bool {
        if seq > self.ready_through {
            return false;
        }
        let Some(rec) = self.recs.get_mut(&seq) else {
            return false;
        };
        if rec.acked {
            return false;
        }
        rec.acked = true;
        if let Some((carrier, epoch)) = self.carrier {
            if carrier == seq {
                self.carrier = None;
                if self.gap.is_some_and(|g| g.epoch == epoch) {
                    self.gap = None;
                    self.gap_dirty = true;
                }
            }
        }
        let Some((&first, group)) = self.groups.range_mut(..=seq).next_back() else {
            return false;
        };
        if seq > group.last {
            return false;
        }
        group.acked += 1;
        if group.acked < group.records {
            return false;
        }
        let last = group.last;
        self.groups.remove(&first);
        self.remove_range(first, last, false);
        true
    }

    /// A record that was stored could not be read back whole: drop its whole delta, as lost.
    pub fn note_damaged(&mut self, seq: u64) {
        let Some((&first, &group)) = self.groups.range(..=seq).next_back() else {
            return;
        };
        if seq > group.last {
            return;
        }
        self.groups.remove(&first);
        self.remove_range(first, group.last, true);
    }

    // -------------------------------------------------------------------------------------------- sending

    /// The next record to send after `cursor`: complete deltas only, in order, first part first. Stamps the pending
    /// loss on the first message that can carry it.
    pub fn next_after(&mut self, cursor: u64) -> Option<Next> {
        if cursor >= self.ready_through {
            return None;
        }
        let (&seq, rec) = self
            .recs
            .range((Bound::Excluded(cursor), Bound::Included(self.ready_through)))
            .next()?;
        let (segment_id, offset, frame_len, meta) = (rec.seg, rec.offset, rec.frame_len, rec.meta);
        let mut gap = None;
        if meta.part == 0 {
            if let Some(owed) = self.gap {
                let carried = self
                    .carrier
                    .is_some_and(|(c, e)| e == owed.epoch && self.recs.contains_key(&c));
                if !carried {
                    self.carrier = Some((seq, owed.epoch));
                    gap = Some(owed.as_gap());
                }
            }
        }
        Some(Next {
            seg: segment_id,
            offset,
            frame_len,
            meta,
            gap,
        })
    }

    /// The sequence number of the last complete delta: everything up to it can be sent.
    pub fn ready_through(&self) -> u64 {
        self.ready_through
    }

    /// A new connection: the loss is owed to it afresh.
    pub fn reset_carrier(&mut self) {
        self.carrier = None;
    }

    // -------------------------------------------------------------------------------------------- loss

    /// What the hub is owed, if anything.
    pub fn gap(&self) -> Option<SpoolGap> {
        self.gap.map(GapAcc::as_gap)
    }

    /// Take the loss a previous run wrote down.
    pub fn restore_gap(&mut self, gap: SpoolGap) {
        self.note_range(gap.from.unix_millis(), gap.to.unix_millis(), gap.lost_entries);
    }

    /// `true` once after the pending loss changed, so the caller writes it down.
    pub fn take_gap_dirty(&mut self) -> bool {
        std::mem::take(&mut self.gap_dirty)
    }

    /// The pending loss changed since it was last written down.
    pub fn gap_is_dirty(&self) -> bool {
        self.gap_dirty
    }

    /// Writing the pending loss down failed: it is still to be written.
    pub fn restore_dirty(&mut self) {
        self.gap_dirty = true;
    }

    /// True while the spool holds a record with this sequence number.
    pub fn holds(&self, seq: u64) -> bool {
        self.recs.contains_key(&seq)
    }

    /// True when the hub has acknowledged `seq`, or the spool no longer holds it.
    pub fn is_settled(&self, seq: u64) -> bool {
        self.recs.get(&seq).is_none_or(|r| r.acked)
    }

    /// Lost: `entries` versions seen between `first_ms` and `last_ms` in any order.
    pub fn note_loss(&mut self, first_ms: i64, last_ms: i64, entries: u64) {
        if entries == 0 {
            return;
        }
        self.lost_total = self.lost_total.saturating_add(entries);
        self.note_range(first_ms, last_ms, entries);
    }

    fn note_range(&mut self, a: i64, b: i64, entries: u64) {
        let (lo, hi) = (a.min(b), a.max(b));
        self.epoch += 1;
        let epoch = self.epoch;
        self.gap = Some(match self.gap {
            Some(g) => GapAcc {
                from: g.from.min(lo),
                to: g.to.max(hi),
                lost: g.lost.saturating_add(entries),
                epoch,
            },
            None => GapAcc {
                from: lo,
                to: hi,
                lost: entries,
                epoch,
            },
        });
        self.gap_dirty = true;
    }

    fn remove_range(&mut self, first: u64, last: u64, loss: bool) {
        let seqs: Vec<u64> = self.recs.range(first..=last).map(|(s, _)| *s).collect();
        let (mut earliest, mut newest, mut dropped) = (i64::MAX, i64::MIN, 0_u64);
        for seq in seqs {
            let Some(rec) = self.recs.remove(&seq) else {
                continue;
            };
            self.entries = self.entries.saturating_sub(u64::from(rec.meta.entries));
            earliest = earliest.min(rec.meta.first_ms);
            newest = newest.max(rec.meta.last_ms);
            dropped += u64::from(rec.meta.entries);
            for key in &rec.keys {
                if self.latest.get(key).is_some_and(|(s, _)| *s == seq) {
                    self.latest.remove(key);
                }
            }
            self.release(rec.seg);
        }
        if loss {
            self.note_loss(earliest, newest, dropped);
        }
    }
}

/// The dedup key of a path.
pub fn key_of(path: &str) -> Key {
    let digest = Sha256::digest(path.as_bytes());
    let mut key = [0_u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use domain::{NfsPath, ScanEntry};
    use proptest::prelude::*;

    use super::*;

    const SEG: u64 = 1;

    fn ms(seq: u64) -> i64 {
        i64::try_from(seq).unwrap()
    }

    fn limits(max_entries: u64) -> Limits {
        Limits {
            max_bytes: 1 << 30,
            max_entries,
            segment_bytes: 1 << 20,
        }
    }

    fn meta(seq: u64, part: u32, more: bool, entries: u32, first: i64, last: i64) -> Meta {
        Meta {
            seq,
            part,
            more,
            entries,
            first_ms: first,
            last_ms: last,
        }
    }

    fn index(max_entries: u64) -> Index {
        let mut index = Index::new(limits(max_entries));
        index.register_segment(SEG, 8);
        index.set_active(Some(SEG));
        index
    }

    /// Store a whole single-message delta.
    fn store(index: &mut Index, seq: u64, entries: u32, time: i64) {
        let m = meta(seq, 0, false, entries, time, time);
        assert_eq!(index.begin(&m).unwrap(), Admit::Store);
        index.commit(&m, SEG, seq * 100, 50, &[]);
        index.grow_segment(SEG, 50);
        index.finish_group();
    }

    #[test]
    fn a_group_is_sent_only_when_complete_and_always_from_its_first_part() {
        let mut index = index(100);
        let first = meta(1, 0, true, 1, 10, 10);
        assert_eq!(index.begin(&first).unwrap(), Admit::Store);
        index.commit(&first, SEG, 8, 50, &[]);
        assert!(index.next_after(0).is_none(), "an open group is not sent");
        let second = meta(2, 1, false, 1, 20, 20);
        assert_eq!(index.begin(&second).unwrap(), Admit::Store);
        index.commit(&second, SEG, 58, 50, &[]);
        assert!(index.next_after(0).is_none(), "not until it is durable");
        index.finish_group();
        assert_eq!(index.next_after(0).unwrap().meta.seq, 1);
        assert_eq!(index.next_after(1).unwrap().meta.seq, 2);
        assert!(index.next_after(2).is_none());
    }

    #[test]
    fn the_cursor_may_be_past_everything_without_a_panic() {
        let mut index = index(100);
        assert!(index.next_after(0).is_none());
        store(&mut index, 5, 1, 1);
        assert!(index.next_after(5).is_none());
        assert!(index.next_after(u64::MAX).is_none());
        assert!(index.next_after(4).is_some());
    }

    #[test]
    fn a_sequence_number_must_increase_even_after_a_rejected_record() {
        let mut index = index(100);
        store(&mut index, 3, 1, 1);
        assert_eq!(
            index.begin(&meta(3, 0, false, 1, 1, 1)),
            Err(OutOfOrder { got: 3, last: 3 })
        );
        assert_eq!(
            index.begin(&meta(2, 0, false, 1, 1, 1)),
            Err(OutOfOrder { got: 2, last: 3 })
        );
        let m = meta(9, 0, false, 1, 1, 1);
        index.begin(&m).unwrap();
        index.reject(&m);
        assert_eq!(index.begin(&meta(9, 0, false, 1, 1, 1)).unwrap_err().last, 9);
    }

    #[test]
    fn a_part_that_does_not_follow_its_predecessor_ends_the_delta_and_is_discarded() {
        let mut index = index(100);
        let a = meta(1, 0, true, 1, 10, 10);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        // Part 2 arrives where part 1 should: the delta is not whole.
        let c = meta(3, 2, false, 1, 30, 30);
        assert_eq!(index.begin(&c).unwrap(), Admit::Discard);
        index.reject(&c);
        assert_eq!(index.stats().records, 0, "the first part went too");
        let gap = index.gap().unwrap();
        assert_eq!(
            (gap.lost_entries, gap.from.unix_millis(), gap.to.unix_millis()),
            (2, 10, 30)
        );
    }

    #[test]
    fn a_new_first_part_abandons_a_delta_left_open() {
        let mut index = index(100);
        let a = meta(1, 0, true, 2, 10, 15);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        store(&mut index, 2, 1, 40);
        assert_eq!(index.stats().records, 1);
        assert_eq!(index.gap().unwrap().lost_entries, 2);
    }

    #[test]
    fn dropping_takes_the_oldest_whole_delta_and_reports_its_range() {
        let mut index = index(100);
        for (seq, t) in [(1, 300), (2, 100), (3, 200)] {
            store(&mut index, seq, 2, t);
        }
        assert!(index.drop_oldest());
        let stats = index.stats();
        assert_eq!((stats.records, stats.entries, stats.groups), (2, 4, 2));
        let gap = index.gap().unwrap();
        assert_eq!(
            (gap.from.unix_millis(), gap.to.unix_millis(), gap.lost_entries),
            (300, 300, 2)
        );
        assert!(index.drop_oldest());
        let gap = index.gap().unwrap();
        // min and max, whatever order the deltas were seen in.
        assert_eq!(
            (gap.from.unix_millis(), gap.to.unix_millis(), gap.lost_entries),
            (100, 300, 4)
        );
        assert!(gap.from <= gap.to);
        assert_eq!(index.lost_total(), 4);
    }

    #[test]
    fn dropping_with_nothing_complete_reports_that() {
        let mut index = index(100);
        assert!(!index.drop_oldest());
        let a = meta(1, 0, true, 1, 1, 1);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        assert!(
            !index.drop_oldest(),
            "an open delta is not dropped to make room for itself"
        );
        assert!(index.has_open_records());
    }

    #[test]
    fn a_poisoned_delta_discards_its_remaining_parts_and_then_ends() {
        let mut index = index(100);
        let a = meta(1, 0, true, 1, 10, 10);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        let b = meta(2, 1, true, 1, 20, 20);
        index.begin(&b).unwrap();
        index.poison_open(&b);
        index.reject(&b);
        assert_eq!(index.stats().records, 0);
        let c = meta(3, 2, false, 1, 30, 30);
        assert_eq!(index.begin(&c).unwrap(), Admit::Discard);
        index.reject(&c);
        assert_eq!(index.gap().unwrap().lost_entries, 3);
        // And the next delta starts clean.
        store(&mut index, 4, 1, 40);
        assert_eq!(index.stats().records, 1);
    }

    #[test]
    fn acknowledging_every_part_releases_the_group_and_its_segment() {
        let mut index = index(100);
        index.register_segment(2, 8);
        index.set_active(Some(2));
        let a = meta(1, 0, true, 1, 10, 10);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        let b = meta(2, 1, false, 1, 20, 20);
        index.begin(&b).unwrap();
        index.commit(&b, SEG, 58, 50, &[]);
        index.finish_group();
        assert!(!index.ack(1));
        assert_eq!(index.stats().records, 2, "half acknowledged is not released");
        assert!(!index.ack(1), "twice is once");
        assert!(index.ack(2));
        assert_eq!(
            index.stats(),
            IndexStats {
                disk_bytes: 8,
                ..IndexStats::default()
            }
        );
        assert_eq!(index.take_freed(), [SEG]);
        assert!(index.take_freed().is_empty());
    }

    #[test]
    fn the_active_segment_is_never_freed_behind_the_writers_back() {
        let mut index = index(100);
        store(&mut index, 1, 1, 1);
        assert!(index.ack(1));
        assert!(index.take_freed().is_empty(), "the writer still has it open");
        assert!(index.segment_is_dead(SEG));
        index.free_segment(SEG);
        assert_eq!(index.take_freed(), [SEG]);
        assert_eq!(index.stats().disk_bytes, 0);
    }

    #[test]
    fn an_acknowledgement_for_something_not_sent_or_not_held_changes_nothing() {
        let mut index = index(100);
        let a = meta(1, 0, true, 1, 10, 10);
        index.begin(&a).unwrap();
        index.commit(&a, SEG, 8, 50, &[]);
        assert!(!index.ack(1), "an open group was never sent");
        index.finish_group();
        let before = index.stats();
        assert!(!index.ack(77));
        assert!(!index.ack(0));
        assert_eq!(index.stats(), before);
    }

    #[test]
    fn the_gap_is_stamped_once_per_connection_until_the_carrier_is_acknowledged() {
        let mut index = index(100);
        for seq in 1..=3 {
            store(&mut index, seq, 1, 100 * ms(seq));
        }
        assert!(index.drop_oldest());
        let first = index.next_after(0).unwrap();
        assert!(first.gap.is_some());
        assert!(
            index.next_after(first.meta.seq).unwrap().gap.is_none(),
            "once is enough"
        );
        index.reset_carrier();
        assert_eq!(
            index.next_after(0).unwrap().gap,
            first.gap,
            "a new connection is told again"
        );
        assert!(index.ack(first.meta.seq));
        assert!(index.gap().is_none(), "acknowledged, so it is paid");
        assert!(index.take_gap_dirty());
    }

    #[test]
    fn a_loss_after_the_stamp_is_still_owed_when_the_older_message_is_acknowledged() {
        let mut index = index(100);
        for seq in 1..=4 {
            store(&mut index, seq, 1, 100 * ms(seq));
        }
        assert!(index.drop_oldest()); // seq 1
        let stamped = index.next_after(0).unwrap(); // seq 2 carries it
        assert!(stamped.gap.is_some());
        // Seq 3 turns out to be unreadable while seq 2 is in flight.
        index.note_damaged(3);
        assert!(index.ack(stamped.meta.seq));
        let owed = index
            .gap()
            .expect("the later loss is not covered by the earlier message");
        assert_eq!(owed.lost_entries, 2);
        // And it goes out with the next message that can carry it.
        assert!(index.next_after(0).unwrap().gap.is_some());
    }

    #[test]
    fn a_carrier_that_was_dropped_leaves_the_gap_to_the_next_message() {
        let mut index = index(100);
        for seq in 1..=3 {
            store(&mut index, seq, 1, ms(seq));
        }
        assert!(index.drop_oldest());
        let stamped = index.next_after(0).unwrap();
        assert_eq!(stamped.meta.seq, 2);
        assert!(index.drop_oldest(), "the message that carried it is dropped too");
        let next = index.next_after(0).unwrap();
        assert_eq!(next.meta.seq, 3);
        assert_eq!(next.gap.unwrap().lost_entries, 2);
    }

    #[test]
    fn a_restored_gap_merges_with_new_loss() {
        let mut index = index(100);
        index.restore_gap(SpoolGap {
            from: Timestamp::from_unix_millis(500),
            to: Timestamp::from_unix_millis(600),
            lost_entries: 4,
        });
        index.note_loss(1_000, 900, 2);
        let gap = index.gap().unwrap();
        assert_eq!(
            (gap.from.unix_millis(), gap.to.unix_millis(), gap.lost_entries),
            (500, 1_000, 6)
        );
        // A restored gap is not new loss in this run.
        assert_eq!(index.lost_total(), 2);
    }

    #[test]
    fn a_loss_with_no_entries_is_not_a_gap() {
        let mut index = index(100);
        index.note_loss(1, 2, 0);
        assert!(index.gap().is_none());
    }

    fn entry(path: &str, content: &[u8]) -> ScanEntry {
        ScanEntry {
            path: NfsPath::parse(path).unwrap(),
            hash: ContentHash::from_bytes(Sha256::digest(content).into()),
            size: content.len() as u64,
            mtime: Timestamp::from_unix_millis(1),
            observed_at: Timestamp::from_unix_millis(1),
            denied: false,
            bytes: Some(Bytes::copy_from_slice(content)),
        }
    }

    fn delta_of(entries: Vec<ScanEntry>) -> ScanDelta {
        ScanDelta {
            seq: 1,
            base_root: None,
            new_root: ContentHash::from_bytes([0; 32]),
            entries,
            removed: Vec::new(),
            skipped: Vec::new(),
            during_job: None,
            more: false,
            part: 0,
            gap: None,
        }
    }

    #[test]
    fn dedupe_drops_only_a_repeat_of_the_version_before() {
        let mut index = index(100);
        let mut first = delta_of(vec![entry("a.yml", b"A")]);
        let claims = index.dedupe(&mut first);
        assert_eq!((first.entries.len(), claims.len()), (1, 1));
        let m = meta(1, 0, false, 1, 1, 1);
        index.begin(&m).unwrap();
        index.commit(&m, SEG, 8, 50, &claims);
        index.finish_group();

        let mut repeat = delta_of(vec![entry("a.yml", b"A"), entry("b.yml", b"B")]);
        let claims = index.dedupe(&mut repeat);
        assert_eq!(repeat.entries.len(), 1);
        assert_eq!(repeat.entries[0].path.as_str(), "b.yml");
        assert_eq!(claims.len(), 1);

        // B, then A again: A is not a repeat of the version before it.
        let mut change = delta_of(vec![entry("a.yml", b"B")]);
        let claims = index.dedupe(&mut change);
        let m = meta(2, 0, false, 1, 2, 2);
        index.begin(&m).unwrap();
        index.commit(&m, SEG, 58, 50, &claims);
        index.finish_group();
        let mut back = delta_of(vec![entry("a.yml", b"A")]);
        assert_eq!(index.dedupe(&mut back).len(), 1);
        assert_eq!(back.entries.len(), 1);
    }

    #[test]
    fn dedupe_forgets_versions_that_are_acknowledged_or_dropped() {
        let mut index = index(100);
        for (seq, name) in [(1_u64, "a.yml"), (2, "b.yml")] {
            let mut d = delta_of(vec![entry(name, b"x")]);
            let claims = index.dedupe(&mut d);
            let m = meta(seq, 0, false, 1, 1, 1);
            index.begin(&m).unwrap();
            index.commit(&m, SEG, seq * 10, 50, &claims);
            index.finish_group();
        }
        index.ack(1);
        let mut again_a = delta_of(vec![entry("a.yml", b"x")]);
        index.dedupe(&mut again_a);
        assert_eq!(again_a.entries.len(), 1, "acknowledged: it is the hub's now");
        assert!(index.drop_oldest());
        let mut again_b = delta_of(vec![entry("b.yml", b"x")]);
        index.dedupe(&mut again_b);
        assert_eq!(again_b.entries.len(), 1, "dropped: nothing holds it any more");
        assert!(index.latest.is_empty(), "the table does not outlive its records");
    }

    #[test]
    fn observing_records_in_file_order_rebuilds_groups_and_discards_an_unfinished_one() {
        let mut index = Index::new(limits(100));
        index.register_segment(SEG, 400);
        for m in [
            meta(1, 0, false, 1, 10, 10),
            meta(2, 0, true, 1, 20, 20),
            meta(3, 1, false, 1, 30, 30),
            meta(4, 0, true, 2, 40, 45),
        ] {
            index.observe(&m, SEG, m.seq * 50, 50).unwrap();
        }
        index.finish_recovery();
        let stats = index.stats();
        assert_eq!((stats.records, stats.groups, stats.entries), (3, 2, 3));
        assert_eq!(index.gap().unwrap().lost_entries, 2);
        assert_eq!(index.next_after(0).unwrap().meta.seq, 1);
    }

    #[test]
    fn observing_a_record_out_of_order_is_an_error_for_the_caller_to_treat_as_damage() {
        let mut index = Index::new(limits(100));
        index.register_segment(SEG, 400);
        let m = meta(5, 0, false, 1, 1, 1);
        index.observe(&m, SEG, 8, 50).unwrap();
        assert!(index.observe(&meta(5, 0, false, 1, 1, 1), SEG, 58, 50).is_err());
    }

    #[test]
    fn recovery_enforces_limits_lowered_since_the_last_run() {
        let mut index = Index::new(limits(3));
        index.register_segment(SEG, 400);
        for seq in 1..=5 {
            let m = meta(seq, 0, false, 1, ms(seq), ms(seq));
            index.observe(&m, SEG, seq * 50, 50).unwrap();
        }
        index.finish_recovery();
        index.enforce_limits();
        assert_eq!(index.stats().entries, 3);
        assert_eq!(index.next_after(0).unwrap().meta.seq, 3);
    }

    proptest! {
        /// Whatever is stored, acknowledged and dropped, in any order: the entry count equals the sum over the live
        /// records, no segment is counted that holds nothing, a gap range never runs backwards, and what is sent is
        /// always a record that is held.
        #[test]
        fn the_books_always_balance(ops in proptest::collection::vec((0_u8..4, 1_u32..4, 0_i64..1_000), 1..60)) {
            let mut index = index(8);
            let mut seq = 0_u64;
            for (op, entries, time) in ops {
                match op {
                    0 | 1 => {
                        seq += 1;
                        let m = meta(seq, 0, false, entries, time, time + 5);
                        index.begin(&m).unwrap();
                        while !index.fits(50, u64::from(entries)) {
                            if !index.drop_oldest() {
                                break;
                            }
                        }
                        if index.fits(50, u64::from(entries)) {
                            index.commit(&m, SEG, seq * 50, 50, &[]);
                            index.grow_segment(SEG, 50);
                            index.finish_group();
                        } else {
                            index.reject(&m);
                        }
                    }
                    2 => {
                        if let Some(next) = index.next_after(seq.saturating_sub(u64::from(entries) + 1)) {
                            let _ = index.ack(next.meta.seq);
                        }
                    }
                    _ => {
                        let _ = index.drop_oldest();
                    }
                }
                let stats = index.stats();
                prop_assert!(stats.entries <= 8, "{stats:?}");
                let summed: u64 = index.recs.values().map(|r| u64::from(r.meta.entries)).sum();
                prop_assert_eq!(summed, stats.entries);
                if let Some(g) = index.gap() {
                    prop_assert!(g.from <= g.to);
                }
                let live: u32 = index.segs.values().map(|s| s.live).sum();
                prop_assert_eq!(live as usize, index.recs.len());
                for take in index.take_freed() {
                    prop_assert!(!index.segs.contains_key(&take));
                }
            }
        }
    }
}
