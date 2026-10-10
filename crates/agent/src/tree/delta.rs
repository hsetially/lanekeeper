//! Deltas (T4, D63, D44, Q12): the difference between two trees as messages for the hub.
//!
//! # What a delta promises
//!
//! - **Every entry is true to its bytes.** Each file is read through cap-std when its message is built, and its hash is
//!   the SHA-256 of the bytes that go out. The tree's hash is a promise made at scan time; the file may have changed
//!   since, and the bytes win.
//! - **`new_root` matches the entries** as far as the agent can know before it sends. Before the first message the
//!   files are checked again (a stat each, and a rehash of any that moved), the tree is patched with what was found,
//!   and `new_root` is the root of that patched tree. A file that changes in the few milliseconds between that check and
//!   its read is sent as it is, true to its bytes, and the tree the agent keeps is patched again. Its root then differs
//!   from `new_root`, the next heartbeat shows that, and the hub asks for the small delta from `new_root`, which the
//!   ring still holds ([`DeltaOutcome::trees`]).
//! - **Messages are small.** At most 3 MiB of file bytes, at most 10,000 entries (files, removals and skips together)
//!   and, by an overestimate, 3.75 MiB on the wire, under gRPC's 4 MiB. The parts of one logical delta share their two
//!   roots, are numbered from 0, and say `more` on all but the last, so the hub applies a delta only when it is whole.
//!   Parts are built one at a time and handed to the [`DeltaSink`], which waits for room: memory is one part, not the
//!   delta.
//! - **Denied files send no bytes** (D79) and are never read; files over the size cap are reported as skipped and are
//!   never read either.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use domain::{JobRef, NfsPath, ScanDelta, ScanEntry, ShortText, SkippedEntry, Timestamp};
use tracing::{debug, warn};

use super::node::{ChangedFile, Entry, FileLeaf, MerkleTree, Skip, SkipReason, TreeDiff};
use super::source::{FileRead, ReadError, ReadRequest, RefreshRequest, Refreshed, TreeSource};
use crate::clock::Clock;
use crate::config::limits;

/// Estimated bytes on the wire per message. The real limit is 4 MiB; the estimate counts every path and a generous
/// 96 bytes of fixed fields for each entry, so it is never below the truth.
const MAX_WIRE_ESTIMATE: usize = 3 * 1024 * 1024 + 768 * 1024;
const ENTRY_OVERHEAD: usize = 96;
const SMALL_OVERHEAD: usize = 8;

/// Why a message could not be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The connection is gone, or there is none.
    #[error("the connection is closed")]
    Closed,
    /// The message cannot be sent at all (over the wire limit).
    #[error("the message is too large to send")]
    TooLarge,
    /// The spool could not keep the message (a full or failing volume). The hub learns the new root from the heartbeat and
    /// asks for the difference.
    #[error("the message could not be stored")]
    Storage,
}

/// Where finished delta messages go. Delivering waits for room, so a slow hub slows the builder down instead of
/// filling memory.
#[async_trait]
pub trait DeltaSink: Send + Sync + fmt::Debug {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError>;

    /// False when a delta would be refused at once (no connection). A caller that can choose skips building it: reading
    /// every changed file just to learn that nobody is listening is the wrong use of an outage.
    fn ready(&self) -> bool {
        true
    }
}

/// The sequence numbers of delta messages: one counter for everything the agent sends, never repeating.
#[derive(Debug)]
pub struct SeqCounter(AtomicU64);

