//! `DeltaAssembler`: one message of a scan delta (D47, D63, D74, P1, P15).
//!
//! 1. **Validate** everything before anything is stored ([`super::validate`]).
//! 2. **Store the blobs** of the message (`BlobStore::put` is idempotent), a few at a time, each with a timeout. The content is in
//!    `blobs` before the message is acknowledged, whatever happens to the rest.
//! 3. A message with `more` is **staged** (`delta_staging`, [`super::staging`]) and acknowledged: the whole delta applies only
//!    with its last part, and a stream that ends first applies nothing.
//! 4. The last message **applies** the staged parts and itself ([`super::apply`]).
//!
//! Parts of one logical delta share the pair of roots (`base_root`, `new_root`); that pair is what groups them. The hub does not
//! trust the numbering of parts to *group* them (a part sent again replaces itself, by `seq`), but it does use it to refuse a
//! delta that is missing a part: a last message numbered `n` needs parts `0` to `n - 1` ([`parts_missing`]).

use std::collections::{BTreeSet, HashSet};

use domain::{ContentHash, ScanDelta, SwimlaneId};
use futures::StreamExt;
use ports::{AgentIdentity, content_hash};

use super::Inner;
use super::apply::{ApplyReport, Logical};
use super::staging::{self, Staged};
use super::validate::{self, Bounds, INLINE_VALIDATION_BYTES, Rules, Validated};
use crate::error::RegistryError;

/// Blobs stored at once. Each is one database round trip, so a delta of many small files needs some overlap to keep up with the
/// spool replay rate (P15).
const BLOB_CONCURRENCY: usize = 16;

/// Progress of an apply, for deciding what a failure leaves behind.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    pub chunks: u64,
}

/// What became of a message.
#[derive(Debug)]
pub(crate) enum Accepted {
    /// A part with `more`: staged, and acknowledged.
    Staged,
    /// The last part: the whole logical delta is applied.
    Applied(ApplyReport),
}

/// Whether a last part numbered `last_part` is missing a part before it: it needs `0` to `last_part - 1`, in whatever order they
/// came. A staged part numbered at or above `last_part` is tolerated: `ports::conformance::report_sink` ends a delta with a part
/// numbered 0 after one numbered 0 that had `more`, and a number above cannot be told from a part sent again. The loop stops at
/// the first part that is not there, and `staged` is bounded by `max_delta_parts`, so a hostile part number costs nothing.
pub(crate) fn parts_missing(staged: &BTreeSet<u32>, last_part: u32) -> bool {
    (0..last_part).any(|p| !staged.contains(&p))
}

/// The state machine for multi-part scan deltas. Borrows the ingestion state; one value per message.
pub(crate) struct DeltaAssembler<'a> {
    inner: &'a Inner,
}

impl<'a> DeltaAssembler<'a> {
    pub(crate) fn new(inner: &'a Inner) -> Self {
        Self { inner }
    }

    pub(crate) async fn accept(&self, id: &AgentIdentity, d: ScanDelta) -> Result<Accepted, RegistryError> {
        let inner = self.inner;
        let swimlane = id.swimlane();
        if !inner.said_hello(swimlane) {
            return Err(RegistryError::HelloRequired);
        }
        // One delta at a time per swimlane on this replica; other replicas are held off by the database lock.
        let lock = inner.lock_for(swimlane);
        let _one_at_a_time = lock.lock().await;

        let v = self.validated(d).await?;
        inner.metrics.skipped(v.staged.skipped.len());
        self.store_blobs(&v).await?;

        let so_far = staging::totals(&inner.db, swimlane, v.base_root, v.new_root, v.seq).await?;
        let c = &inner.config;
        let bounds = Bounds {
            parts: c.max_delta_parts as u64,
            items: c.max_delta_entries as u64,
            bytes: c.max_delta_bytes,
        };
        if so_far.with(&v.staged, bounds).is_err() {
            // A delta over its bounds can never be applied: drop what is staged so that it cannot wait for the sweep.
            staging::clear(&inner.db, swimlane, v.base_root, v.new_root).await?;
            return Err(RegistryError::TooLarge);
        }

        if v.more {
            staging::stage(&inner.db, swimlane, &v, inner.clock.now().unix_millis()).await?;
            return Ok(Accepted::Staged);
        }
        self.complete(swimlane, v).await.map(Accepted::Applied)
    }

    /// Validate on the async thread for a small message, on a blocking one for a message with much content to hash (rule 4).
    async fn validated(&self, d: ScanDelta) -> Result<Validated, RegistryError> {
        let inner = self.inner;
        let now_ms = inner.clock.now().unix_millis();
        let max_skew_ms = i64::try_from(inner.config.max_clock_skew.as_millis()).unwrap_or(i64::MAX);
        let max_file_bytes = inner.config.max_file_bytes;
        if d.payload_bytes() <= INLINE_VALIDATION_BYTES {
            return validate::validate(
                d,
                &Rules {
                    deny: &inner.deny,
                    max_file_bytes,
                    now_ms,
                    max_skew_ms,
                },
            );
        }
        let deny = inner.deny.clone();
        tokio::task::spawn_blocking(move || {
            validate::validate(
                d,
                &Rules {
                    deny: &deny,
                    max_file_bytes,
                    now_ms,
                    max_skew_ms,
                },
            )
        })
        .await
        .map_err(|_| RegistryError::Invalid)?
    }

