//! P15, P1, P5 (T7): attribution adds nothing to the hot path of ingestion. The registry applies a scan delta in chunks of at most
//! `chunk_paths` paths and P15 is spent in the statements of that loop, so the sentinel's correlation happens in the sink's own transaction
//! and in the leased job, never in the apply loop. Held two ways: the statements of a delta are the same whether or not a sentinel is
//! registered and has records waiting, and no source of the apply loop or the pipeline names the sentinel's tables.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use hub_registry::ManualClock;
use support::log_capture::LogCapture;
use support::rig::{Rig, T, delta, entry, root};
use support::sentinel::{batch, record};
use support::statements::{Statement, statements};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("svc/f{i:04}.yml")).collect()
}

/// An inventory of `n` files, then a delta that changes all of them, with the statements of that delta.
async fn statements_of_a_delta(rig: &Rig, n: usize) -> Vec<Statement> {
    rig.hello().await;
    let names = names(n);
    let inventory = names.iter().map(|p| entry(p, b"v: 0", T)).collect();
    rig.delta(delta(1, None, root(1), inventory)).await.unwrap();
    rig.clock.advance_secs(3);
    let (capture, _guard) = LogCapture::install();
    let edits = names
        .iter()
        .map(|p| entry(p, b"v: 1", ManualClock::T0_MS + 1_000))
        .collect();
    rig.delta(delta(2, Some(root(1)), root(2), edits)).await.unwrap();
    statements(&capture)
}

#[tokio::test]
async fn a_delta_costs_the_same_statements_with_a_sentinel_and_records_waiting_as_without() {
    let Some(plain) = Rig::new().await else { return };
    let without = statements_of_a_delta(&plain, 120).await;
    assert!(
        !without.is_empty(),
        "the capture works: the counts below would pass for nothing"
    );

    let Some(watched) = Rig::new().await else { return };
    watched.sentinel_for_the_swimlane().await;
    watched.add_user(1, "ada@example.com", "active").await;
    watched.sentinel_hello().await.unwrap();
    // Records for every path the delta is about, stored before it arrives: the correlation would be at its busiest if ingestion did it.
    let records = (0..120)
        .map(|i| record(&format!("svc/f{i:04}.yml"), ManualClock::T0_MS + 500))
        .collect::<Vec<_>>();
    for (n, chunk) in records.chunks(100).enumerate() {
        watched
            .records(batch(u64::try_from(n).unwrap() + 1, chunk.to_vec()))
            .await
            .unwrap();
    }
    let with = statements_of_a_delta(&watched, 120).await;

    assert_eq!(
        with.len(),
        without.len(),
        "ingestion gains no statement for attribution: {} against {}",
        with.len(),
        without.len()
    );
    for s in &with {
        assert!(
            !s.text.contains("sentinel"),
            "an apply statement names a sentinel table: {}",
            s.text
        );
    }
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The lines of a file that are code: before the test module, and not comments.
fn code(text: &str) -> String {
    text.lines()
        .take_while(|l| l.trim() != "#[cfg(test)]")
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn nothing_in_the_apply_loop_or_the_pipeline_names_the_sentinel_or_the_correlator() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for dir in ["ingest", "pipeline"] {
        let mut files = Vec::new();
        sources(&src.join(dir), &mut files);
        for file in files {
            let text = code(&fs::read_to_string(&file).unwrap());
            for forbidden in [
                "sentinel_records",
                "sentinels",
                "attribution::",
                "SentinelIngest",
                "os_login",
            ] {
                assert!(
                    !text.contains(forbidden),
                    "{} names {forbidden}: ingestion and the pipeline do not do the sentinel's work (P15)",
                    file.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 15, "the scan covers the loop, covered {checked} files");
}

#[test]
fn the_scan_finds_a_planted_mention() {
    assert!(code("let x = sqlx::query(\"SELECT 1 FROM sentinel_records\");").contains("sentinel_records"));
    assert!(!code("// sentinel_records in prose\nlet x = 1;").contains("sentinel_records"));
}
