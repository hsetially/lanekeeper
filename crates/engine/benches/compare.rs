//! P7: the comparison of two versions of a 6,000-line file takes 100 ms or less (`P7.compare`, `engine/compare_6000_lines`).
//!
//! The two versions are the largest YAML file of swimlane 1 of the target-scale fixtures and the same file with about 5% of
//! its lines changed (identical inputs would measure nothing): every third item's `weight` changes, every fiftieth item loses
//! its `enabled` line, and every fiftieth item (half a period later) gains a `note` line. The work measured is all of what a
//! compare answers with: both sides decoded, the line diff in hunks, and the setting changes from both sides parsed and
//! flattened (the hub has the flattened settings in the settings index, so this is the pessimistic case).
//!
//! Since T9 the line diff carries the inline (word-level) change ranges of the replaced lines, so they are in the number.
//!
//! The result is checked on every iteration, so a change that makes the work disappear shows up as a failure, not as a fast
//! number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use engine::{Limits, Syntax, decode_text, flatten, parse, semantic_diff, text_diff};

/// A deterministic edit of about 5% of the lines, keeping the file's own line endings.
fn mutate(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len() + text.len() / 20);
    let mut weights = 0_usize;
    let mut items = 0_usize;
    let mut edits = 0_usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let eol = &line[trimmed.len()..];
        if let Some(rest) = trimmed.strip_prefix("    weight: ") {
            weights += 1;
            if weights % 3 == 0 {
                let n: u64 = rest.trim().parse().unwrap();
                write!(out, "    weight: {}{eol}", n + 1_000).unwrap();
                edits += 1;
                continue;
            }
        }
        if trimmed.starts_with("  item-") {
            items += 1;
        }
        if trimmed.starts_with("    enabled: ") && items % 50 == 0 {
            edits += 1;
            continue;
        }
        out.push_str(line);
        if trimmed.starts_with("    name: ") && items % 50 == 25 {
            write!(out, "    note: added by the benchmark{eol}").unwrap();
            edits += 1;
        }
    }
    (out, edits)
}

fn compare(c: &mut Criterion) {
    let (path, left_text) = common::big_yaml();
    let (right_text, edits) = mutate(&left_text);
    let lines = left_text.lines().count();
    assert!(
        edits * 100 / lines >= 3 && edits * 100 / lines <= 8,
        "{}: {edits} of {lines} lines edited: not about 5%",
        path.display()
    );
    let left = left_text.into_bytes();
    let right = right_text.into_bytes();
    let limits = Limits::default();

    let run = || {
        let l = decode_text(&left, &limits).unwrap();
        let r = decode_text(&right, &limits).unwrap();
        let diff = text_diff(&l.text, &r.text, &limits);
        let lf = flatten(&parse(&l.text, Syntax::Yaml, &limits).unwrap(), &limits).unwrap();
        let rf = flatten(&parse(&r.text, Syntax::Yaml, &limits).unwrap(), &limits).unwrap();
        let changes = semantic_diff(&lf.settings, &rf.settings);
        (diff, changes)
    };
    let (diff, changes) = run();
    let (hunks, setting_changes) = (diff.hunks.len(), changes.len());
    assert!(!diff.too_large && !diff.eol_only && hunks > 100, "{hunks} hunks");
    // The inline (word-level) ranges are part of what is measured (T9): the replaced `weight` lines have them.
    let inline = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| !l.inline_changes.is_empty())
        .count();
    assert!(
        inline > 100,
        "{inline} lines with inline ranges: the benchmark must run with them on"
    );
    assert!(setting_changes > 100, "{setting_changes} setting changes");

    let mut group = c.benchmark_group("engine");
    group.bench_function("compare_6000_lines", |b| {
        b.iter(|| {
            let (diff, changes) = black_box(&run)();
            assert_eq!(diff.hunks.len(), hunks);
            assert_eq!(changes.len(), setting_changes);
            black_box((diff, changes))
        });
    });
    group.finish();
}

criterion_group!(benches, compare);
criterion_main!(benches);