    /// Put the content of a message in the blob store, each blob once, each put with a timeout (rule 5).
    async fn store_blobs(&self, v: &Validated) -> Result<(), RegistryError> {
        let mut seen: HashSet<ContentHash> = HashSet::new();
        let mut todo = Vec::new();
        for (entry, content) in v.staged.entries.iter().filter(|e| e.has_content).zip(&v.contents) {
            if seen.insert(entry.hash) {
                todo.push((entry.hash, content.clone()));
            }
        }
        let blobs = &self.inner.blobs;
        let timeout = self.inner.config.blob_timeout;
        let results: Vec<Result<(), RegistryError>> = futures::stream::iter(todo)
            .map(|(hash, bytes)| async move {
                let Ok(stored) = tokio::time::timeout(timeout, blobs.put(bytes.clone())).await else {
                    tracing::warn!(op = "blob_put", why = "timed_out", "blob store call failed");
                    return Err(RegistryError::Timeout);
                };
                let stored = stored?;
                // The store addresses by SHA-256; the answer must be the hash that was validated, or something is badly wrong.
                if stored != hash || content_hash(&bytes) != hash {
                    return Err(RegistryError::Corrupt);
                }
                Ok(())
            })
            .buffer_unordered(BLOB_CONCURRENCY)
            .collect()
            .await;
        results.into_iter().collect()
    }

    /// The message that completes a logical delta: gather its staged parts and apply them all.
    async fn complete(&self, swimlane: &SwimlaneId, last: Validated) -> Result<ApplyReport, RegistryError> {
        let inner = self.inner;
        let staged: Vec<Staged> = staging::load(
            &inner.db,
            swimlane,
            last.base_root,
            last.new_root,
            inner.config.max_delta_parts,
        )
        .await?
        .into_iter()
        .filter(|s| s.seq != last.seq)
        .collect();

        // A last part numbered `n` needs the parts before it.
        let have: BTreeSet<u32> = staged.iter().map(|s| s.part).collect();
        if parts_missing(&have, last.part) {
            staging::clear(&inner.db, swimlane, last.base_root, last.new_root).await?;
            return Err(RegistryError::Invalid);
        }

        let mut seqs: Vec<u64> = staged.iter().map(|s| s.seq).collect();
        seqs.push(last.seq);
        let mut parts: Vec<_> = staged.into_iter().map(|s| s.data).collect();
        parts.push(last.staged);
        let logical = Logical {
            swimlane: swimlane.clone(),
            base_root: last.base_root,
            new_root: last.new_root,
            seqs,
            parts,
        };
        let mut progress = Progress::default();
        match inner.apply_logical(&logical, &mut progress).await {
            Ok(report) => Ok(report),
            Err(e) => {
                // Chunks that committed stay (they are idempotent and the agent sends the delta again), but the hub can no longer
                // vouch for its picture of the swimlane: ask for a full scan (A13).
                if progress.chunks > 0 {
                    inner.flag_full_scan(swimlane).await;
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn staged(parts: &[u32]) -> BTreeSet<u32> {
        parts.iter().copied().collect()
    }

    #[test]
    fn a_delta_of_one_part_needs_nothing_staged() {
        assert!(!parts_missing(&staged(&[]), 0));
    }

    #[test]
    fn a_last_part_needs_every_part_before_it() {
        assert!(parts_missing(&staged(&[]), 3), "nothing staged, three parts owed");
        assert!(parts_missing(&staged(&[0, 2]), 3), "part 1 never arrived");
        assert!(parts_missing(&staged(&[1, 2]), 3), "part 0 never arrived");
        assert!(!parts_missing(&staged(&[0, 1, 2]), 3));
    }

    #[test]
    fn the_order_the_parts_arrived_in_does_not_matter() {
        // The hub groups parts by their roots and numbers them by what they say, not by when they came.
        assert!(!parts_missing(&staged(&[2, 0, 1]), 3));
    }

    #[test]
    fn a_staged_part_at_or_above_the_last_one_is_tolerated() {
        // `ports::conformance::report_sink` sends a last part numbered 0 after a part numbered 0 that had `more`; both carry the
        // same roots, so they are one delta. A number above the last part's cannot be told from a part sent again.
        assert!(!parts_missing(&staged(&[0]), 0));
        assert!(!parts_missing(&staged(&[0, 1, 2]), 1));
    }

    #[test]
    fn a_wild_part_number_is_refused_quickly() {
        // A hostile agent names part 4 billion: the check stops at the first part that is not there.
        assert!(parts_missing(&staged(&[0, 1, 2]), u32::MAX));
        assert!(parts_missing(&staged(&[]), u32::MAX));
    }
}
