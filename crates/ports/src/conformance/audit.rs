use async_trait::async_trait;
use domain::{ContentHash, Timestamp};

use super::sample::{audit_events_totalling, hash, nfs, storable_worst_case_audit_event, swimlane, user_id};
use crate::{
    AuditAction, AuditActor, AuditEntry, AuditError, AuditEvent, AuditLog, AuditVia, MAX_AUDIT_BATCH,
    MAX_AUDIT_BATCH_BYTES, TxFactory, canonical_json, chain_hash, genesis_hash, verify_chain,
};

/// What the audit conformance suite needs to see.
#[async_trait]
pub trait AuditProbe: AuditLog {
    /// The committed chain, from the first entry, in chain order.
    async fn committed(&self) -> Vec<AuditEntry>;
}

fn event(n: i64, action: AuditAction) -> AuditEvent {
    let mut e = AuditEvent::new(
        Timestamp::from_unix_millis(1_700_000_000_000 + n),
        AuditActor::User { user: user_id(1) },
        action,
        AuditVia::Ui,
    );
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(nfs("a/b.yml"));
    e.hash_before = Some(hash(b"before"));
    e.hash_after = Some(hash(b"after"));
    e
}

/// - An event exists only if its transaction commits; a rolled-back one leaves no trace.
/// - Entries form the hash chain from the genesis hash (`verify_chain`), in commit order, with increasing ids.
/// - Several events in one transaction keep their order.
/// - An oversized diff is `TooLarge` and records nothing.
/// - `record_many` ([`audit_log_batch`]) behaves like `record` called once per event.
pub async fn audit_log<A: AuditProbe + ?Sized>(a: &A, txf: &dyn TxFactory) {
    let start = a.committed().await;
    assert!(verify_chain(&start).is_ok(), "chain before the test");
    let base = start.len();
    let prev = start.last().map_or_else(genesis_hash, |e| e.hash);

    let mut tx = txf.begin().await.unwrap();
    let id1 = a
        .record(&mut tx.tx(), event(1, AuditAction::FileEdited))
        .await
        .unwrap();
    assert_eq!(a.committed().await.len(), base, "not visible before commit");
    tx.commit().await.unwrap();

    let mut tx = txf.begin().await.unwrap();
    let id2 = a
        .record(&mut tx.tx(), event(2, AuditAction::FileDeleted))
        .await
        .unwrap();
    let id3 = a
        .record(&mut tx.tx(), event(3, AuditAction::PrRaised))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(id1 < id2 && id2 < id3, "ids increase");

    let mut tx = txf.begin().await.unwrap();
    a.record(&mut tx.tx(), event(4, AuditAction::RoleChanged))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let mut big = event(5, AuditAction::FileEdited);
    big.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(a.record(&mut tx.tx(), big).await, Err(AuditError::TooLarge));
    tx.commit().await.unwrap();

    let chain = a.committed().await;
    assert_eq!(
        chain.len(),
        base + 3,
        "three committed events, nothing from the rollback or the oversized one"
    );
    assert!(verify_chain(&chain).is_ok(), "the chain verifies end to end");
    let ours = &chain[base..];
    assert_eq!(ours[0].prev_hash, prev, "continues the existing chain");
    assert_eq!(
        ours.iter().map(|e| e.event.action).collect::<Vec<_>>(),
        [
            AuditAction::FileEdited,
            AuditAction::FileDeleted,
            AuditAction::PrRaised
        ]
    );
    assert_eq!(ours.iter().map(|e| e.id).collect::<Vec<_>>(), [id1, id2, id3]);
    let hashes: Vec<ContentHash> = ours.iter().map(|e| e.hash).collect();
    assert!(hashes.windows(2).all(|w| w[0] != w[1]));

    audit_log_batch(a, txf).await;
}

/// The chain that `events` must produce when they follow `prev`: what `n` calls of `record` write, from the one definition of
/// the link ([`canonical_json`] and [`chain_hash`]).
fn expected_links(prev: ContentHash, events: &[AuditEvent]) -> Vec<(ContentHash, ContentHash)> {
    let mut prev = prev;
    events
        .iter()
        .map(|e| {
            let hash = chain_hash(&prev, &canonical_json(e).unwrap());
            let link = (prev, hash);
            prev = hash;
            link
        })
        .collect()
}

/// `n` distinct events, starting at `from` (the time differs, so every hash does).
fn events(from: i64, n: usize) -> Vec<AuditEvent> {
    const ACTIONS: [AuditAction; 4] = [
        AuditAction::FileEdited,
        AuditAction::FileChangeDetected,
        AuditAction::BaselineAdopted,
        AuditAction::SettingsChanged,
    ];
    (0..n)
        .map(|i| event(from + i as i64, ACTIONS[i % ACTIONS.len()]))
        .collect()
}

