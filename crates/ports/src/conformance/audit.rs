use async_trait::async_trait;
use domain::{ContentHash, Timestamp};

use super::sample::{hash, nfs, swimlane, user_id};
use crate::{
    AuditAction, AuditActor, AuditEntry, AuditError, AuditEvent, AuditLog, AuditVia, TxFactory, genesis_hash,
    verify_chain,
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
}
