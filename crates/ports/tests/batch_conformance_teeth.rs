//! The batch conformance suites fail for an implementation that gets a batch wrong: the order, the limit, the all-or-nothing check.
//! Each wrapper below is a fake with one deliberate defect in `record_many` or `publish_many_in_tx`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use domain::{AuditId, DomainEvent};
use futures::stream::BoxStream;
use ports::conformance::{self, AuditProbe};
use ports::fakes::{FakeAuditLog, FakeEventBus, FakeTxFactory};
use ports::{
    AuditEntry, AuditError, AuditEvent, AuditLog, BusError, EventBus, EventFilter, MAX_AUDIT_BATCH,
    MAX_AUDIT_BATCH_BYTES, MAX_EVENT_BATCH, Tx,
};

#[derive(Clone, Copy)]
enum AuditDefect {
    /// Records the events back to front.
    Reversed,
    /// Takes the first `MAX_AUDIT_BATCH` events of a bigger batch and says nothing.
    Truncates,
    /// Records the events one by one without checking them first, so a bad event leaves the ones before it behind.
    ChecksLate,
    /// Everything but the byte budget: it checks the count and each event, and takes any number of bytes.
    IgnoresTheByteBudget,
    /// Counts the characters of the diffs and not the bytes of the canonical JSON, where a control character is six.
    CountsCharacters,
}

struct BrokenAudit {
    inner: FakeAuditLog,
    defect: AuditDefect,
}

#[async_trait]
impl AuditLog for BrokenAudit {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        self.inner.record(tx, e).await
    }

    async fn record_many(
        &self,
        tx: &mut Tx<'_>,
        mut events: Vec<AuditEvent>,
    ) -> Result<Vec<AuditId>, AuditError> {
        match self.defect {
            AuditDefect::Reversed => {
                events.reverse();
                let mut ids = Vec::new();
                for e in events {
                    ids.push(self.inner.record(tx, e).await?);
                }
                ids.reverse();
                Ok(ids)
            }
            AuditDefect::Truncates => {
                // Right about everything else: an event that `record` refuses is refused before any is recorded.
                if events.iter().any(|e| {
                    e.diff
                        .as_ref()
                        .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
                }) {
                    return Err(AuditError::TooLarge);
                }
                events.truncate(MAX_AUDIT_BATCH);
                let mut ids = Vec::new();
                for e in events {
                    ids.push(self.inner.record(tx, e).await?);
                }
                Ok(ids)
            }
            AuditDefect::ChecksLate => {
                let mut ids = Vec::new();
                for e in events {
                    ids.push(self.inner.record(tx, e).await?);
                }
                Ok(ids)
            }
            AuditDefect::IgnoresTheByteBudget | AuditDefect::CountsCharacters => {
                if events.len() > MAX_AUDIT_BATCH {
                    return Err(AuditError::BatchTooLarge);
                }
                if events.iter().any(|e| {
                    e.diff
                        .as_ref()
                        .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
                }) {
                    return Err(AuditError::TooLarge);
                }
                if matches!(self.defect, AuditDefect::CountsCharacters) {
                    let characters: usize = events
                        .iter()
                        .map(|e| 300 + e.diff.as_ref().map_or(0, String::len))
                        .sum();
                    if characters > MAX_AUDIT_BATCH_BYTES {
                        return Err(AuditError::BatchTooLarge);
                    }
                }
                let mut ids = Vec::new();
                for e in events {
                    ids.push(self.inner.record(tx, e).await?);
                }
                Ok(ids)
            }
        }
    }
}

#[async_trait]
impl AuditProbe for BrokenAudit {
    async fn committed(&self) -> Vec<AuditEntry> {
        self.inner.entries()
    }
}

async fn run_audit(defect: AuditDefect) {
    let log = BrokenAudit {
        inner: FakeAuditLog::new(),
        defect,
    };
    conformance::audit_log_batch(&log, &FakeTxFactory::new()).await;
}

#[tokio::test(start_paused = true)]
async fn the_audit_suite_passes_for_an_implementation_without_a_defect() {
    // The control: the same wrapper, with the defaults.
    struct Plain(FakeAuditLog);
    #[async_trait]
    impl AuditLog for Plain {
        async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
            self.0.record(tx, e).await
        }
    }
    #[async_trait]
    impl AuditProbe for Plain {
        async fn committed(&self) -> Vec<AuditEntry> {
            self.0.entries()
        }
    }
    conformance::audit_log_batch(&Plain(FakeAuditLog::new()), &FakeTxFactory::new()).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "input order")]
async fn the_audit_suite_catches_a_batch_recorded_out_of_order() {
    run_audit(AuditDefect::Reversed).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "refused, not truncated")]
