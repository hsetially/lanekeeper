//! P15 (T1): what the tool-write registrations add to the hot path of ingestion, counted from sqlx's own statement log.
//!
//! The registry applies a delta in chunks of at most `chunk_paths` paths, one transaction each. For the registrations a chunk costs **one
//! `SELECT`** of the table by the paths of the chunk (served by the unique key), whatever the number of paths and of registrations, and
//! **one `DELETE`** only when an observation of the chunk consumed or contradicted a registration. A chunk that no registration concerns
//! (the usual case) costs the one read and nothing else. (The first seam also read once for each chunk, from `file_observations` by
//! source; the read is replaced, not added to.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use support::log_capture::LogCapture;
use support::rig::{Rig, T, delta, entry, root};
use support::statements::{Statement, statements};

fn touching_registrations(all: &[Statement]) -> Vec<&Statement> {
    all.iter()
        .filter(|s| s.text.contains("tool_write_registrations"))
        .collect()
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("svc/f{i:04}.yml")).collect()
}

/// An inventory of `n` files, then a delta that changes all of them (`registered` of them have a registration for the new content), with
/// the statements of that delta.
async fn statements_of_a_delta(rig: &Rig, n: usize, registered: usize) -> Vec<Statement> {
    rig.hello().await;
    let names = names(n);
    let inventory = names.iter().map(|p| entry(p, b"v: 0", T)).collect();
    rig.delta(delta(1, None, root(1), inventory)).await.unwrap();
    if registered > 0 {
        let mut tx = rig.caller().await;
        for (i, p) in names[..registered].iter().enumerate() {
            rig.register_in(&mut tx, p, "v: 1", &format!("req-{i}"), 5).await;
        }
        tx.commit().await.unwrap();
    }
    rig.clock.advance_secs(3);
    let (capture, _guard) = LogCapture::install();
    let edits = names.iter().map(|p| entry(p, b"v: 1", T + 5_000)).collect();
    rig.delta(delta(2, Some(root(1)), root(2), edits)).await.unwrap();
    statements(&capture)
}

#[tokio::test]
async fn a_chunk_that_no_registration_concerns_costs_one_read_and_no_write() {
    let Some(rig) = Rig::new().await else { return };
    let all = statements_of_a_delta(&rig, 50, 0).await;
    assert!(
        !all.is_empty(),
        "no statement was captured: the counts below would pass for nothing"
    );
    let touching = touching_registrations(&all);
    assert_eq!(
        touching.len(),
        1,
        "one statement of the table for a chunk of 50 paths: {touching:?}",
        touching = touching.iter().map(|s| &s.text).collect::<Vec<_>>()
    );
    assert!(touching[0].text.contains("SELECT"), "and it is the read");
    assert!(
        !all.iter()
            .any(|s| s.text.contains("DELETE FROM tool_write_registrations")),
        "nothing is dropped when nothing is registered"
    );
    assert_eq!(rig.metrics.out_of_band_count(), 50);
}

#[tokio::test]
async fn the_read_is_one_statement_for_each_chunk_whatever_the_number_of_paths() {
    let Some(rig) = Rig::new().await else { return };
    // 120 paths at 50 a chunk: three chunks, three reads.
    let all = statements_of_a_delta(&rig, 120, 0).await;
    assert_eq!(touching_registrations(&all).len(), 3);
}

#[tokio::test]
async fn the_registrations_a_chunk_consumes_are_dropped_by_one_delete() {
    let Some(rig) = Rig::new().await else { return };
    // Twenty of the fifty paths have a registration for the content that arrives.
    let all = statements_of_a_delta(&rig, 50, 20).await;
    let touching = touching_registrations(&all);
    assert_eq!(touching.len(), 2, "the read, and one delete for all of them");
    assert!(touching[0].text.contains("SELECT") && touching[1].text.contains("DELETE"));
    assert_eq!(touching[1].rows, 20, "twenty registrations consumed");
    assert_eq!(
        rig.metrics.out_of_band_count(),
        30,
        "the thirty others are out of band"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM tool_write_registrations").await,
        0
    );
}