impl SeqCounter {
    /// A counter whose first number is `first`.
    pub fn new(first: u64) -> Self {
        Self(AtomicU64::new(first))
    }

    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }

    /// The number the next message will get.
    pub fn peek(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeltaError {
    /// A message could not be delivered. The parts already sent say `more`, so the hub ignores them.
    #[error("a delta message could not be delivered: {0}")]
    Sink(#[from] SinkError),
}

/// A tree the ring should remember, with the bytes building it allocated.
#[derive(Debug, Clone)]
pub struct Remembered {
    pub tree: MerkleTree,
    pub new_bytes: usize,
}

/// What a finished delta leaves behind.
#[derive(Debug, Clone)]
pub struct DeltaOutcome {
    /// The tree the agent should keep: the one given, patched with everything the check and the reads found.
    pub tree: MerkleTree,
    /// The trees worth remembering in the ring, oldest first. The first has the root that went out as `new_root`; a
    /// second follows when a file changed during the send.
    pub trees: Vec<Remembered>,
    pub messages: u32,
    pub entries: usize,
    pub removed: usize,
    pub skipped: usize,
    /// Files whose bytes at read time were not the ones the check saw.
    pub late_patches: usize,
}

/// A file to send.
#[derive(Debug, Clone)]
struct Item {
    path: NfsPath,
    leaf: FileLeaf,
}

enum Unit {
    Removed(NfsPath),
    Skipped(SkippedEntry),
    /// A denied file: name, size and hash only.
    Denied(Item),
    File {
        item: Item,
        read: Option<Result<FileRead, ReadError>>,
    },
}

impl Unit {
    /// Estimated bytes on the wire, and the file bytes it carries.
    fn cost(&self) -> (usize, usize) {
        match self {
            Self::Removed(path) => (path.as_str().len() + SMALL_OVERHEAD, 0),
            Self::Skipped(skip) => (
                skip.path.as_str().len() + skip.reason.as_str().len() + SMALL_OVERHEAD,
                0,
            ),
            Self::Denied(item) => (item.path.as_str().len() + ENTRY_OVERHEAD, 0),
            Self::File { item, read } => {
                let payload = match read {
                    Some(Ok(read)) => read.bytes.len(),
                    _ => usize::try_from(item.leaf.stat.size).unwrap_or(usize::MAX),
                };
                (item.path.as_str().len() + ENTRY_OVERHEAD + payload, payload)
            }
        }
    }
}

/// Builds and sends the messages of one logical delta.
pub struct DeltaBuilder<'a> {
    source: &'a dyn TreeSource,
    clock: &'a dyn Clock,
    seq: &'a SeqCounter,
    max_file_bytes: u64,
    during_job: Option<&'a JobRef>,
}

impl fmt::Debug for DeltaBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeltaBuilder")
            .field("max_file_bytes", &self.max_file_bytes)
            .finish_non_exhaustive()
    }
}

impl<'a> DeltaBuilder<'a> {
    pub fn new(source: &'a dyn TreeSource, clock: &'a dyn Clock, seq: &'a SeqCounter) -> Self {
        Self {
            source,
            clock,
            seq,
            max_file_bytes: limits::MAX_FILE_BYTES,
            during_job: None,
        }
    }

    /// The most bytes of a file to send. The hub can lower the 2 MiB cap, never raise it.
    #[must_use]
    pub fn with_max_file_bytes(mut self, max: u64) -> Self {
        self.max_file_bytes = max.min(limits::MAX_FILE_BYTES);
        self
    }

    /// The sync Job to tag every message with (D72).
    #[must_use]
    pub fn with_job(mut self, job: Option<&'a JobRef>) -> Self {
        self.during_job = job;
        self
    }

