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

/// An entry for another swimlane and another user.
fn entry_of(sw: &str, user: u32, path: &str, content: &[u8], request: &str) -> ToolWriteEntry {
    ToolWriteEntry {
        swimlane: swimlane(sw),
        user: user_id(user),
        ..entry(path, content, request)
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

/// The newest registrations are exactly `expected`, oldest of them first. A bounded probe may have dropped the oldest,
/// so this never counts on the history being complete.
fn ends_with(all: &[ToolWriteEntry], expected: &[ToolWriteEntry]) -> bool {
    all.ends_with(expected)
}

/// How many times `e` is registered in what the probe shows.
fn times(all: &[ToolWriteEntry], e: &ToolWriteEntry) -> usize {
    all.iter().filter(|x| *x == e).count()
}

/// - A registration exists only if its transaction commits; a rolled-back one leaves nothing behind.
/// - Registrations keep their order, and carry the swimlane, path, hash, user and request they were made with.
/// - Registering the same (swimlane, path, hash, request) again is a no-op, in the same transaction or a later one.
/// - The same hash at another path, in another swimlane, or from another request, is another registration.
pub async fn tool_writes<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory) {
    let first = entry("svc/app.yml", b"x: 2\n", "req-tw-1");
    // Another swimlane and another user, with the path and hash of `first`: a registration of its own.
    let second = entry_of("sit2", 2, "svc/app.yml", b"x: 2\n", "req-tw-2");
    let gone = entry("svc/gone.yml", b"z: 0\n", "req-tw-3");

    let before = t.registered().await;
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &second).await.unwrap();
    assert_eq!(t.registered().await, before, "not visible before commit");
    tx.commit().await.unwrap();

    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &gone).await.unwrap();
    tx.rollback().await.unwrap();

    let after = t.registered().await;
    assert_eq!(times(&after, &gone), 0, "the rollback left nothing behind");
    assert!(
        ends_with(&after, &[first.clone(), second.clone()]),
        "in order, as made; the same path and hash in another swimlane, from another user, is another registration: {after:?}"
    );

    // A repeat is a no-op, in a later transaction and inside one.
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    tx.commit().await.unwrap();
    let again = t.registered().await;
    assert_eq!(times(&again, &first), 1, "a repeat registers nothing");
    assert!(ends_with(&again, &[first.clone(), second.clone()]));

    // A registration made twice inside one open transaction is one registration once it commits.
    let twice = entry_of("sit2", 3, "svc/twice.yml", b"t: 1\n", "req-tw-4");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &twice).await.unwrap();
    register(t, &mut tx.tx(), &twice).await.unwrap();
    tx.commit().await.unwrap();
    let all = t.registered().await;
    assert_eq!(times(&all, &twice), 1, "twice in one transaction is once");
    assert!(ends_with(&all, &[twice.clone()]));

    // Another path or another request is a new registration.
    let elsewhere = entry("svc/copy.yml", b"x: 2\n", "req-tw-1");
    let retried_elsewhere = entry("svc/app.yml", b"x: 2\n", "req-tw-9");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &elsewhere).await.unwrap();
    register(t, &mut tx.tx(), &retried_elsewhere).await.unwrap();
    tx.commit().await.unwrap();
    let after = t.registered().await;
    assert!(ends_with(&after, &[elsewhere, retried_elsewhere]));
}
