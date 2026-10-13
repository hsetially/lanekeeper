//! T9 (D76, P7): the changed character ranges inside a replaced line. A removed line and the added line that replaces it are
//! split into identifier runs, punctuation and whitespace, so that a YAML key and its value change separately; the tokens are
//! diffed, and the changed ones come back as byte ranges into the line's text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use domain::{DiffLine, DiffLineKind};
use engine::{Limits, text_diff};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

fn lines(left: &str, right: &str, limits: &Limits) -> Vec<DiffLine> {
    text_diff(left, right, limits)
        .hunks
        .into_iter()
        .flat_map(|h| h.lines)
        .collect()
}

/// The text the ranges of a line mark, piece by piece.
fn marked(line: &DiffLine) -> Vec<&str> {
    line.inline_changes
        .iter()
        .map(|r| &line.text[r.start as usize..r.end as usize])
        .collect()
}

/// The first removed and first added line of a one-change diff, as (text, marked pieces).
fn first_pair(left: &str, right: &str) -> ((String, Vec<String>), (String, Vec<String>)) {
    let all = lines(left, right, &Limits::default());
    let removed = all.iter().find(|l| l.kind == DiffLineKind::Removed).unwrap();
    let added = all.iter().find(|l| l.kind == DiffLineKind::Added).unwrap();
    let own = |l: &DiffLine| (l.text.clone(), marked(l).into_iter().map(str::to_owned).collect());
    (own(removed), own(added))
}

#[test]
fn golden_word_change_in_value() {
    let ((old, old_marked), (new, new_marked)) = first_pair(
        "image: registry.example/app:1.2.3\n",
        "image: registry.example/app:1.2.4\n",
    );
    assert_eq!(old, "image: registry.example/app:1.2.3");
    assert_eq!(new, "image: registry.example/app:1.2.4");
    assert_eq!(old_marked, ["3"], "only the last version number changed");
    assert_eq!(new_marked, ["4"]);

    let ((_, old_marked), (_, new_marked)) = first_pair("mode: strict\n", "mode: lenient\n");
    assert_eq!(old_marked, ["strict"]);
    assert_eq!(new_marked, ["lenient"]);
}

#[test]
fn golden_key_vs_value() {
    // The key and the value change; the colon and the space between them are shared, so they are two ranges, not one.
    let ((_, old_marked), (_, new_marked)) = first_pair("  timeout-ms: 30\n", "  timeout-seconds: 45\n");
    assert_eq!(old_marked, ["ms", "30"]);
    assert_eq!(new_marked, ["seconds", "45"]);

    // Only the key changes.
    let ((_, old_marked), (_, new_marked)) = first_pair("old-name: 7\n", "new-name: 7\n");
    assert_eq!(old_marked, ["old"]);
    assert_eq!(new_marked, ["new"]);

    // An underscore belongs to the identifier: `max_retries` is one word, so it is marked whole.
    let ((_, old_marked), (_, new_marked)) = first_pair("max_retries: 7\n", "max_attempts: 7\n");
    assert_eq!(old_marked, ["max_retries"]);
    assert_eq!(new_marked, ["max_attempts"]);

    // Only the indentation changes: the whitespace run is the change.
    let ((_, old_marked), (_, new_marked)) = first_pair("  key: v\n", "    key: v\n");
    assert_eq!(old_marked, ["  "]);
    assert_eq!(new_marked, ["    "]);
}

#[test]
fn changed_words_with_only_whitespace_between_them_are_one_range() {
    let ((_, old_marked), (_, new_marked)) = first_pair("k: red green blue\n", "k: cyan pink blue\n");
    assert_eq!(old_marked, ["red green"]);
    assert_eq!(new_marked, ["cyan pink"]);
}

