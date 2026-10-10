//! The hub restarts twenty times; the agent comes back every time and does not grow (T3, S6).
//!
//! This test is alone in its file on purpose: `cargo test` runs one test binary per file, so the process contains
//! nothing but this test, and its resident memory means something.
// The numbers go to stderr on purpose: they are the evidence that memory is flat.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod support;

const RESTARTS: usize = 20;
/// The first connections warm up the allocator, the TLS tables and the HTTP/2 buffers. Growth is measured after that.
const WARM_UP: usize = 4;

#[tokio::test(start_paused = true)]
async fn hub_restarted_20_times_agent_recovers_memory_flat() {
    // `restarts::run` asserts the recovery itself: after every restart a whole connection, and nothing left over from
    // the old ones (streams, outboxes) on either side.
    let report = support::restarts::run(RESTARTS).await;

    // The number of live tasks does not climb with the number of restarts.
    eprintln!("alive tasks per connection: {:?}", report.tasks);
    let (settled, last) = (report.tasks[WARM_UP], *report.tasks.last().unwrap());
    assert!(
        last <= settled,
        "tasks grew from {settled} to {last} over {} restarts: {:?}",
        RESTARTS - WARM_UP,
        report.tasks
    );

    // Resident memory: under 2 MiB between the fifth connection and the last. A connection leaking its 64 KiB of
    // buffers would add 1 MiB over these sixteen restarts on its own; `session_soak` shows the long-run slope.
    eprintln!("resident 4 KiB pages per connection: {:?}", report.pages);
    let (settled, last) = (report.pages[WARM_UP], *report.pages.last().unwrap());
    let grown_kib = last.saturating_sub(settled) * 4;
    assert!(
        grown_kib < 2 * 1024,
        "resident memory grew by {grown_kib} KiB over {} restarts: {:?}",
        RESTARTS - WARM_UP,
        report.pages
    );
}
