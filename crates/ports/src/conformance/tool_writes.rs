use async_trait::async_trait;

use super::sample::{hash, nfs, request_id, swimlane, user_id};
use crate::{ToolWriteEntry, ToolWrites, TxFactory};

/// What the tool-write conformance suite needs to see.
#[async_trait]
pub trait ToolWritesProbe: ToolWrites {
    /// The committed registrations, oldest first. A bounded probe may have dropped the oldest.
    async fn registered(&self) -> Vec<ToolWriteEntry>;
}

fn entry(path: &str, content: &[u8], request: &str) -> ToolWriteEntry {
    ToolWriteEntry {
        swimlane: swimlane("sit1"),
        path: nfs(path),
        new_hash: hash(content),
        user: user_id(1),
        request_id: request_id(request),
    }
}

async fn register<T: ToolWritesProbe + ?Sized>(
    t: &T,
    tx: &mut crate::Tx<'_>,
    e: &ToolWriteEntry,
) -> Result<(), crate::ToolWriteError> {
    t.register(tx, &e.swimlane, &e.path, &e.new_hash, &e.user, &e.request_id)
        .await
}

/// - A registration exists only if its transaction commits; a rolled-back one leaves nothing behind.
/// - Registrations keep their order, and carry the swimlane, path, hash, user and request they were made with.
/// - Registering the same (swimlane, path, hash, request) again is a no-op, in the same transaction or a later one.
/// - The same hash at another path, or from another request, is another registration.
pub async fn tool_writes<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory) {
    let base = t.registered().await.len();
    let first = entry("svc/app.yml", b"x: 2\n", "req-tw-1");
    let second = entry("svc/other.yml", b"y: 1\n", "req-tw-2");

    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &second).await.unwrap();
    assert_eq!(t.registered().await.len(), base, "not visible before commit");
    tx.commit().await.unwrap();

    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &entry("svc/gone.yml", b"z: 0\n", "req-tw-3"))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let after = t.registered().await;
    assert_eq!(after.len(), base + 2, "the rollback left nothing behind");
    assert_eq!(
        &after[base..],
        &[first.clone(), second.clone()],
        "in order, as made"
    );

    // A repeat is a no-op, in a later transaction and inside one.
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(t.registered().await.len(), base + 2, "a repeat registers nothing");

    // Another path or another request is a new registration.
    let elsewhere = entry("svc/copy.yml", b"x: 2\n", "req-tw-1");
    let retried_elsewhere = entry("svc/app.yml", b"x: 2\n", "req-tw-9");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &elsewhere).await.unwrap();
    register(t, &mut tx.tx(), &retried_elsewhere).await.unwrap();
    tx.commit().await.unwrap();
    let after = t.registered().await;
    assert_eq!(after.len(), base + 4);
    assert_eq!(&after[base + 2..], &[elsewhere, retried_elsewhere]);
}