#[test]
fn context_lines_and_lines_without_a_partner_have_no_ranges() {
    let left = "a: 1\nb: 2\nc: 3\nd: 4\ne: 5\n";
    let right = "a: 1\nb: 2\nc: 9\nextra: 1\nmore: 2\nd: 4\ne: 5\n";
    let all = lines(left, right, &Limits::default());
    for line in &all {
        if line.kind == DiffLineKind::Context {
            assert!(line.inline_changes.is_empty(), "{line:?}");
        }
    }
    let added: Vec<_> = all.iter().filter(|l| l.kind == DiffLineKind::Added).collect();
    assert_eq!(added.len(), 3);
    // The first added line pairs with the removed one; the two after it have nobody to be compared with.
    assert_eq!(marked(added[0]), ["9"]);
    assert!(added[1].inline_changes.is_empty() && added[2].inline_changes.is_empty());
}

#[test]
fn a_line_that_only_lost_its_final_newline_has_no_ranges() {
    let all = lines("a: 1\nb: 2\n", "a: 1\nb: 2", &Limits::default());
    assert!(all.iter().any(|l| l.kind == DiffLineKind::Removed));
    assert!(all.iter().all(|l| l.inline_changes.is_empty()), "{all:?}");
}

#[test]
fn skipped_above_line_cap() {
    // A line of exactly the cap is compared; one byte more is not, and the hunk is there either way.
    let make = |n: usize, last: char| format!("k: {}{last}\n", "x".repeat(n - 4));
    let limits = Limits {
        inline_line_bytes: 40,
        ..Limits::default()
    };
    let at_cap = lines(&make(40, '1'), &make(40, '2'), &limits);
    let removed = at_cap.iter().find(|l| l.kind == DiffLineKind::Removed).unwrap();
    assert_eq!(removed.text.len(), 40);
    assert_eq!(marked(removed).len(), 1, "a line of exactly the cap gets ranges");

    let over_cap = lines(&make(41, '1'), &make(41, '2'), &limits);
    assert_eq!(
        over_cap
            .iter()
            .filter(|l| l.kind != DiffLineKind::Context)
            .count(),
        2
    );
    assert!(
        over_cap.iter().all(|l| l.inline_changes.is_empty()),
        "{over_cap:?}"
    );

    // One long side is enough to skip the pair.
    let mixed = lines(&make(41, '1'), &make(10, '2'), &limits);
    assert!(mixed.iter().all(|l| l.inline_changes.is_empty()), "{mixed:?}");
}

#[test]
fn work_is_bounded_by_the_diff_cell_budget() {
    // Two lines with no token in common cost about (tokens)^2 cells. With a budget below that they get no ranges, and the
    // hunks are the same as ever; with the default budget they get ranges.
    let left = format!(
        "{}\n",
        (0..20).map(|i| format!("a{i}")).collect::<Vec<_>>().join(" ")
    );
    let right = format!(
        "{}\n",
        (0..20).map(|i| format!("b{i}")).collect::<Vec<_>>().join(" ")
    );
    let tight = Limits {
        diff_cells: 100,
        ..Limits::default()
    };
    let none = text_diff(&left, &right, &tight);
    assert!(!none.too_large, "the line diff itself is cheap");
    assert_eq!(none.hunks.len(), 1);
    assert!(none.hunks[0].lines.iter().all(|l| l.inline_changes.is_empty()));

    let some = text_diff(&left, &right, &Limits::default());
    assert!(some.hunks[0].lines.iter().any(|l| !l.inline_changes.is_empty()));
    // Everything but the ranges is the same.
    let strip = |d: &engine::TextDiff| {
        let mut d = d.clone();
        for line in d.hunks.iter_mut().flat_map(|h| &mut h.lines) {
            line.inline_changes.clear();
        }
        d
    };
    assert_eq!(strip(&none), strip(&some));
}

#[test]
fn ranges_are_on_character_boundaries_for_non_ascii_text() {
    let all = lines(
        "name: caf\u{e9} \u{1F600}\n",
        "name: caf\u{e8} \u{1F601}\n",
        &Limits::default(),
    );
    let changed: Vec<_> = all.iter().filter(|l| l.kind != DiffLineKind::Context).collect();
    assert_eq!(changed.len(), 2);
    for line in changed {
        assert!(!line.inline_changes.is_empty());
        for r in &line.inline_changes {
            assert!(line.text.is_char_boundary(r.start as usize), "{line:?}");
            assert!(line.text.is_char_boundary(r.end as usize), "{line:?}");
        }
    }
}

