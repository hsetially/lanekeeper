//! A test database that owns a container is dropped under a Tokio runtime wherever the test drops it. `ContainerAsync`'s `Drop` calls
//! `Handle::current()`, which panics when the thread is in no runtime; CI, which runs the real container (an external server has none
//! to drop), failed `retention_gc::gc_never_deletes_a_referenced_blob` that way: the proptest builds a runtime of its own, and its
//! database is dropped at the end of the function, after `block_on` returned. No Docker is needed here: the stand-in's `Drop` makes the
//! same call.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use support::pg::{TestDb, drop_with_runtime};

/// Fails in `Drop` as the container does outside a runtime, and says when it ran to the end.
struct NeedsRuntime(Arc<AtomicBool>);

impl NeedsRuntime {
    fn new() -> (Self, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        (Self(Arc::clone(&dropped)), dropped)
    }
}

impl Drop for NeedsRuntime {
    fn drop(&mut self) {
        let _ = tokio::runtime::Handle::current();
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn the_stand_in_fails_outside_a_runtime_as_the_containers_drop_does() {
    let (value, dropped) = NeedsRuntime::new();
    // On a thread of its own: the test thread of the harness is in no runtime either, but nothing else shares it.
    let outcome = std::thread::spawn(move || catch_unwind(AssertUnwindSafe(|| drop(value))).is_err())
        .join()
        .unwrap();
    assert!(
        outcome,
        "the stand-in's Drop did not panic outside a runtime, so it proves nothing"
    );
    assert!(!dropped.load(Ordering::SeqCst));
}

#[test]
fn drop_with_runtime_from_a_thread_in_no_runtime_does_not_panic() {
    let (value, dropped) = NeedsRuntime::new();
    drop_with_runtime(value);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the value was not dropped to the end"
    );
}

#[tokio::test]
async fn drop_with_runtime_inside_a_current_thread_runtime_does_not_panic() {
    let (value, dropped) = NeedsRuntime::new();
    drop_with_runtime(value);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_with_runtime_inside_a_multi_thread_runtime_does_not_panic() {
    let (value, dropped) = NeedsRuntime::new();
    drop_with_runtime(value);
    assert!(dropped.load(Ordering::SeqCst));
}

/// The shape of the CI failure: the test builds a runtime, makes its database in it, and drops the database after `block_on` returned,
/// on the test thread, which is in no runtime (the runtime itself is still alive, as `rt` is in `retention_gc`).
#[test]
fn a_test_db_that_owns_a_runtime_bound_resource_drops_outside_its_runtime_without_a_panic() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (value, dropped) = NeedsRuntime::new();
    let db = rt.block_on(async move { TestDb::around(value) });
    assert!(!dropped.load(Ordering::SeqCst));
    drop(db);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the resource of the test database was not dropped to the end"
    );
    drop(rt);
}

#[test]
fn a_test_db_dropped_by_a_plain_thread_does_not_panic() {
    let (value, dropped) = NeedsRuntime::new();
    let db = TestDb::around(value);
    std::thread::spawn(move || drop(db)).join().unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_test_db_dropped_inside_a_tokio_test_does_not_panic() {
    let (value, dropped) = NeedsRuntime::new();
    drop(TestDb::around(value));
    assert!(dropped.load(Ordering::SeqCst));
}
