//! The tool-write conformance suites fail for an implementation that gets a delete registration wrong: dropped, registered
//! outside the transaction, not idempotent, mixed up with a hash registration, accepted from another store, not bounded.
//! Each wrapper below is a fake with one deliberate defect, and each transaction factory has one defect in its commit or its
//! rollback. The control passes, so a failure below is the defect and not the harness.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};
use ports::conformance::sample::hash;
use ports::conformance::{self, ToolWritesProbe};
use ports::fakes::{FakeToolWrites, FakeTxFactory};
use ports::{OwnedTx, ToolDeleteEntry, ToolWriteEntry, ToolWriteError, ToolWrites, Tx, TxError, TxFactory};

type Key = (SwimlaneId, NfsPath, RequestId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Defect {
    /// No defect: the wrapper is a plain fake.
    None,
    /// `register_delete` checks the transaction, says `Ok` and registers nothing, and the probe opts in. It is what a decorator
    /// that forgot to forward the method looks like, or an implementation that kept the default by mistake.
    DropsDeletes,
    /// `register_delete` registers at once, in a store of its own, and not in the caller's transaction.
    DeletesIgnoreTheTransaction,
    /// `register_delete` says `Ok` to a transaction of another store.
    DeletesAcceptAnyTransaction,
    /// `register` says `Ok` to a transaction of another store.
    HashesAcceptAnyTransaction,
    /// The repeat of a delete registration is registered again.
    RepeatedDeletesAreNew,
    /// A delete registration is also registered as a hash registration.
    DeleteIsAlsoAHash,
    /// Once a delete registration of a swimlane, path and request is made, a hash registration of them is not kept.
    DeleteShadowsTheHash,
    /// Once a hash registration of a swimlane, path and request is made, a delete registration of them is not kept.
    HashShadowsTheDelete,
    /// The same hash registration from another user is a second registration.
    SecondUserIsAnotherHash,
    /// The same delete registration from another user is a second registration.
    SecondUserIsAnotherDelete,
    /// The same hash registration from another user is a no-op that replaces the first user.
    LastUserWins,
    /// The same delete registration from another user is a no-op that replaces the first user.
    LastDeleteUserWins,
    /// A delete registration is kept without the hash of the file it deleted: it shows another hash.
    ForgetsTheDeletedHash,
    /// The key of a delete registration leaves out the deleted hash, so a delete of another version of the file is a repeat.
    DeleteKeyIgnoresTheHash,
}

struct Defective {
    inner: FakeToolWrites,
    /// What a defect registers a second time or outside the transaction.
    extra: FakeToolWrites,
    outside: Mutex<Vec<ToolDeleteEntry>>,
    seen_deletes: Mutex<Vec<Key>>,
    seen_hashes: Mutex<Vec<Key>>,
    /// The first user of each (kind, key), and the latest one: `true` is a delete registration.
    first_users: Mutex<Vec<(bool, Key, UserId)>>,
    latest_users: Mutex<Vec<(bool, Key, UserId)>>,
    defect: Defect,
}

impl Defective {
    fn new(defect: Defect) -> Self {
        Self {
            inner: FakeToolWrites::new(),
            extra: FakeToolWrites::new(),
            outside: Mutex::default(),
            seen_deletes: Mutex::default(),
            seen_hashes: Mutex::default(),
            first_users: Mutex::default(),
            latest_users: Mutex::default(),
            defect,
        }
    }

    /// Remember who registered `key`, and answer who did first (before this call), if anyone.
    fn note_user(&self, delete: bool, key: &Key, user: &UserId) -> Option<UserId> {
        let mut first = self.first_users.lock().unwrap();
        let before = first
            .iter()
            .find(|(d, k, _)| *d == delete && k == key)
            .map(|(_, _, u)| u.clone());
        if before.is_none() {
            first.push((delete, key.clone(), user.clone()));
        }
        let mut latest = self.latest_users.lock().unwrap();
        latest.retain(|(d, k, _)| !(*d == delete && k == key));
        latest.push((delete, key.clone(), user.clone()));
        before
    }

    fn latest_user(&self, delete: bool, key: &Key) -> Option<UserId> {
        self.latest_users
            .lock()
            .unwrap()
            .iter()
            .find(|(d, k, _)| *d == delete && k == key)
            .map(|(_, _, u)| u.clone())
    }
}

#[async_trait]
impl ToolWrites for Defective {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let key: Key = (swimlane.clone(), path.clone(), request_id.clone());
        match self.defect {
            Defect::HashesAcceptAnyTransaction if tx.mem().is_err() => return Ok(()),
            Defect::DeleteShadowsTheHash if self.seen_deletes.lock().unwrap().contains(&key) => {
                tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
                return Ok(());
            }
            _ => {}
        }
        self.seen_hashes.lock().unwrap().push(key.clone());
        let first_user = self.note_user(false, &key, user);
        self.inner
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await?;
        if self.defect == Defect::SecondUserIsAnotherHash && first_user.is_some_and(|u| u != *user) {
            self.extra
                .register(tx, swimlane, path, new_hash, user, request_id)
                .await?;
        }
        Ok(())
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let key: Key = (swimlane.clone(), path.clone(), request_id.clone());
        match self.defect {
            Defect::DropsDeletes => {
                tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
                return Ok(());
            }
            Defect::DeletesIgnoreTheTransaction => {
                tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
                let entry = ToolDeleteEntry {
                    swimlane: swimlane.clone(),
                    path: path.clone(),
                    deleted_hash: *deleted_hash,
                    user: user.clone(),
                    request_id: request_id.clone(),
                };
                let mut outside = self.outside.lock().unwrap();
                if !outside.contains(&entry) {
                    outside.push(entry);
                }
                return Ok(());
            }
            Defect::DeletesAcceptAnyTransaction if tx.mem().is_err() => return Ok(()),
            // A refused transaction registers nothing, and is not remembered.
            Defect::DeleteKeyIgnoresTheHash if tx.mem().is_err() => return Err(ToolWriteError::WrongTx),
            Defect::HashShadowsTheDelete if self.seen_hashes.lock().unwrap().contains(&key) => {
                tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
                return Ok(());
            }
            _ => {}
        }
        let first_time = {
            let mut seen = self.seen_deletes.lock().unwrap();
            let first_time = !seen.contains(&key);
            seen.push(key.clone());
            first_time
        };
        if self.defect == Defect::DeleteKeyIgnoresTheHash && !first_time {
            tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
            return Ok(());
        }
        let forgotten = hash(b"forgotten");
        let deleted_hash = if self.defect == Defect::ForgetsTheDeletedHash {
            &forgotten
        } else {
            deleted_hash
        };
        let first_user = self.note_user(true, &key, user);
        self.inner
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await?;
        if self.defect == Defect::SecondUserIsAnotherDelete && first_user.is_some_and(|u| u != *user) {
            self.extra
                .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
                .await?;
        }
        match self.defect {
            Defect::RepeatedDeletesAreNew if !first_time => {
                self.extra
                    .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
                    .await?;
            }
            Defect::DeleteIsAlsoAHash => {
                self.inner
                    .register(tx, swimlane, path, &hash(b"gone"), user, request_id)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[async_trait]
impl ToolWritesProbe for Defective {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        let mut all = self.inner.entries();
        all.extend(self.extra.entries());
        if self.defect == Defect::LastUserWins {
            for e in &mut all {
                let key: Key = (e.swimlane.clone(), e.path.clone(), e.request_id.clone());
                if let Some(user) = self.latest_user(false, &key) {
                    e.user = user;
                }
            }
        }
        all
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        let mut all = self.inner.delete_entries();
        all.extend(self.extra.delete_entries());
        all.extend(self.outside.lock().unwrap().clone());
        if self.defect == Defect::LastDeleteUserWins {
            for e in &mut all {
                let key: Key = (e.swimlane.clone(), e.path.clone(), e.request_id.clone());
                if let Some(user) = self.latest_user(true, &key) {
                    e.user = user;
                }
            }
        }
        Some(all)
    }
}

async fn run(defect: Defect) {
    conformance::tool_writes_with_deletes(&Defective::new(defect), &FakeTxFactory::new()).await;
}

#[tokio::test]
async fn the_suite_passes_for_an_implementation_without_a_defect() {
    run(Defect::None).await;
}

#[tokio::test]
#[should_panic(expected = "delete registrations are visible after the commit")]
async fn the_suite_catches_delete_registrations_that_are_dropped() {
    run(Defect::DropsDeletes).await;
}

#[tokio::test]
#[should_panic(expected = "a delete registration is not visible before commit")]
async fn the_suite_catches_delete_registrations_made_outside_the_transaction() {
    run(Defect::DeletesIgnoreTheTransaction).await;
}

#[tokio::test]
#[should_panic(expected = "refused with WrongTx by register_delete too")]
async fn the_suite_catches_a_delete_registration_accepted_from_another_store() {
    run(Defect::DeletesAcceptAnyTransaction).await;
}

#[tokio::test]
#[should_panic(expected = "a transaction of another store is refused with WrongTx")]
async fn the_suite_catches_a_hash_registration_accepted_from_another_store() {
    run(Defect::HashesAcceptAnyTransaction).await;
}

#[tokio::test]
#[should_panic(expected = "a repeated delete registration registers nothing")]
async fn the_suite_catches_a_repeated_delete_registration_that_registers_again() {
    run(Defect::RepeatedDeletesAreNew).await;
}

#[tokio::test]
#[should_panic(expected = "a delete registration is not a hash registration")]
async fn the_suite_catches_a_delete_registration_that_is_also_a_hash_registration() {
    run(Defect::DeleteIsAlsoAHash).await;
}

#[tokio::test]
#[should_panic(expected = "hash a is registered once, whatever delete registration")]
async fn the_suite_catches_a_hash_registration_lost_to_a_delete_registration_of_the_same_path() {
    run(Defect::DeleteShadowsTheHash).await;
}

#[tokio::test]
#[should_panic(expected = "delete registrations are visible after the commit")]
async fn the_suite_catches_a_delete_registration_lost_to_a_hash_registration_of_the_same_path() {
    run(Defect::HashShadowsTheDelete).await;
}

#[tokio::test]
#[should_panic(expected = "delete registrations are visible after the commit")]
async fn the_suite_catches_a_delete_registration_that_forgets_the_hash_it_was_made_with() {
    run(Defect::ForgetsTheDeletedHash).await;
}

#[tokio::test]
#[should_panic(expected = "a delete of another version of the file is another registration")]
async fn the_suite_catches_a_delete_of_another_version_taken_for_a_repeat() {
    run(Defect::DeleteKeyIgnoresTheHash).await;
}

#[tokio::test]
#[should_panic(expected = "a repeat of a hash registration from another user registers nothing")]
async fn the_suite_catches_the_same_hash_registration_from_another_user_registered_twice() {
    run(Defect::SecondUserIsAnotherHash).await;
}

#[tokio::test]
#[should_panic(expected = "a repeat of a delete registration from another user registers nothing")]
async fn the_suite_catches_the_same_delete_registration_from_another_user_registered_twice() {
    run(Defect::SecondUserIsAnotherDelete).await;
}

#[tokio::test]
#[should_panic(expected = "a repeat of a hash registration from another user keeps the first user")]
async fn the_suite_catches_a_repeat_from_another_user_that_replaces_the_first_user() {
    run(Defect::LastUserWins).await;
}

#[tokio::test]
#[should_panic(expected = "a repeat of a delete registration from another user keeps the first user")]
async fn the_suite_catches_a_delete_repeat_from_another_user_that_replaces_the_first_user() {
    run(Defect::LastDeleteUserWins).await;
}

/// A transaction factory whose transactions end wrongly: a rollback that commits (after `honest_rollbacks` honest ones, counted
/// over every transaction it began), or a commit that rolls back.
struct BrokenTxFactory {
    inner: FakeTxFactory,
    honest_rollbacks: usize,
    rollbacks: Arc<AtomicUsize>,
    commit_discards: bool,
}

struct BrokenTx {
    inner: Box<dyn OwnedTx>,
    honest_rollbacks: usize,
    rollbacks: Arc<AtomicUsize>,
    commit_discards: bool,
}

#[async_trait]
impl OwnedTx for BrokenTx {
    fn tx(&mut self) -> Tx<'_> {
        self.inner.tx()
    }

    async fn commit(self: Box<Self>) -> Result<(), TxError> {
        if self.commit_discards {
            self.inner.rollback().await
        } else {
            self.inner.commit().await
        }
    }

    async fn rollback(self: Box<Self>) -> Result<(), TxError> {
        let nth = self.rollbacks.fetch_add(1, Ordering::SeqCst);
        if !self.commit_discards && nth >= self.honest_rollbacks {
            // The rollback commits.
            self.inner.commit().await
        } else {
            self.inner.rollback().await
        }
    }
}

#[async_trait]
impl TxFactory for BrokenTxFactory {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError> {
        Ok(Box::new(BrokenTx {
            inner: self.inner.begin().await?,
            honest_rollbacks: self.honest_rollbacks,
            rollbacks: self.rollbacks.clone(),
            commit_discards: self.commit_discards,
        }))
    }
}

fn rollback_commits_after(honest_rollbacks: usize) -> BrokenTxFactory {
    BrokenTxFactory {
        inner: FakeTxFactory::new(),
        honest_rollbacks,
        rollbacks: Arc::new(AtomicUsize::new(0)),
        commit_discards: false,
    }
}

fn commit_discards() -> BrokenTxFactory {
    BrokenTxFactory {
        commit_discards: true,
        ..rollback_commits_after(0)
    }
}

#[tokio::test]
#[should_panic(expected = "the rollback left nothing behind")]
async fn the_suite_catches_a_rollback_that_commits_a_hash_registration() {
    // The first rollback of the suite is the one of the hash registrations.
    conformance::tool_writes_with_deletes(&FakeToolWrites::new(), &rollback_commits_after(0)).await;
}

#[tokio::test]
#[should_panic(expected = "the rollback left no delete registration behind")]
async fn the_suite_catches_a_rollback_that_commits_a_delete_registration() {
    // The first rollback (the hash registrations') is honest; the second one (the delete registrations') commits.
    conformance::tool_writes_with_deletes(&FakeToolWrites::new(), &rollback_commits_after(1)).await;
}

#[tokio::test]
#[should_panic(expected = "in order, as made")]
async fn the_suite_catches_a_commit_that_discards() {
    conformance::tool_writes_with_deletes(&FakeToolWrites::new(), &commit_discards()).await;
}

// ---------------------------------------------------------------------------------------------------------------------
// The bound

const BOUND: usize = 8;

/// Hash registrations and delete registrations are each held in a store of their own, and each store is bounded: deletes do
/// not count against the bound of the hashes, and the other way round.
struct SplitBound {
    hashes: FakeToolWrites,
    deletes: FakeToolWrites,
}

#[async_trait]
impl ToolWrites for SplitBound {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.hashes
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.deletes
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for SplitBound {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.hashes.entries()
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.deletes.delete_entries())
    }
}

/// A registry that answers `Full` for everything once it holds the bound, a repeat of what it holds included.
struct RepeatIsFull {
    inner: FakeToolWrites,
}

impl RepeatIsFull {
    fn full(&self) -> bool {
        self.inner.entries().len() + self.inner.delete_entries().len() >= BOUND
    }
}

#[async_trait]
impl ToolWrites for RepeatIsFull {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        if self.full() {
            return Err(ToolWriteError::Full);
        }
        self.inner
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        if self.full() {
            return Err(ToolWriteError::Full);
        }
        self.inner
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for RepeatIsFull {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.inner.entries()
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.inner.delete_entries())
    }
}

