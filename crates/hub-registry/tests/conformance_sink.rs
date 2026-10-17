//! The port suites for `ReportSink` (`ports::conformance::report_sink`) and `SentinelSink` (`ports::conformance::sentinel_sink`), run
//! against the real implementations over a real Postgres. For the sentinel: `records` before `hello` is `HelloRequired`, a batch is
//! applied once and acknowledged with its sequence number, a resend is acknowledged with the highest sequence so far and not applied again
//! (D74), and a batch over `max_batch_records` is `TooLarge` and applies nothing. For the agent: nothing is accepted before `hello`, a `hello` for another swimlane is refused (S5), the first heartbeat asks for a
//! full scan, a multi-part delta applies only with its last part, a denied file with bytes and bytes that do not match their hash
//! are `Invalid`, and agents are independent of each other.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use support::rig::Rig;

#[tokio::test]
async fn report_sink_conformance() {
    let Some(rig) = Rig::new().await else { return };
    ports::conformance::report_sink(&rig.ingest).await;
}

/// The suite sends a part of a delta numbered 0 as the last message of a delta whose first part was numbered 0 too. The hub
/// groups the parts of a delta by their roots, so the staged part is applied with it.
#[tokio::test]
async fn the_suites_second_part_is_applied_with_the_first() {
    let Some(rig) = Rig::new().await else { return };
    ports::conformance::report_sink(&rig.ingest).await;
    assert!(
        rig.state("c.yml").await.is_some(),
        "c.yml was in the part with `more`; the last part applies it"
    );
    assert!(rig.state("a/b.yml").await.is_some());
    assert_eq!(rig.count("SELECT count(*) FROM delta_staging").await, 0);
}

#[tokio::test]
async fn sentinel_sink_conformance() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    ports::conformance::sentinel_sink(&rig.sentinel_probe()).await;
}
