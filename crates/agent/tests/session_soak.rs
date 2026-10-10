//! The same restart cycle, four hundred times: memory has stopped growing (T3, S6).
//!
//! The first few hundred connections warm up caches that have a bound (the TLS server's session cache holds 256
//! entries, for one). After that a flat line means nothing is kept per connection. Alone in its file for the same
//! reason as `session_restarts`.
// The numbers go to stderr on purpose: they are the evidence that memory is flat.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod support;

const RESTARTS: usize = 400;
/// Growth is measured over the last 150 connections.
const WINDOW: usize = 150;

#[tokio::test(start_paused = true)]
async fn four_hundred_hub_restarts_leave_memory_flat() {
    let report = support::restarts::run(RESTARTS).await;
    let n = report.pages.len();
    let (before, after) = (report.pages[n - 1 - WINDOW], report.pages[n - 1]);
    eprintln!(
        "pages at connection {} and {}: {before} and {after}; tasks {:?} .. {:?}",
        n - WINDOW,
        n,
        report.tasks.first(),
        report.tasks.last()
    );
    // Under 512 KiB over 150 connections: nothing of 3.5 KiB or more is kept per connection. A connection's own
    // buffers (the two 64 KiB pipes, the TLS and HTTP/2 state) are an order of magnitude more than that, so keeping
    // any of them would show here. What is left is allocator noise and the few bytes the test itself records.
    assert!(
        after.saturating_sub(before) < 128,
        "memory still grows: {before} pages at connection {} and {after} at connection {n}",
        n - WINDOW
    );
    assert!(report.tasks[n - 1] <= report.tasks[WINDOW], "{:?}", report.tasks);
}
