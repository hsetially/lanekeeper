//! P7: the grid for one logical file across 40 swimlanes takes 150 ms or less on pre-flattened inputs (`P7.engine_grid`,
//! `engine/grid_40_swimlanes`).
//!
//! The logical file is the 6,000-line `reference-data/catalog-big-01.yml` of the target-scale fixtures. Each swimlane has the
//! file as its base copy or as its tenant copy (`catalog-big-01-<tenant>.yml`) when it has one, which is how the effective
//! file of a swimlane is chosen. Reading, parsing and flattening the 40 files is set-up and is not timed: the grid is given
//! effective configs that were flattened once, as the hub holds them.
//!
//! One iteration is both views the UI asks for: the default (only the rows that differ) and every row, so the budget covers
//! the heavier of the two. The result is checked on every iteration, so a change that makes the work disappear shows up as a
//! failure, not as a fast number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::hint::black_box;

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use domain::{FileClass, MergeMode, NfsPath, SwimlaneId};
use engine::{
    Effective, GridColumn, GridOptions, Layer, Layers, Limits, Syntax, decode_text, effective, flatten, grid,
    normalised_hash, parse,
};

const LOGICAL: &str = "reference-data/catalog-big-01.yml";

/// The effective config of the logical file in one swimlane directory, or `None` when the swimlane lacks it.
fn effective_of(dir: &std::path::Path, limits: &Limits) -> Option<Effective> {
    let folder = dir.join("reference-data");
    // The tenant copy, if there is one: `catalog-big-01-<tenant>.yml`.
    let mut tenant: Option<std::path::PathBuf> = None;
    for entry in fs::read_dir(&folder).ok()? {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("catalog-big-01-") && p.extension().is_some_and(|e| e == "yml") {
            tenant = Some(p);
        }
    }
    let chosen = tenant.unwrap_or_else(|| folder.join("catalog-big-01.yml"));
    let data = Bytes::from(fs::read(chosen).ok()?);
    let text = decode_text(&data, limits).unwrap().text.into_owned();
    let settings = flatten(&parse(&text, Syntax::Yaml, limits).unwrap(), limits)
        .unwrap()
        .settings;
    let path = NfsPath::parse_prefix(LOGICAL).unwrap();
    effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: None,
            base: Some(Layer {
                path: &path,
                hash: normalised_hash(&data, FileClass::Structured),
                settings: &settings,
            }),
            root: None,
        },
    )
}

fn grid_bench(c: &mut Criterion) {
    let limits = Limits::default();
    let nfs = common::fixtures().join("nfs");
    let mut dirs: Vec<_> = fs::read_dir(&nfs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 40, "the target scale has 40 swimlanes");
    let swimlanes: Vec<SwimlaneId> = dirs
        .iter()
        .map(|d| SwimlaneId::parse(&d.file_name().unwrap().to_string_lossy()).unwrap())
        .collect();
    let effectives: Vec<Option<Effective>> = dirs.iter().map(|d| effective_of(d, &limits)).collect();
    let columns: Vec<GridColumn<'_>> = swimlanes
        .iter()
        .zip(&effectives)
        .map(|(swimlane, e)| GridColumn {
            swimlane,
            effective: e.as_ref(),
        })
        .collect();
    assert!(
        columns
            .iter()
            .all(|c| c.effective.is_some_and(|e| e.settings.len() > 5_000)),
        "every swimlane has the 6,000-line file"
    );

    let differing = GridOptions {
        channel: None,
        only_differing: true,
        after: None,
        max_rows: limits.settings,
    };
    let every_row = GridOptions {
        only_differing: false,
        ..differing
    };
    let run = || {
        let d = grid(&columns, &differing, &limits).unwrap();
        let a = grid(&columns, &every_row, &limits).unwrap();
        (d, a)
    };
    let (d, a) = run();
    let (differing_rows, all_rows) = (d.rows.len(), a.rows.len());
    assert!(all_rows > 5_000, "{all_rows} rows");
    assert!(
        differing_rows > 0 && differing_rows < all_rows,
        "{differing_rows} of {all_rows} rows differ: the fixtures must have both kinds"
    );
    assert!(!d.truncated && !a.truncated);

    let mut group = c.benchmark_group("engine");
    group.bench_function("grid_40_swimlanes", |b| {
        b.iter(|| {
            let (d, a) = black_box(&run)();
            assert_eq!(d.rows.len(), differing_rows);
            assert_eq!(a.rows.len(), all_rows);
            black_box((d, a))
        });
    });
    group.finish();
}

criterion_group!(benches, grid_bench);
criterion_main!(benches);
