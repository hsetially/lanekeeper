//! The test database is torn down inside a live runtime, and dropping it anywhere else never needs one.
//!
//! The benchmarks build their own runtime, use it with `block_on`, and drop the rig once the last `block_on` has returned.
//! `testcontainers` removes a container from `Drop` through `tokio::runtime::Handle::current()`, which panics ("there is no
//! reactor running") outside a runtime, so with a real container both benchmarks aborted at teardown with exit 101 and
//! `cargo bench -p hub-identity` never finished. There is no Docker here, so these tests use a stand-in container that
//! behaves the same way: its `Drop` panics unless a runtime is current.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use futures::future::BoxFuture;
use support::pg::{Container, TestDb};

/// What happened to the stand-in, readable after the rig is gone.
#[derive(Default)]
struct Log {
    removed_async: AtomicU32,
    removed_blocking: AtomicU32,
    abandoned: AtomicBool,
    dropped: AtomicBool,
    fail_remove: AtomicBool,
}

struct Reactorful(Arc<Log>);

impl Container for Reactorful {
    fn id(&self) -> String {
        "fake-container".to_owned()
    }

    fn remove(&mut self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.0.removed_async.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn remove_blocking(&self) {
        self.0.removed_blocking.fetch_add(1, Ordering::SeqCst);
    }

    fn abandon(self: Box<Self>) {
        self.0.abandoned.store(true, Ordering::SeqCst);
        std::mem::forget(self);
    }
}

impl Drop for Reactorful {
    fn drop(&mut self) {
        // The line of `testcontainers::core::async_drop` that panics outside a runtime.
        let _ = tokio::runtime::Handle::current();
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

fn rig(log: &Arc<Log>) -> TestDb {
    TestDb::detached(Box::new(Reactorful(Arc::clone(log))))
}

/// The benchmarks' shape: a runtime of their own, the rig built and shut down inside it, dropped after.
#[test]
fn shutdown_inside_the_runtime_then_drop_outside_it() {
    let log = Arc::new(Log::default());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let db = rt.block_on(async { rig(&log) });
    rt.block_on(db.shutdown());
    assert_eq!(
        log.removed_async.load(Ordering::SeqCst),
        1,
        "removed through the async API"
    );
    assert_eq!(log.removed_blocking.load(Ordering::SeqCst), 0);
    assert!(!log.abandoned.load(Ordering::SeqCst));
    // `shutdown` consumed the rig; nothing is left to drop outside the runtime, and the runtime can go.
    drop(rt);
    assert_eq!(log.removed_async.load(Ordering::SeqCst), 1, "removed once");
}

/// A rig that is only dropped (a benchmark that panicked, a forgotten `shutdown`) outside any runtime must not panic.
#[test]
fn drop_outside_a_runtime_never_needs_one() {
    let log = Arc::new(Log::default());
    let db = rig(&log);
    drop(db);
    assert_eq!(
        log.removed_blocking.load(Ordering::SeqCst),
        1,
        "removed without a runtime"
    );
    assert!(
        log.abandoned.load(Ordering::SeqCst),
        "its own Drop, which needs a reactor, never ran"
    );
    assert_eq!(log.removed_async.load(Ordering::SeqCst), 0);
}

/// Dropped on the thread of a runtime, as the tests do, the container's own `Drop` keeps working: that path is the one
/// `testcontainers` supports, and it ran in every passing test.
#[test]
fn drop_inside_a_runtime_leaves_it_to_the_container() {
    let log = Arc::new(Log::default());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        drop(rig(&log));
    });
    assert!(log.dropped.load(Ordering::SeqCst), "the container's own Drop ran");
    assert_eq!(log.removed_blocking.load(Ordering::SeqCst), 0);
    assert!(!log.abandoned.load(Ordering::SeqCst));
}

#[test]
fn the_rig_can_be_shut_down_on_a_multi_thread_runtime_too() {
    let log = Arc::new(Log::default());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let db = rig(&log);
        db.shutdown().await;
    });
    assert_eq!(log.removed_async.load(Ordering::SeqCst), 1);
}

/// A failed removal does not panic, and the blocking removal runs as the last resort.
#[test]
fn a_failed_async_removal_falls_back_to_the_blocking_one() {
    struct Failing(Reactorful);
    impl Container for Failing {
        fn id(&self) -> String {
            self.0.id()
        }
        fn remove(&mut self) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async move {
                self.0.0.fail_remove.store(true, Ordering::SeqCst);
                Err("daemon went away".to_owned())
            })
        }
        fn remove_blocking(&self) {
            self.0.remove_blocking();
        }
        fn abandon(self: Box<Self>) {
            std::mem::forget(self);
        }
    }
    let log = Arc::new(Log::default());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        TestDb::detached(Box::new(Failing(Reactorful(Arc::clone(&log)))))
            .shutdown()
            .await;
    });
    assert!(log.fail_remove.load(Ordering::SeqCst));
    assert_eq!(
        log.removed_blocking.load(Ordering::SeqCst),
        1,
        "the blocking removal ran as the fallback"
    );
}
