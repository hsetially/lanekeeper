//! P6, P7 (settings): the settings index of the 6,000-line YAML file takes 60 ms or less (`P6.index_settings`,
//! `engine/index_settings_6000_lines`). The plan proposed 40 ms; T3 measured 45.8 ms p95
//! and a human set 60 ms (`plans/04-fact-engine.blockers.md` has the measurements). The entry is registered.
//!
//! The same file as `parse_flatten`, end to end as the hub runs it for every unique blob: decode, parse, flatten, and then
//! tree-sitter reading the text a second time and being checked against the parser, setting by setting. The result is
//! checked on every iteration, so a change that makes tree-sitter give up (and so makes the run cheaper) shows up as a
//! failure and not as a fast number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use engine::{Limits, LocationSource, ParseWarning, Syntax, index_settings};

fn index(c: &mut Criterion) {
    let (path, text) = common::big_yaml();
    let bytes = text.into_bytes();
    let limits = Limits::default();
    let expected = {
        let index = index_settings(&bytes, Syntax::Yaml, &limits).unwrap();
        assert!(
            index.rows.len() > 5_000,
            "{}: {} settings",
            path.display(),
            index.rows.len()
        );
        assert_eq!(index.located_by, LocationSource::TreeSitter);
        index.rows.len()
    };

    let mut group = c.benchmark_group("engine");
    group.bench_function("index_settings_6000_lines", |b| {
        b.iter(|| {
            let index = index_settings(black_box(&bytes), Syntax::Yaml, &limits).unwrap();
            assert_eq!(index.rows.len(), expected);
            assert_eq!(index.located_by, LocationSource::TreeSitter);
            assert!(
                !index
                    .warnings
                    .iter()
                    .any(|w| matches!(w, ParseWarning::LocationMismatch { .. })),
                "tree-sitter disagreed with the parser"
            );
            black_box(index)
        });
    });
    group.finish();
}

criterion_group!(benches, index);
criterion_main!(benches);