    /// Send what changed from `base` to `tree` (a full listing when `base` is `None`).
    pub async fn send(
        &self,
        base: Option<&MerkleTree>,
        tree: &MerkleTree,
        sink: &dyn DeltaSink,
    ) -> Result<DeltaOutcome, DeltaError> {
        let diff = base.map_or_else(|| tree.full_listing(), |b| tree.diff(b));
        let mut plan = self.plan(diff);
        let mut remembered = Vec::with_capacity(2);

        // Check the files again before anything goes out, so the root in every message is the root of what is sent.
        let mut current = tree.clone();
        let refreshed = self.refresh(base, &mut plan, &mut current).await;
        let sent_tree = current.clone();
        remembered.push(Remembered {
            tree: sent_tree.clone(),
            new_bytes: refreshed,
        });
        let new_root = sent_tree.root_hash();
        let base_root = base.map(MerkleTree::root_hash);

        let mut queue = Self::units(plan);
        let mut late_edits: Vec<LateEdit> = Vec::new();
        let (mut messages, mut entries, mut removed, mut skipped) = (0_u32, 0_usize, 0_usize, 0_usize);
        loop {
            let mut batch = take_batch(&mut queue);
            self.read_batch(&mut batch, &mut late_edits).await;
            give_back_overflow(&mut batch, &mut queue);
            let message = self.message(batch, base_root, new_root, !queue.is_empty(), messages);
            entries += message.entries.len();
            removed += message.removed.len();
            skipped += message.skipped.len();
            let more = message.more;
            sink.deliver(message).await?;
            messages += 1;
            if !more {
                break;
            }
        }

        let late_patches = late_edits
            .iter()
            .filter(|e| matches!(e, LateEdit::Put(..)))
            .count();
        let mut final_tree = current;
        let mut late_bytes = 0;
        for edit in late_edits {
            let edited = match edit {
                LateEdit::Put(path, leaf) => final_tree.put(path.as_str(), Entry::File(leaf)),
                LateEdit::Remove(path) => final_tree.remove(path.as_str()),
            };
            late_bytes += edited.new_bytes;
            final_tree = edited.tree;
        }
        if final_tree.root_hash() != new_root {
            remembered.push(Remembered {
                tree: final_tree.clone(),
                new_bytes: late_bytes,
            });
        }
        debug!(messages, entries, removed, skipped, late_patches, "delta sent");
        Ok(DeltaOutcome {
            tree: final_tree,
            trees: remembered,
            messages,
            entries,
            removed,
            skipped,
            late_patches,
        })
    }

    /// Turn the tree diff into paths the wire accepts, and decide which files are sent, denied or too big.
    fn plan(&self, diff: TreeDiff) -> Plan {
        let mut plan = Plan::default();
        for ChangedFile { path, leaf } in diff.changed {
            let Some(path) = wire_path(&path) else { continue };
            if leaf.denied {
                plan.denied.push(Item { path, leaf });
            } else if leaf.stat.size > self.max_file_bytes {
                push_skip(&mut plan.skipped, path, SkipReason::TooLarge);
            } else {
                plan.files.push(Item { path, leaf });
            }
        }
        plan.removed = diff.removed.iter().filter_map(|p| wire_path(p)).collect();
        for Skip { path, reason } in diff.skipped {
            if let Some(path) = wire_path(&path) {
                push_skip(&mut plan.skipped, path, reason);
            }
        }
        plan
    }

    /// Ask the source about every file that is going to be read. Patches `current` and the plan with what it finds and
    /// returns the bytes the patching allocated.
    async fn refresh(&self, base: Option<&MerkleTree>, plan: &mut Plan, current: &mut MerkleTree) -> usize {
        let requests: Vec<RefreshRequest> = plan
            .files
            .iter()
            .chain(&plan.denied)
            .map(|i| RefreshRequest {
                path: i.path.clone(),
                leaf: i.leaf,
            })
            .collect();
        if requests.is_empty() {
            return 0;
        }
        let mut answers = self.source.refresh(requests).await.into_iter();
        let files = std::mem::take(&mut plan.files);
        let denied = std::mem::take(&mut plan.denied);
        let file_count = files.len();
        let mut new_bytes = 0;
        for (i, item) in files.into_iter().chain(denied).enumerate() {
            let answer = answers.next().unwrap_or(Refreshed::Failed);
            let Some(item) = self.settle(base, current, &mut new_bytes, item, answer, plan) else {
                continue;
            };
            if i < file_count {
                plan.files.push(item);
            } else {
                plan.denied.push(item);
            }
        }
        new_bytes
    }