/// The entries after the first `base`, which must be exactly `events` and chain from `prev` as `record` would write them.
fn assert_written(chain: &[AuditEntry], base: usize, prev: ContentHash, events: &[AuditEvent], what: &str) {
    assert_eq!(chain.len(), base + events.len(), "{what}: how many entries");
    let ours = &chain[base..];
    assert_eq!(
        ours.iter().map(|e| &e.event).collect::<Vec<_>>(),
        events.iter().collect::<Vec<_>>(),
        "{what}: the events, in input order"
    );
    let links = expected_links(prev, events);
    for (i, (entry, (p, h))) in ours.iter().zip(&links).enumerate() {
        assert_eq!(entry.prev_hash, *p, "{what}: entry {i} links to its predecessor");
        assert_eq!(entry.hash, *h, "{what}: entry {i} has the chain hash");
    }
    assert!(
        ours.windows(2).all(|w| w[0].id < w[1].id),
        "{what}: ids increase along the chain"
    );
    assert!(
        verify_chain(chain).is_ok(),
        "{what}: the chain verifies end to end"
    );
}

/// `AuditLog::record_many`, against `record`. Every implementation must pass it, whether it batches or keeps the default.
///
/// - `n` events in one call write exactly the chain that `n` calls of `record` write (sequence, links and hashes), and the ids
///   come back in input order.
/// - Calls of `record` and `record_many` interleave, in one transaction and across transactions, into one valid chain.
/// - The batch belongs to the transaction: invisible before the commit, and a rollback leaves no event and no chain advance.
/// - An event that cannot be stored is reported before anything is recorded: nothing of the batch is written, the transaction
///   stays usable, and committing it keeps nothing of the failed batch.
/// - An empty batch is a no-op; [`MAX_AUDIT_BATCH`] events are accepted and one more is `BatchTooLarge`, recording nothing.
/// - The byte budget: events whose canonical JSON adds up to exactly [`MAX_AUDIT_BATCH_BYTES`] are accepted, one byte more is
///   `BatchTooLarge` (also with few events: the budget counts the canonical bytes, in which a control character is six, and not
///   the characters), nothing of a refused batch is recorded, and the largest event that can exist is accepted alone, by
///   `record_many` and by `record`.
pub async fn audit_log_batch<A: AuditProbe + ?Sized>(a: &A, txf: &dyn TxFactory) {
    let head = |chain: &[AuditEntry]| chain.last().map_or_else(genesis_hash, |e| e.hash);
    let start = a.committed().await;
    assert!(verify_chain(&start).is_ok(), "chain before the batch tests");

    // Same chain as `record`. A batch first.
    let batch = events(100, 5);
    let (base, prev) = (start.len(), head(&start));
    let mut tx = txf.begin().await.unwrap();
    let ids = a.record_many(&mut tx.tx(), batch.clone()).await.unwrap();
    assert_eq!(ids.len(), batch.len(), "one id per event");
    assert_eq!(a.committed().await.len(), base, "not visible before commit");
    tx.commit().await.unwrap();
    let chain = a.committed().await;
    assert_written(&chain, base, prev, &batch, "record_many");
    assert_eq!(
        chain[base..].iter().map(|e| e.id).collect::<Vec<_>>(),
        ids,
        "ids come back in input order, and are the ids of the entries"
    );

    // The same five events through `record`, one call each, follow the same rule from the new head.
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    let mut single_ids = Vec::new();
    for e in &batch {
        single_ids.push(a.record(&mut tx.tx(), e.clone()).await.unwrap());
    }
    tx.commit().await.unwrap();
    let chain = a.committed().await;
    assert_written(&chain, base, prev, &batch, "record x5");
    assert_eq!(chain[base..].iter().map(|e| e.id).collect::<Vec<_>>(), single_ids);

    // Interleaved in one transaction: record, many, record, many, record.
    let (base, prev) = (chain.len(), head(&chain));
    let all = events(200, 8);
    let mut tx = txf.begin().await.unwrap();
    let mut got = vec![a.record(&mut tx.tx(), all[0].clone()).await.unwrap()];
    got.extend(a.record_many(&mut tx.tx(), all[1..4].to_vec()).await.unwrap());
    got.push(a.record(&mut tx.tx(), all[4].clone()).await.unwrap());
    got.extend(a.record_many(&mut tx.tx(), all[5..7].to_vec()).await.unwrap());
    got.push(a.record(&mut tx.tx(), all[7].clone()).await.unwrap());
    tx.commit().await.unwrap();
    let chain = a.committed().await;
    assert_written(&chain, base, prev, &all, "interleaved in one transaction");
    assert_eq!(chain[base..].iter().map(|e| e.id).collect::<Vec<_>>(), got);

    // Interleaved across transactions.
    let (base, prev) = (chain.len(), head(&chain));
    let all = events(300, 6);
    let mut tx = txf.begin().await.unwrap();
    a.record_many(&mut tx.tx(), all[0..2].to_vec()).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = txf.begin().await.unwrap();
    a.record(&mut tx.tx(), all[2].clone()).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = txf.begin().await.unwrap();
    a.record_many(&mut tx.tx(), all[3..6].to_vec()).await.unwrap();
    tx.commit().await.unwrap();
    let chain = a.committed().await;
    assert_written(&chain, base, prev, &all, "interleaved across transactions");

    // A rollback: no event, and no chain advance (the next event chains to the old head).
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    a.record_many(&mut tx.tx(), events(400, 4)).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        a.committed().await.len(),
        base,
        "a rolled-back batch leaves nothing"
    );
    let next = events(410, 3);
    let mut tx = txf.begin().await.unwrap();
    a.record_many(&mut tx.tx(), next.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &next,
        "after a rolled-back batch",
    );

    // An event that cannot be stored, in the middle: nothing of the batch is written and the transaction is still usable.
    let (chain, ok_marker) = (a.committed().await, events(420, 1));
    let (base, prev) = (chain.len(), head(&chain));
    let mut bad = events(430, 4);
    bad[2].diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(
        a.record_many(&mut tx.tx(), bad).await,
        Err(AuditError::TooLarge),
        "the bad event is named by the same error as `record`"
    );
    a.record_many(&mut tx.tx(), ok_marker.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &ok_marker,
        "after a batch refused for one event (only the later marker)",
    );

    // Empty: a successful no-op.
    let (chain, marker) = (a.committed().await, events(440, 1));
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(a.record_many(&mut tx.tx(), Vec::new()).await, Ok(Vec::new()));
    a.record_many(&mut tx.tx(), marker.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(&a.committed().await, base, prev, &marker, "after an empty batch");

    // The limit: `MAX_AUDIT_BATCH` is accepted, one more is refused whole.
    let (chain, marker) = (a.committed().await, events(450, 1));
    let (base, prev) = (chain.len(), head(&chain));
    let too_many = events(1_000, MAX_AUDIT_BATCH + 1);
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(
        a.record_many(&mut tx.tx(), too_many).await,
        Err(AuditError::BatchTooLarge),
        "refused, not truncated"
    );
    a.record_many(&mut tx.tx(), marker.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &marker,
        "after an oversized batch",
    );

    let (chain, full) = (a.committed().await, events(2_000, MAX_AUDIT_BATCH));
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    let ids = a.record_many(&mut tx.tx(), full.clone()).await.unwrap();
    assert_eq!(ids.len(), MAX_AUDIT_BATCH);
    tx.commit().await.unwrap();
    let chain = a.committed().await;
    assert_written(&chain, base, prev, &full, "a batch of exactly the limit");
    assert_eq!(chain[base..].iter().map(|e| e.id).collect::<Vec<_>>(), ids);

    // The byte budget. Three events of the largest size are 3 events and 1.2 MB: under the count limit and over the budget, and
    // their diffs are 192 KiB of characters, which is what a count of characters would see.
    let (chain, marker) = (a.committed().await, events(460, 1));
    let (base, prev) = (chain.len(), head(&chain));
    let largest: Vec<AuditEvent> = (0..3).map(storable_worst_case_audit_event).collect();
    let mut tx = txf.begin().await.unwrap();
    assert_eq!(
        a.record_many(&mut tx.tx(), largest.clone()).await,
        Err(AuditError::BatchTooLarge),
        "over the byte budget with three events: refused, in canonical bytes and not in characters"
    );
    let one_more = audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES + 1);
    assert_eq!(
        a.record_many(&mut tx.tx(), one_more).await,
        Err(AuditError::BatchTooLarge),
        "one byte over the byte budget"
    );
    a.record_many(&mut tx.tx(), marker.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &marker,
        "after batches over the byte budget (only the later marker)",
    );

    // The largest event that can exist is accepted alone, by both methods: the budget is at least one such event.
    let (chain, one) = (a.committed().await, vec![storable_worst_case_audit_event(7)]);
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    a.record_many(&mut tx.tx(), one.clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &one,
        "the largest event, by record_many",
    );
    let (chain, one) = (a.committed().await, vec![storable_worst_case_audit_event(8)]);
    let (base, prev) = (chain.len(), head(&chain));
    let mut tx = txf.begin().await.unwrap();
    a.record(&mut tx.tx(), one[0].clone()).await.unwrap();
    tx.commit().await.unwrap();
    assert_written(
        &a.committed().await,
        base,
        prev,
        &one,
        "the largest event, by record",
    );

    // What fits: two of the largest, and the budget to the byte, in three events and in as many as the count allows.
    let at_the_edge: [(Vec<AuditEvent>, &str); 3] = [
        (
            (10..12).map(storable_worst_case_audit_event).collect(),
            "two of the largest",
        ),
        (
            audit_events_totalling(3, MAX_AUDIT_BATCH_BYTES),
            "exactly the byte budget, in three events",
        ),
        (
            audit_events_totalling(MAX_AUDIT_BATCH, MAX_AUDIT_BATCH_BYTES),
            "exactly the byte budget, in as many events as the count allows",
        ),
    ];
    for (batch, what) in at_the_edge {
        let chain = a.committed().await;
        let (base, prev) = (chain.len(), head(&chain));
        let mut tx = txf.begin().await.unwrap();
        let ids = a.record_many(&mut tx.tx(), batch.clone()).await.unwrap();
        assert_eq!(ids.len(), batch.len(), "{what}");
        tx.commit().await.unwrap();
        assert_written(&a.committed().await, base, prev, &batch, what);
    }
}
