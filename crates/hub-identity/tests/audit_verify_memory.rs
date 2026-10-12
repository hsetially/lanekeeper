//! B2 (round 3): a huge stored value is never transferred or decoded, so the verifier's peak memory does not grow with it.
//!
//! A test file of its own with one test: the figure is the peak resident set of the whole process (`VmHWM`), which other tests
//! running beside this one in the same binary would move.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use hub_identity::audit::verify::FailureKind;
use hub_identity::audit::{Column, ColumnSet};
use support::audit_rig::AuditRig;

/// The two tests read a process-wide peak, so they take turns.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The peak resident set of this process so far, in KiB.
fn peak_rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmHWM in /proc/self/status")
}

#[tokio::test]
async fn a_huge_stored_value_does_not_raise_the_peak_memory_of_the_verifier() {
    let _turn = ONE_AT_A_TIME.lock().await;
    let rig = AuditRig::new(db!()).await;
    rig.record(5).await;
    rig.tamper_open(false).await;
    // Built inside the server, so none of it passes through this process: a 16 MB `jsonb` of eight million numbers (decoded into
    // a `serde_json::Value` it costs about 300 MB in a debug build, 18 bytes for every byte of text) and 64 MB of text.
    rig.admin_sql(
        "UPDATE audit_events SET attribution_evidence = ('[' || repeat('1,', 8000000) || '1]')::jsonb, \
         github_login = repeat('z', 64000000) WHERE seq = 3",
    )
    .await;

    let before = peak_rss_kib();
    let report = rig.chain_verifier().verify().await.unwrap();
    let after = peak_rss_kib();

    assert_eq!(
        report.failures.len(),
        1,
        "the row is reported: {:?}",
        report.failures
    );
    assert_eq!(report.failures[0].seq, 3);
    assert_eq!(
        report.failures[0].kind,
        FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::GithubLogin, Column::AttributionEvidence]))
    );
    let grew = after.saturating_sub(before);
    assert!(
        grew < 64 * 1024,
        "the peak resident set grew by {grew} KiB while verifying a row with 80 MB in two columns"
    );
}

#[tokio::test]
async fn the_largest_row_the_bounds_let_through_has_a_bounded_cost() {
    // Every column that has a bound is filled to just under it, and the event itself to just under the megabyte the log allows,
    // with the hash kept good, so that every column is decoded and compared: the most one row can ask of the verifier. (It is
    // about 14 MB in the row; the figure below, about three times the 45 MB measured, is the process's peak growth for it, `serde_json` values and all, in a debug build.)
    let _turn = ONE_AT_A_TIME.lock().await;
    let rig = AuditRig::new(db!()).await;
    rig.record(5).await;
    rig.tamper_open(true).await;
    for statement in [
        "DROP INDEX audit_events_actor_idx",
        "DROP INDEX audit_events_swimlane_idx",
        "DROP INDEX audit_events_path_idx",
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_actor_kind_check",
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_action_check",
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_via_check",
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_attribution_source_check",
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_attribution_confidence_check",
    ] {
        rig.admin_sql(statement).await;
    }
    rig.admin_sql(
        "UPDATE audit_events SET actor_kind = repeat('a', 1000000), actor_ref = repeat('b', 1000000), \
         action = repeat('c', 1000000), via = repeat('d', 1000000), swimlane_id = repeat('e', 1000000), \
         path = repeat('f', 1000000), github_login = repeat('g', 1000000), request_id = repeat('h', 1000000), \
         attribution_source = repeat('i', 1000000), attribution_confidence = repeat('j', 1000000), \
         attribution_actor = ('\"' || repeat('k', 2000000) || '\"')::jsonb, \
         attribution_evidence = ('\"' || repeat('l', 2000000) || '\"')::jsonb, \
         event_json = event_json || repeat(' ', 1000000), \
         hash = sha256(prev_hash || convert_to(event_json || repeat(' ', 1000000), 'UTF8')) WHERE seq = 3",
    )
    .await;
    // The change of hash breaks the link of the next row; the row itself is read in full, hash good, every column compared.
    let before = peak_rss_kib();
    let report = rig.chain_verifier().verify().await.unwrap();
    let after = peak_rss_kib();
    assert_eq!(report.failures.first().map(|f| f.seq), Some(3));
    assert!(
        after.saturating_sub(before) < 128 * 1024,
        "the peak resident set grew by {} KiB for one row of 14 MB",
        after.saturating_sub(before)
    );
}
