//! T5 (D65, P7): the comparison grid, one logical file across the swimlanes the user picks. Rows are setting paths, columns are
//! swimlanes, a cell is a value or "absent". The inputs are effective configs that were flattened once, already.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use bytes::Bytes;
use domain::{ChannelName, FileClass, LineRange, MergeMode, NfsPath, SettingPath, SwimlaneId};
use engine::{
    Effective, GridColumn, GridError, GridOptions, GridResult, Layer, Layers, Limits, effective, grid,
    normalised_hash,
};
use proptest::prelude::*;

/// A swimlane and its effective config, kept together so that `GridColumn` can borrow both.
struct Col {
    swimlane: SwimlaneId,
    effective: Option<Effective>,
}

fn whole_file(text: &str) -> Effective {
    let settings = support::yaml(text).settings;
    let path = NfsPath::parse_prefix("svc/app.yml").unwrap();
    let hash = normalised_hash(&Bytes::copy_from_slice(text.as_bytes()), FileClass::Structured);
    effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: None,
            base: Some(Layer {
                path: &path,
                hash,
                settings: &settings,
            }),
            root: None,
        },
    )
    .unwrap()
}

fn spring(text: &str) -> Effective {
    let settings = support::yaml(text).settings;
    let path = NfsPath::parse_prefix("svc/app.yml").unwrap();
    let hash = normalised_hash(&Bytes::copy_from_slice(text.as_bytes()), FileClass::Structured);
    effective(
        MergeMode::SpringMerge,
        &Layers {
            tenant: None,
            base: Some(Layer {
                path: &path,
                hash,
                settings: &settings,
            }),
            root: None,
        },
    )
    .unwrap()
}

fn col(name: &str, text: Option<&str>) -> Col {
    Col {
        swimlane: SwimlaneId::parse(name).unwrap(),
        effective: text.map(whole_file),
    }
}

fn columns(cols: &[Col]) -> Vec<GridColumn<'_>> {
    cols.iter()
        .map(|c| GridColumn {
            swimlane: &c.swimlane,
            effective: c.effective.as_ref(),
        })
        .collect()
}

fn options(only_differing: bool) -> GridOptions<'static> {
    GridOptions {
        channel: None,
        only_differing,
        after: None,
        max_rows: 10_000,
    }
}

fn run<'a>(cols: &'a [Col], opts: &GridOptions<'_>) -> GridResult<'a> {
    grid(&columns(cols), opts, &Limits::default()).unwrap()
}

