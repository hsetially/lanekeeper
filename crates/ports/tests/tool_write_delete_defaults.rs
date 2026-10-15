//! The default `ToolWrites::register_delete`: an implementation that has only `register` (every implementor that existed before
//! the method) keeps compiling and behaving as it did, and gets the documented do-nothing default. The tests also pin what the
//! rustdoc warns about: a decorator that forwards only `register` drops every delete without a sound, and the conformance
//! suite is what notices.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};
use ports::conformance::sample::{hash, nfs, request_id, swimlane, user_id};
use ports::conformance::{self, ToolWritesProbe};
use ports::fakes::{FakeToolWrites, FakeTxFactory};
use ports::{ToolDeleteEntry, ToolWriteEntry, ToolWriteError, ToolWrites, Tx, TxFactory};

/// An implementation with only `register`, as every one was before `register_delete`: it keeps hash registrations in a fake.
struct OnlyRegister(FakeToolWrites);

#[async_trait]
impl ToolWrites for OnlyRegister {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.0
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for OnlyRegister {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.0.entries()
    }
}

/// A decorator written before `register_delete` existed. It forwards `register` and nothing else, and its probe reads the
/// wrapped fake, which does register deletes, so it opts in: what a wrapper that forgot the method looks like to the suite.
struct ForwardsOnlyRegister(FakeToolWrites);

#[async_trait]
impl ToolWrites for ForwardsOnlyRegister {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.0
            .register(tx, swimlane, path, new_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for ForwardsOnlyRegister {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.0.entries()
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.0.delete_entries())
    }
}

/// The same decorator, written correctly: it forwards both.
struct ForwardsBoth(FakeToolWrites);

#[async_trait]
impl ToolWrites for ForwardsBoth {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.0
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
        self.0
            .register_delete(tx, swimlane, path, deleted_hash, user, request_id)
            .await
    }
}

#[async_trait]
impl ToolWritesProbe for ForwardsBoth {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.0.entries()
    }

    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.0.delete_entries())
    }
}

#[tokio::test]
async fn the_default_register_delete_says_ok_and_registers_nothing() {
    let registry = OnlyRegister(FakeToolWrites::new());
    let txf = FakeTxFactory::new();
    let mut tx = txf.begin().await.unwrap();
    registry
        .register_delete(
            &mut tx.tx(),
            &swimlane("sit1"),
            &nfs("svc/a.yml"),
            &hash(b"a: 1\n"),
            &user_id(1),
            &request_id("req-1"),
        )
        .await
        .expect("the default says Ok");
    tx.commit().await.unwrap();
    assert!(
        registry.0.delete_entries().is_empty() && registry.0.entries().is_empty(),
        "the default registers nothing: a delete made through the tool stays an out-of-band change"
    );
    assert_eq!(
        registry.registered_deletes().await,
        None,
        "and a probe that does not override registered_deletes does not opt in"
    );
}

#[tokio::test]
async fn an_implementation_without_the_method_passes_the_suite() {
    conformance::tool_writes(&OnlyRegister(FakeToolWrites::new()), &FakeTxFactory::new()).await;
}

#[tokio::test]
#[should_panic(expected = "keeps the default `register_delete`")]
async fn the_strict_suite_refuses_a_probe_that_does_not_opt_in() {
    conformance::tool_writes_with_deletes(&OnlyRegister(FakeToolWrites::new()), &FakeTxFactory::new()).await;
}

#[tokio::test]
async fn the_bound_suite_runs_on_hash_registrations_alone_for_an_implementation_without_the_method() {
    conformance::tool_writes_bounded(
        &OnlyRegister(FakeToolWrites::with_capacity(6)),
        &FakeTxFactory::new(),
        6,
    )
    .await;
}

/// The pitfall of the rustdoc, pinned: a delete sent through a decorator that forwards only `register` never reaches the
/// wrapped implementation, whatever that does with it, and the caller is told `Ok`.
#[tokio::test]
async fn a_decorator_that_forwards_only_register_drops_deletes_without_a_sound() {
    let wrapped = FakeToolWrites::new();
    let decorator = ForwardsOnlyRegister(wrapped.clone());
    let txf = FakeTxFactory::new();
    let args = (
        swimlane("sit1"),
        nfs("svc/a.yml"),
        hash(b"a: 1\n"),
        user_id(1),
        request_id("req-1"),
    );

    let mut tx = txf.begin().await.unwrap();
    decorator
        .register_delete(&mut tx.tx(), &args.0, &args.1, &args.2, &args.3, &args.4)
        .await
        .expect("the decorator says Ok");
    tx.commit().await.unwrap();
    assert!(wrapped.delete_entries().is_empty(), "the delete was dropped");

    let mut tx = txf.begin().await.unwrap();
    wrapped
        .register_delete(&mut tx.tx(), &args.0, &args.1, &args.2, &args.3, &args.4)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        wrapped.delete_entries().len(),
        1,
        "the wrapped fake does register it"
    );
}

/// The suite sees it at its first delete check: the inherited default answers `Ok` to a transaction of another store, where an
/// implementation that registers deletes answers `WrongTx` (and, past that check, registers nothing).
#[tokio::test]
#[should_panic(expected = "refused with WrongTx by register_delete too")]
async fn the_suite_catches_a_decorator_that_forwards_only_register() {
    conformance::tool_writes_with_deletes(
        &ForwardsOnlyRegister(FakeToolWrites::new()),
        &FakeTxFactory::new(),
    )
    .await;
}

#[tokio::test]
async fn a_decorator_that_forwards_both_passes_the_strict_suite() {
    conformance::tool_writes_with_deletes(&ForwardsBoth(FakeToolWrites::new()), &FakeTxFactory::new()).await;
}