fn line_text() -> impl Strategy<Value = String> {
    // Words, punctuation, whitespace and a few multi-byte characters: what YAML lines are made of.
    proptest::collection::vec(
        prop_oneof![
            "[a-z0-9_]{1,6}",
            Just(" ".to_owned()),
            Just("  ".to_owned()),
            Just(":".to_owned()),
            Just("-".to_owned()),
            Just(".".to_owned()),
            Just("\u{e9}".to_owned()),
            Just("\u{1F600}".to_owned()),
            Just("${X}".to_owned()),
        ],
        0..14,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The ranges are sorted, do not touch, are not empty, lie inside the line on character boundaries, and what they leave
    /// out is the same on both sides once whitespace is ignored (a range may swallow whitespace between two changed words).
    #[test]
    fn ranges_are_well_formed_and_leave_the_same_text_prop(a in line_text(), b in line_text()) {
        let left = format!("head\n{a}\ntail\n");
        let right = format!("head\n{b}\ntail\n");
        let all = lines(&left, &right, &Limits::default());
        let rest = |line: &DiffLine| -> Result<String, TestCaseError> {
            let mut out = String::new();
            let mut at = 0_usize;
            for r in &line.inline_changes {
                let (s, e) = (r.start as usize, r.end as usize);
                prop_assert!(at <= s && s < e && e <= line.text.len(), "{:?}", line);
                prop_assert!(line.text.is_char_boundary(s) && line.text.is_char_boundary(e));
                out.push_str(&line.text[at..s]);
                at = e;
            }
            out.push_str(&line.text[at..]);
            Ok(out.chars().filter(|c| !c.is_whitespace()).collect())
        };
        let removed: Vec<_> = all.iter().filter(|l| l.kind == DiffLineKind::Removed).collect();
        let added: Vec<_> = all.iter().filter(|l| l.kind == DiffLineKind::Added).collect();
        if removed.len() == 1 && added.len() == 1 {
            let (r, d): (String, String) = (rest(removed[0])?, rest(added[0])?);
            prop_assert_eq!(r, d);
        }
        for line in all.iter().filter(|l| l.kind == DiffLineKind::Context) {
            prop_assert!(line.inline_changes.is_empty());
        }
    }

    /// Two runs on the same input give the same ranges.
    #[test]
    fn ranges_are_deterministic_prop(a in line_text(), b in line_text()) {
        let (left, right) = (format!("{a}\n"), format!("{b}\n"));
        prop_assert_eq!(
            text_diff(&left, &right, &Limits::default()),
            text_diff(&left, &right, &Limits::default())
        );
    }
}

/// The hunk lines with the changed ranges in square brackets, `-` for removed and `+` for added: the form the golden uses.
fn render(left: &str, right: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for line in lines(left, right, &Limits::default()) {
        let mark = match line.kind {
            DiffLineKind::Context => ' ',
            DiffLineKind::Added => '+',
            DiffLineKind::Removed => '-',
        };
        let mut text = String::new();
        let mut at = 0;
        for r in &line.inline_changes {
            write!(
                text,
                "{}[{}]",
                &line.text[at..r.start as usize],
                &line.text[r.start as usize..r.end as usize]
            )
            .unwrap();
            at = r.end as usize;
        }
        text.push_str(&line.text[at..]);
        writeln!(out, "{mark}{text}").unwrap();
    }
    out
}

#[test]
fn golden_inline_ranges_of_a_realistic_change() {
    let left = "\
spring:
  datasource:
    url: jdbc:postgresql://db-sit1.internal:5432/core
    max_pool: 10
  profiles: sit1,default
logging:
  level:
    root: INFO
";
    let right = "\
spring:
  datasource:
    url: jdbc:postgresql://db-sit2.internal:5432/core
    max_pool: 25
  profiles: sit2,default
logging:
  level:
    root: DEBUG
    com.example: TRACE
";
    insta::assert_snapshot!("inline_realistic_change", render(left, right));
}
