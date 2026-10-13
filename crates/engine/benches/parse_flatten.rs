//! P6: parse and flatten of the 6,000-line YAML file takes 20 ms or less (`P6.parse_flatten`,
//! `engine/parse_flatten_6000_lines`).
//!
//! The largest YAML file of swimlane 1 of the target-scale fixtures: merge keys and aliases in every item, so the work is
//! the realistic mix of events, a tree, alias expansion and path building. The result is checked on every iteration, so a
//! change that makes the work disappear shows up as a failure, not as a fast number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use engine::{Limits, Syntax, flatten, parse};

fn parse_flatten(c: &mut Criterion) {
    let (path, text) = common::big_yaml();
    let limits = Limits::default();
    let expected = {
        let flat = flatten(&parse(&text, Syntax::Yaml, &limits).unwrap(), &limits).unwrap();
        assert!(
            flat.settings.len() > 5_000,
            "{}: {} settings",
            path.display(),
            flat.settings.len()
        );
        flat.settings.len()
    };

    let mut group = c.benchmark_group("engine");
    group.bench_function("parse_flatten_6000_lines", |b| {
        b.iter(|| {
            let parsed = parse(black_box(&text), Syntax::Yaml, &limits).unwrap();
            let flat = flatten(&parsed, &limits).unwrap();
            assert_eq!(flat.settings.len(), expected);
            black_box(flat)
        });
    });
    group.finish();
}

criterion_group!(benches, parse_flatten);
criterion_main!(benches);
