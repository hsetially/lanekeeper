//! The log-capture helper keeps working however the test threads interleave (S10 tests depend on it).
//!
//! `tracing` caches, per callsite, whether any subscriber wants it. While exactly one thread-local subscriber is alive, a
//! callsite that is first reached on a thread with no subscriber is cached as "nobody wants it", and the thread that does
//! capture never sees it. These tests drive that interleaving with channels, so they do not depend on timing: they fail
//! deterministically for a capture built on `tracing::subscriber::set_default`.
//!
//! The tests share one lock so that they do not hide each other's races (with two live subscribers `tracing` asks both).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::mpsc::channel;
use std::sync::{Mutex, MutexGuard};
use std::thread;

use support::log_capture;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Each helper owns one callsite, so no other test can have registered it first.
fn emit_first_elsewhere(text: &str) {
    tracing::info!(marker = text, "callsite one");
}
fn emit_after_install(text: &str) {
    tracing::info!(marker = text, "callsite two");
}
fn emit_two_threads(text: &str) {
    tracing::info!(marker = text, "callsite three");
}
fn emit_in_span(text: &str) {
    let span = tracing::info_span!("capture_span", marker = text);
    let _entered = span.enter();
    tracing::info!("inside the span");
}

/// The failing order: the capture is installed, then a thread without one reaches a callsite for the first time.
#[test]
fn a_callsite_first_reached_on_a_thread_without_capture_is_still_captured() {
    let _serial = serial();
    let (capture, _guard) = log_capture::install();
    thread::spawn(|| emit_first_elsewhere("from-the-other-thread"))
        .join()
        .unwrap();
    emit_first_elsewhere("from-the-capturing-thread");
    let text = capture.text();
    assert!(text.contains("from-the-capturing-thread"), "{text:?}");
    assert!(
        !text.contains("from-the-other-thread"),
        "another thread's events are not this test's: {text:?}"
    );
}

/// The same, with the other thread running while the capture is installed and the capture's thread waiting for it.
#[test]
fn the_capture_wins_however_the_threads_interleave() {
    let _serial = serial();
    let (installed_tx, installed_rx) = channel::<()>();
    let (done_tx, done_rx) = channel::<()>();
    let other = thread::spawn(move || {
        installed_rx.recv().unwrap();
        emit_after_install("other");
        done_tx.send(()).unwrap();
    });
    let (capture, _guard) = log_capture::install();
    installed_tx.send(()).unwrap();
    done_rx.recv().unwrap();
    emit_after_install("mine");
    other.join().unwrap();
    let text = capture.text();
    assert!(text.contains("mine"), "{text:?}");
    assert!(!text.contains("other"), "{text:?}");
}

/// Two tests capture at once, each on its own thread: each sees its own events and none of the other's.
#[test]
fn captures_on_different_threads_are_isolated() {
    let _serial = serial();
    let (ready_tx, ready_rx) = channel::<()>();
    let (go_tx, go_rx) = channel::<()>();
    let second = thread::spawn(move || {
        let (capture, _guard) = log_capture::install();
        ready_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        emit_two_threads("second");
        capture.text()
    });
    let (capture, _guard) = log_capture::install();
    ready_rx.recv().unwrap();
    emit_two_threads("first");
    go_tx.send(()).unwrap();
    let theirs = second.join().unwrap();
    let mine = capture.text();
    assert!(mine.contains("first") && !mine.contains("second"), "{mine:?}");
    assert!(
        theirs.contains("second") && !theirs.contains("first"),
        "{theirs:?}"
    );
}

/// Once the guard is dropped, nothing more is recorded, and a nested install restores the outer capture.
#[test]
fn the_guard_ends_the_capture_and_installs_nest() {
    let _serial = serial();
    let (outer, outer_guard) = log_capture::install();
    emit_in_span("outer-1");
    {
        let (inner, _inner_guard) = log_capture::install();
        emit_in_span("inner");
        let inner = inner.text();
        assert!(inner.contains("inner") && !inner.contains("outer-1"), "{inner:?}");
    }
    emit_in_span("outer-2");
    drop(outer_guard);
    emit_in_span("after-the-guard");
    let outer = outer.text();
    assert!(
        outer.contains("outer-1") && outer.contains("outer-2"),
        "{outer:?}"
    );
    assert!(
        !outer.contains("inner") && !outer.contains("after-the-guard"),
        "{outer:?}"
    );
    // Spans are included: the span's fields are on the event inside it.
    assert!(outer.contains("capture_span"), "{outer:?}");
}
