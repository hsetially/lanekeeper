//! What the incremental recompute sends to the database (P5, T3, T10): the statements `Pipeline::process` runs for a one-file change, counted
//! from sqlx's own statement log and pinned.
//!
//! T10 added the structure checks (C9 to C11) to the registry, and the plan says that they must not slow the incremental recompute. They
//! run in a leased job of their own, never inside `process`, so the number of statements of a batch is the number it was before them. This
//! test is the proof that holds that number: a check that was added to the batch would add a statement, and the count would move. The
//! number was measured on the code of the commit before T10's second half (`c97ffd3`) and is the same now; it also does not depend on the
//! size of the swimlane (a one-file change in 40 files and in 800 sends the same statements).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use support::log_capture::LogCapture;
use support::statements::statements;
use support::world::World;

/// The statements `process` sends for one changed file of a swimlane that is in memory. Measured on the code before the structure checks
/// existed (`c97ffd3`): the same.
const ONE_FILE_CHANGE: usize = 18;

fn swimlane(files: usize) -> Vec<(String, String)> {
    (0..files)
        .map(|i| (format!("svc{}/file{i}.yml", i % 20), format!("k: {i}\n")))
        .collect()
}

async fn statements_of_a_one_file_change(files: usize) -> Option<(usize, Vec<String>)> {
    let mut w = World::new(&["sit1"]).await?;
    let all = swimlane(files);
    let refs: Vec<(&str, &str)> = all.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    w.inventory(&refs).await;
    // The swimlane is loaded and computed whole: from here on it is in memory and a change is an incremental update.
    w.whole().await;
    w.nfs_change(&[("svc3/file3.yml", "k: changed\n")]).await;
    let (capture, _guard) = LogCapture::install();
    let report = w.paths(&["svc3/file3.yml"]).await;
    assert_eq!(report.lanes.len(), 1, "{report:?}");
    // The event bus's relay polls the outbox on a task of its own, on the same thread: how often it looks is the timing of the test, not
    // what the batch sends.
    let all: Vec<String> = statements(&capture)
        .into_iter()
        .map(|s| s.text)
        .filter(|t| !t.contains("FROM outbox WHERE id >"))
        .collect();
    Some((all.len(), all))
}

#[tokio::test]
async fn a_one_file_change_sends_the_statements_it_always_sent_and_no_structure_check() {
    let Some((n, texts)) = statements_of_a_one_file_change(40).await else {
        return;
    };
    assert_eq!(
        n,
        ONE_FILE_CHANGE,
        "the incremental recompute's statements moved:\n{}",
        texts.join("\n")
    );
    // Nothing of C9 to C11 is read or written inside a batch: those kinds and the folders are not touched.
    let all = texts.join("\n");
    for forbidden in [
        "swimlane_folders",
        "ambiguous_file_name",
        "unresolved_placeholder",
    ] {
        assert!(
            !all.contains(forbidden),
            "the batch touched `{forbidden}`:\n{all}"
        );
    }
}

#[tokio::test]
async fn the_statements_of_a_one_file_change_do_not_depend_on_the_size_of_the_swimlane() {
    let (Some((small, _)), Some((large, _))) = (
        statements_of_a_one_file_change(40).await,
        statements_of_a_one_file_change(800).await,
    ) else {
        return;
    };
    assert_eq!(small, large, "a bigger swimlane must not mean more statements");
}