async fn the_audit_suite_catches_a_batch_that_is_truncated_instead_of_refused() {
    run_audit(AuditDefect::Truncates).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "after a batch refused for one event")]
async fn the_audit_suite_catches_a_batch_that_leaves_events_behind_when_one_is_refused() {
    run_audit(AuditDefect::ChecksLate).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "over the byte budget with three events")]
async fn the_audit_suite_catches_a_batch_that_ignores_the_byte_budget() {
    run_audit(AuditDefect::IgnoresTheByteBudget).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "over the byte budget with three events")]
async fn the_audit_suite_catches_a_batch_that_counts_characters_and_not_canonical_bytes() {
    run_audit(AuditDefect::CountsCharacters).await;
}

#[derive(Clone, Copy)]
enum BusDefect {
    Reversed,
    Truncates,
    /// Fans out at once, in a transaction that may still roll back.
    IgnoresTheTransaction,
    /// Right for the small batches the suite tries first, and for a big one it silently publishes every other event.
    DropsEveryOtherOfABigBatch,
    /// And for a big one it silently publishes only the first 60 %.
    DropsTheTailOfABigBatch,
    /// And for a big one it publishes nothing and says nothing.
    DropsAllOfABigBatch,
}

struct BrokenBus {
    inner: FakeEventBus,
    defect: BusDefect,
}

#[async_trait]
impl EventBus for BrokenBus {
    async fn publish(&self, e: DomainEvent) -> Result<(), BusError> {
        self.inner.publish(e).await
    }

    async fn publish_in_tx(&self, tx: &mut Tx<'_>, e: DomainEvent) -> Result<(), BusError> {
        self.inner.publish_in_tx(tx, e).await
    }

    async fn publish_many_in_tx(
        &self,
        tx: &mut Tx<'_>,
        mut events: Vec<DomainEvent>,
    ) -> Result<(), BusError> {
        match self.defect {
            BusDefect::Reversed => {
                events.reverse();
                for e in events {
                    self.inner.publish_in_tx(tx, e).await?;
                }
            }
            BusDefect::Truncates => {
                events.truncate(MAX_EVENT_BATCH);
                for e in events {
                    self.inner.publish_in_tx(tx, e).await?;
                }
            }
            BusDefect::IgnoresTheTransaction => {
                for e in events {
                    self.inner.publish(e).await?;
                }
            }
            BusDefect::DropsEveryOtherOfABigBatch
            | BusDefect::DropsTheTailOfABigBatch
            | BusDefect::DropsAllOfABigBatch => {
                if events.len() > MAX_EVENT_BATCH {
                    return Err(BusError::BatchTooLarge);
                }
                let big = events.len() > 50;
                for (i, e) in events.iter().cloned().enumerate() {
                    let keep = !big
                        || match self.defect {
                            BusDefect::DropsEveryOtherOfABigBatch => i % 2 == 0,
                            BusDefect::DropsTheTailOfABigBatch => i < events.len() * 6 / 10,
                            _ => false,
                        };
                    if keep {
                        self.inner.publish_in_tx(tx, e).await?;
                    }
                }
            }
        }
        Ok(())
    }

    fn subscribe(&self, f: EventFilter) -> BoxStream<'static, DomainEvent> {
        self.inner.subscribe(f)
    }
}

async fn run_bus(defect: BusDefect) {
    let bus = BrokenBus {
        inner: FakeEventBus::new(8),
        defect,
    };
    conformance::event_bus_batch(&bus, &FakeTxFactory::new()).await;
}

#[tokio::test(start_paused = true)]
async fn the_bus_suite_passes_for_a_bus_without_a_defect() {
    conformance::event_bus_batch(&FakeEventBus::new(8), &FakeTxFactory::new()).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "input order")]
async fn the_bus_suite_catches_a_batch_published_out_of_order() {
    run_bus(BusDefect::Reversed).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "refused, not truncated")]
async fn the_bus_suite_catches_a_batch_that_is_truncated_instead_of_refused() {
    run_bus(BusDefect::Truncates).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "published inside an open transaction")]
async fn the_bus_suite_catches_a_batch_that_does_not_wait_for_the_commit() {
    run_bus(BusDefect::IgnoresTheTransaction).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "silent loss or disorder inside a run")]
async fn the_bus_suite_catches_a_big_batch_that_silently_drops_events_inside() {
    run_bus(BusDefect::DropsEveryOtherOfABigBatch).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "silent loss at the end")]
async fn the_bus_suite_catches_a_big_batch_that_silently_loses_its_tail() {
    run_bus(BusDefect::DropsTheTailOfABigBatch).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "silent loss: without a Resync")]
async fn the_bus_suite_catches_a_big_batch_that_publishes_nothing_and_says_nothing() {
    run_bus(BusDefect::DropsAllOfABigBatch).await;
}