/// A registry whose probe has lost the oldest registration, as if the commit had dropped one it accepted.
struct DropsOneAtCommit {
    inner: FakeToolWrites,
}

#[async_trait]
impl ToolWrites for DropsOneAtCommit {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.inner
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.inner
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for DropsOneAtCommit {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.inner.entries().into_iter().skip(1).collect()
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.inner.delete_entries())
    }
}

#[tokio::test]
async fn the_bound_suite_passes_for_a_registry_that_counts_both_kinds() {
    conformance::tool_writes_bounded(
        &FakeToolWrites::with_capacity(BOUND),
        &FakeTxFactory::new(),
        BOUND,
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "refused with Full, whichever kind it is")]
async fn the_bound_suite_catches_delete_registrations_that_do_not_count_against_the_bound() {
    let registry = SplitBound {
        hashes: FakeToolWrites::with_capacity(BOUND),
        deletes: FakeToolWrites::with_capacity(BOUND),
    };
    conformance::tool_writes_bounded(&registry, &FakeTxFactory::new(), BOUND).await;
}

#[tokio::test]
#[should_panic(expected = "refused with Full, whichever kind it is")]
async fn the_bound_suite_catches_a_registry_without_a_bound() {
    conformance::tool_writes_bounded(
        &FakeToolWrites::with_capacity(usize::MAX),
        &FakeTxFactory::new(),
        BOUND,
    )
    .await;
}

#[tokio::test]
#[should_panic(expected = "is a no-op, not Full")]
async fn the_bound_suite_catches_a_full_registry_that_refuses_a_repeat() {
    let registry = RepeatIsFull {
        inner: FakeToolWrites::with_capacity(BOUND),
    };
    conformance::tool_writes_bounded(&registry, &FakeTxFactory::new(), BOUND).await;
}

#[tokio::test]
#[should_panic(expected = "Full comes from register, never from the commit")]
async fn the_bound_suite_catches_a_commit_that_drops_what_was_accepted() {
    let registry = DropsOneAtCommit {
        inner: FakeToolWrites::with_capacity(BOUND),
    };
    conformance::tool_writes_bounded(&registry, &FakeTxFactory::new(), BOUND).await;
}

#[tokio::test]
#[should_panic(expected = "a rolled-back registration leaves nothing behind")]
async fn the_bound_suite_catches_a_rollback_that_commits() {
    conformance::tool_writes_bounded(
        &FakeToolWrites::with_capacity(BOUND),
        &rollback_commits_after(0),
        BOUND,
    )
    .await;
}