    /// Apply one answer of the check. `None`: the file drops out of the plan.
    fn settle(
        &self,
        base: Option<&MerkleTree>,
        current: &mut MerkleTree,
        new_bytes: &mut usize,
        mut item: Item,
        answer: Refreshed,
        plan: &mut Plan,
    ) -> Option<Item> {
        let in_base = || match base.and_then(|b| b.get(item.path.as_str())) {
            Some(Entry::File(leaf)) => Some(*leaf),
            _ => None,
        };
        match answer {
            Refreshed::Same | Refreshed::Failed => Some(item),
            Refreshed::Changed(leaf) => {
                let edited = current.put(item.path.as_str(), Entry::File(leaf));
                *new_bytes += edited.new_bytes;
                *current = edited.tree;
                item.leaf = leaf;
                if in_base().is_some_and(|b| b.hash == leaf.hash && b.denied == leaf.denied) {
                    // Changed and changed back since the base: there is nothing to tell the hub.
                    None
                } else if !leaf.denied && leaf.stat.size > self.max_file_bytes {
                    push_skip(&mut plan.skipped, item.path, SkipReason::TooLarge);
                    None
                } else {
                    Some(item)
                }
            }
            Refreshed::Gone => {
                let edited = current.remove(item.path.as_str());
                *new_bytes += edited.new_bytes;
                *current = edited.tree;
                if in_base().is_some() {
                    plan.removed.push(item.path);
                }
                None
            }
        }
    }

    /// The order things are sent in: removals and skips are cheap and go first, then denied files, then file bytes.
    fn units(plan: Plan) -> VecDeque<Unit> {
        let mut queue = VecDeque::with_capacity(plan.removed.len() + plan.skipped.len() + plan.files.len());
        queue.extend(plan.removed.into_iter().map(Unit::Removed));
        queue.extend(plan.skipped.into_iter().map(Unit::Skipped));
        queue.extend(plan.denied.into_iter().map(Unit::Denied));
        queue.extend(plan.files.into_iter().map(|item| Unit::File { item, read: None }));
        queue
    }