/// One line per row: `path: v1 | v2 | -` where `-` is absent.
fn show(result: &GridResult<'_>) -> Vec<String> {
    result
        .rows
        .iter()
        .map(|row| {
            let cells: Vec<String> = row
                .cells
                .iter()
                .map(|c| c.as_ref().map_or("-".to_owned(), |c| c.value.text.clone()))
                .collect();
            format!("{}: {}", row.path, cells.join(" | "))
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn only_differing_is_what_the_default_shows() {
    let cols = [
        col("sl-1", Some("same: 1\nvaries: a\nonly1: x\n")),
        col("sl-2", Some("same: 1\nvaries: b\n")),
        col("sl-3", Some("same: 1\nvaries: a\nonly3: y\n")),
    ];
    let r = run(&cols, &options(true));
    assert_eq!(
        show(&r),
        ["only1: x | - | -", "only3: - | - | y", "varies: a | b | a"]
    );
    assert!(!r.truncated);
    let names: Vec<&str> = r.swimlanes.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        ["sl-1", "sl-2", "sl-3"],
        "columns stay in the order asked for"
    );
    // All rows, with the settings that agree.
    let all = run(&cols, &options(false));
    assert_eq!(
        show(&all),
        [
            "only1: x | - | -",
            "only3: - | - | y",
            "same: 1 | 1 | 1",
            "varies: a | b | a"
        ]
    );
}

#[test]
fn cells_carry_the_lines_each_swimlane_writes_the_value_on() {
    let cols = [
        col("sl-1", Some("a: 1\nb: 2\n")),
        col("sl-2", Some("# note\n\nb: 3\na: 1\n")),
    ];
    let r = run(&cols, &options(false));
    let line = |row: usize, cell: usize| {
        let l = r.rows[row].cells[cell].as_ref().unwrap().lines;
        (l.start(), l.end())
    };
    assert_eq!(r.rows[0].path.as_str(), "a");
    assert_eq!((line(0, 0), line(0, 1)), ((1, 1), (4, 4)));
    assert_eq!(r.rows[1].path.as_str(), "b");
    assert_eq!((line(1, 0), line(1, 1)), ((2, 2), (3, 3)));
}

#[test]
fn identical_swimlanes_short_circuit() {
    // Three swimlanes with the same effective hash: nothing differs, and the answer comes from the hashes alone. The second
    // column's settings are different from the first's on purpose, to show that they are not read.
    let a = whole_file("x: 1\ny: 2\n");
    let mut b = a.clone();
    b.settings.truncate(1);
    let mut c = a.clone();
    c.settings.clear();
    assert_eq!(a.hash, b.hash);
    let swimlanes: Vec<SwimlaneId> = ["sl-1", "sl-2", "sl-3"]
        .iter()
        .map(|s| SwimlaneId::parse(s).unwrap())
        .collect();
    let cols = [
        GridColumn {
            swimlane: &swimlanes[0],
            effective: Some(&a),
        },
        GridColumn {
            swimlane: &swimlanes[1],
            effective: Some(&b),
        },
        GridColumn {
            swimlane: &swimlanes[2],
            effective: Some(&c),
        },
    ];
    let r = grid(&cols, &options(true), &Limits::default()).unwrap();
    assert!(r.rows.is_empty() && !r.truncated);
    assert_eq!(r.swimlanes.len(), 3);
    // Asking for every row does read them: each cell is its own column's.
    let r = grid(&cols, &options(false), &Limits::default()).unwrap();
    assert_eq!(show(&r), ["x: 1 | 1 | -", "y: 2 | - | -"]);
}

#[test]
fn same_settings_written_on_other_lines_are_not_a_difference() {
    // Two merged configs with the same settings have the same hash, whatever lines they are on.
    let a = spring("x: 1\ny: 2\n");
    let b = spring("# comment\nx: 1\n\ny: 2\n");
    assert_eq!(a.hash, b.hash);
    let (s1, s2) = (
        SwimlaneId::parse("sl-1").unwrap(),
        SwimlaneId::parse("sl-2").unwrap(),
    );
    let cols = [
        GridColumn {
            swimlane: &s1,
            effective: Some(&a),
        },
        GridColumn {
            swimlane: &s2,
            effective: Some(&b),
        },
    ];
    assert!(
        grid(&cols, &options(true), &Limits::default())
            .unwrap()
            .rows
            .is_empty()
    );
    let all = grid(&cols, &options(false), &Limits::default()).unwrap();
    let lines = |row: usize, cell: usize| all.rows[row].cells[cell].as_ref().unwrap().lines;
    assert_eq!(lines(0, 0), LineRange::new(1, 1).unwrap());
    assert_eq!(
        lines(0, 1),
        LineRange::new(2, 2).unwrap(),
        "the column's own lines"
    );
}

#[test]
fn absent_cells() {
    // A swimlane that lacks the file has an absent cell in every row, and so every row differs.
    let cols = [
        col("sl-1", Some("a: 1\nb: 2\n")),
        col("sl-2", None),
        col("sl-3", Some("a: 1\nb: 2\n")),
    ];
    let r = run(&cols, &options(true));
    assert_eq!(show(&r), ["a: 1 | - | 1", "b: 2 | - | 2"]);
    // Nobody has the file: no rows at all.
    let none = [col("sl-1", None), col("sl-2", None)];
    assert!(run(&none, &options(false)).rows.is_empty());
    // Everyone has an empty file: no rows.
    let empty = [col("sl-1", Some("")), col("sl-2", Some("# only a comment\n"))];
    assert!(run(&empty, &options(false)).rows.is_empty());
}

#[test]
fn a_yes_and_a_true_are_different_cells() {
    let cols = [
        col("sl-1", Some("flag: true\n")),
        col("sl-2", Some("flag: yes\n")),
    ];
    let r = run(&cols, &options(true));
    assert_eq!(show(&r), ["flag: true | yes"]);
    assert!(r.rows[0].cells[1].as_ref().unwrap().value.flag.is_some());
}

#[test]
fn channel_filter() {
    let text = |extra: &str| {
        format!(
            "options:\n  remote-itm-teller:\n    sorting: {extra}\n  remote-itm-teller-2:\n    sorting: {extra}\n  other:\n    sorting: {extra}\nlist:\n  - remote-itm-teller\nquoted:\n  \"a.b\":\n    remote-itm-teller: {extra}\n"
        )
    };
    let cols = [col("sl-1", Some(&text("on"))), col("sl-2", Some(&text("off")))];
    let channel = ChannelName::parse("remote-itm-teller").unwrap();
    let mut opts = options(true);
    opts.channel = Some(&channel);
    let r = run(&cols, &opts);
    let paths: Vec<&str> = r.rows.iter().map(|row| row.path.as_str()).collect();
    // The channel is a whole name in the path: the segment `remote-itm-teller`, not `remote-itm-teller-2`, and a key of a
    // quoted segment counts too. A value that says the name is not a path.
    assert_eq!(
        paths,
        [
            "options.remote-itm-teller.sorting",
            "quoted[\"a.b\"].remote-itm-teller"
        ]
    );
    // Without a filter every differing row is there.
    assert_eq!(run(&cols, &options(true)).rows.len(), 4);
    // The filter applies to unchanged rows too, when they are asked for.
    let same = [col("sl-1", Some(&text("on"))), col("sl-2", Some(&text("on")))];
    let mut all = options(false);
    all.channel = Some(&channel);
    assert_eq!(run(&same, &all).rows.len(), 2);
}

#[test]
fn keyset_paging_with_after_and_max_rows() {
    let mut a = String::new();
    let mut b = String::new();
    for i in 0..10 {
        writeln!(a, "k{i}: 1").unwrap();
        writeln!(b, "k{i}: 2").unwrap();
    }
    let cols = [col("sl-1", Some(&a)), col("sl-2", Some(&b))];
    let mut opts = options(true);
    opts.max_rows = 4;
    let first = run(&cols, &opts);
    assert_eq!(first.rows.len(), 4);
    assert!(first.truncated, "more rows exist");
    assert_eq!(first.rows[3].path.as_str(), "k3");
    // The next page starts after the last path of this one.
    let after = first.rows[3].path;
    opts.after = Some(after);
    let second = run(&cols, &opts);
    assert_eq!(second.rows[0].path.as_str(), "k4");
    assert_eq!(second.rows.len(), 4);
    assert!(second.truncated);
    let after = second.rows[3].path;
    opts.after = Some(after);
    let third = run(&cols, &opts);
    assert_eq!(third.rows.len(), 2);
    assert!(!third.truncated, "the last page");
    // Exactly full is not truncated.
    opts.after = None;
    opts.max_rows = 10;
    let all = run(&cols, &opts);
    assert_eq!(all.rows.len(), 10);
    assert!(!all.truncated);
    // The rows of the pages are the rows of the whole, in order.
    let paged: Vec<&SettingPath> = first
        .rows
        .iter()
        .chain(&second.rows)
        .chain(&third.rows)
        .map(|r| r.path)
        .collect();
    let whole: Vec<&SettingPath> = all.rows.iter().map(|r| r.path).collect();
    assert_eq!(paged, whole);
}

#[test]
fn max_rows_is_capped_by_the_limits() {
    let mut a = String::new();
    let mut b = String::new();
    for i in 0..20 {
        writeln!(a, "k{i:02}: 1").unwrap();
        writeln!(b, "k{i:02}: 2").unwrap();
    }
    let cols = [col("sl-1", Some(&a)), col("sl-2", Some(&b))];
    let limits = Limits {
        settings: 5,
        ..Limits::default()
    };
    let mut opts = options(true);
    opts.max_rows = 1_000_000;
    let r = grid(&columns(&cols), &opts, &limits).unwrap();
    assert_eq!(r.rows.len(), 5);
    assert!(r.truncated);
}

#[test]
fn input_that_is_not_sorted_is_refused() {
    let mut e = whole_file("a: 1\nb: 2\nc: 3\n");
    e.settings.swap(0, 2);
    let good = whole_file("a: 1\nb: 5\nc: 3\n");
    let (s1, s2) = (
        SwimlaneId::parse("sl-1").unwrap(),
        SwimlaneId::parse("sl-2").unwrap(),
    );
    let cols = [
        GridColumn {
            swimlane: &s1,
            effective: Some(&good),
        },
        GridColumn {
            swimlane: &s2,
            effective: Some(&e),
        },
    ];
    assert_eq!(
        grid(&cols, &options(true), &Limits::default()).unwrap_err(),
        GridError::Unsorted
    );
}

#[test]
fn at_most_64_swimlanes() {
    let cols: Vec<Col> = (0..65).map(|i| col(&format!("sl-{i}"), Some("a: 1\n"))).collect();
    assert_eq!(
        grid(&columns(&cols), &options(true), &Limits::default()).unwrap_err(),
        GridError::TooManyColumns
    );
    let cols: Vec<Col> = (0..64).map(|i| col(&format!("sl-{i}"), Some("a: 1\n"))).collect();
    assert!(grid(&columns(&cols), &options(true), &Limits::default()).is_ok());
    // No swimlane at all is an empty grid.
    let none = grid(&[], &options(true), &Limits::default()).unwrap();
    assert!(none.rows.is_empty() && none.swimlanes.is_empty());
}

#[test]
fn the_same_input_gives_the_same_grid() {
    let cols = [
        col("sl-1", Some("a: 1\nb: 2\n")),
        col("sl-2", Some("a: 2\nc: 3\n")),
        col("sl-3", None),
    ];
    assert_eq!(run(&cols, &options(true)), run(&cols, &options(true)));
}

proptest! {
    /// The grid is what a naive scan of every path says: a row for each path in the union (or each that differs), and in each
    /// row the column's value or nothing.
    #[test]
    fn grid_equals_the_naive_scan_prop(
        files in prop::collection::vec(prop::collection::btree_map(0_u8..8, 0_u8..3, 0..8), 1..6),
        only_differing in any::<bool>(),
    ) {
        let texts: Vec<String> = files
            .iter()
            .map(|m| m.iter().fold(String::new(), |mut s, (k, v)| { writeln!(s, "k{k}: {v}").unwrap(); s }))
            .collect();
        let cols: Vec<Col> = texts.iter().enumerate().map(|(i, t)| col(&format!("sl-{i}"), Some(t))).collect();
        let r = run(&cols, &options(only_differing));

        let union: BTreeSet<u8> = files.iter().flat_map(|m| m.keys().copied()).collect();
        let mut expected: Vec<String> = Vec::new();
        for k in union {
            let cells: Vec<Option<u8>> = files.iter().map(|m| m.get(&k).copied()).collect();
            let differs = cells.iter().any(|c| *c != cells[0]);
            if differs || !only_differing {
                let shown: Vec<String> = cells.iter().map(|c| c.map_or("-".to_owned(), |v| v.to_string())).collect();
                expected.push(format!("k{k}: {}", shown.join(" | ")));
            }
        }
        prop_assert_eq!(show(&r), expected);
        prop_assert!(!r.truncated);
    }
}