    /// Read the files of one batch that have not been read yet, and settle what each read means.
    async fn read_batch(&self, batch: &mut Vec<Unit>, late: &mut Vec<LateEdit>) {
        let fresh: Vec<bool> = batch
            .iter()
            .map(|u| matches!(u, Unit::File { read: None, .. }))
            .collect();
        let requests: Vec<ReadRequest> = batch
            .iter()
            .filter_map(|u| match u {
                Unit::File { item, read: None } => Some(ReadRequest {
                    path: item.path.clone(),
                    max_bytes: self.max_file_bytes,
                }),
                _ => None,
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        let mut answers = self.source.read(requests).await.into_iter();
        let mut settled = Vec::with_capacity(batch.len());
        for (unit, fresh) in batch.drain(..).zip(fresh) {
            let Unit::File { item, read } = unit else {
                settled.push(unit);
                continue;
            };
            let read = if fresh {
                answers.next().unwrap_or(Err(ReadError::Io))
            } else {
                // Read in an earlier round and put back because the batch was full.
                match read {
                    Some(read) => read,
                    None => Err(ReadError::Io),
                }
            };
            match read {
                Ok(read) => {
                    if fresh && read.hash != item.leaf.hash {
                        late.push(LateEdit::Put(
                            item.path.clone(),
                            FileLeaf::new(read.hash, read.stat),
                        ));
                    }
                    settled.push(Unit::File {
                        item,
                        read: Some(Ok(read)),
                    });
                }
                Err(ReadError::NotFound) => {
                    late.push(LateEdit::Remove(item.path.clone()));
                    settled.push(Unit::Removed(item.path));
                }
                Err(ReadError::TooLarge) => push_unit_skip(&mut settled, item.path, SkipReason::TooLarge),
                Err(error @ (ReadError::NotRegular | ReadError::Io)) => {
                    warn!(
                        path = item.path.as_str(),
                        ?error,
                        "a file could not be read for a delta"
                    );
                    push_unit_skip(&mut settled, item.path, SkipReason::Unreadable);
                }
            }
        }
        *batch = settled;
    }

    fn message(
        &self,
        batch: Vec<Unit>,
        base_root: Option<domain::ContentHash>,
        new_root: domain::ContentHash,
        more: bool,
        part: u32,
    ) -> ScanDelta {
        let observed_at = self.clock.now();
        let mut message = ScanDelta {
            seq: self.seq.next(),
            base_root,
            new_root,
            entries: Vec::new(),
            removed: Vec::new(),
            skipped: Vec::new(),
            during_job: self.during_job.cloned(),
            more,
            part,
            // A delta built from the tree never carries a gap; only the spool's replay does (T9).
            gap: None,
        };
        for unit in batch {
            match unit {
                Unit::Removed(path) => message.removed.push(path),
                Unit::Skipped(entry) => message.skipped.push(entry),
                Unit::Denied(item) => message.entries.push(ScanEntry {
                    path: item.path,
                    hash: item.leaf.hash,
                    size: item.leaf.stat.size,
                    mtime: Timestamp::from_unix_millis(item.leaf.stat.mtime.unix_millis()),
                    observed_at,
                    denied: true,
                    bytes: None,
                }),
                Unit::File {
                    item,
                    read: Some(Ok(read)),
                } => message.entries.push(ScanEntry {
                    path: item.path,
                    hash: read.hash,
                    size: read.bytes.len() as u64,
                    mtime: Timestamp::from_unix_millis(read.stat.mtime.unix_millis()),
                    observed_at,
                    denied: false,
                    bytes: Some(read.bytes),
                }),
                // `read_batch` settles every failed read, and every file in a batch has been read.
                Unit::File { .. } => {}
            }
        }
        message
    }
}

#[derive(Debug)]
enum LateEdit {
    Put(NfsPath, FileLeaf),
    Remove(NfsPath),
}

#[derive(Default)]
struct Plan {
    files: Vec<Item>,
    denied: Vec<Item>,
    removed: Vec<NfsPath>,
    skipped: Vec<SkippedEntry>,
}

/// Add a skipped entry. The reasons are short ASCII constants, so the text always parses; if it ever did not, the entry
/// would be left out rather than the process stopped.
fn push_skip(into: &mut Vec<SkippedEntry>, path: NfsPath, reason: SkipReason) {
    if let Ok(reason) = ShortText::parse(reason.as_str()) {
        into.push(SkippedEntry { path, reason });
    }
}

fn push_unit_skip(into: &mut Vec<Unit>, path: NfsPath, reason: SkipReason) {
    let mut one = Vec::with_capacity(1);
    push_skip(&mut one, path, reason);
    into.extend(one.into_iter().map(Unit::Skipped));
}

/// The path as the wire takes it, or `None` (and a log line) for bytes that are not one. Tree paths are valid by
/// construction; this is the check at the last step before the message.
fn wire_path(bytes: &[u8]) -> Option<NfsPath> {
    let parsed = std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| NfsPath::parse(s).ok());
    if parsed.is_none() {
        warn!(path = %String::from_utf8_lossy(bytes), "a path is not valid on the wire; left out of the delta");
    }
    parsed
}

/// Take units off the front of the queue while one message can hold them. A unit that alone is too big for a message
/// still goes out, alone: the limits on a single file are enforced where the files are chosen.
fn take_batch(queue: &mut VecDeque<Unit>) -> Vec<Unit> {
    let mut batch = Vec::new();
    let (mut count, mut wire, mut payload) = (0_usize, 0_usize, 0_usize);
    while let Some(unit) = queue.front() {
        let (cost, bytes) = unit.cost();
        let fits = count < ScanDelta::MAX_ENTRIES
            && wire + cost <= MAX_WIRE_ESTIMATE
            && payload + bytes <= ScanDelta::MAX_BYTES;
        if !fits && !batch.is_empty() {
            break;
        }
        count += 1;
        wire += cost;
        payload += bytes;
        if let Some(unit) = queue.pop_front() {
            batch.push(unit);
        }
    }
    batch
}

/// After the reads, a file that grew may push the batch over the byte limit. Put the last files back at the front of
/// the queue, keeping what was already read, until it fits (one file always stays).
fn give_back_overflow(batch: &mut Vec<Unit>, queue: &mut VecDeque<Unit>) {
    loop {
        let (wire, payload) = batch
            .iter()
            .map(Unit::cost)
            .fold((0, 0), |(w, p), (cw, cp)| (w + cw, p + cp));
        let files = batch.iter().filter(|u| matches!(u, Unit::File { .. })).count();
        if (wire <= MAX_WIRE_ESTIMATE && payload <= ScanDelta::MAX_BYTES) || files <= 1 {
            return;
        }
        let Some(at) = batch.iter().rposition(|u| matches!(u, Unit::File { .. })) else {
            return;
        };
        queue.push_front(batch.remove(at));
    }
}
