//! T5 (D65, D76, P7): the three kinds of diff. A text diff is hunks for a virtualised view, computed with Patience, with line
//! endings normalised first (a difference in line endings alone is flagged and hidden). A semantic diff is the settings that
//! differ. A binary comparison is hash and size.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;

use domain::{
    ChangeKind, ContentHash, DiffHunk, DiffLineKind, LineRange, MergeMode, NfsPath, SettingPath, ValueFlag,
    ValueType,
};
use engine::{
    BinaryFacts, Layer, Layers, Limits, SettingView, TextDiff, binary_compare, effective, semantic_diff,
    text_diff,
};
use proptest::prelude::*;

fn limits() -> Limits {
    Limits::default()
}

fn diff(left: &str, right: &str) -> TextDiff {
    text_diff(left, right, &limits())
}

/// A unified-diff rendering of the hunks, the form the golden snapshots use.
fn render(hunks: &[DiffHunk]) -> String {
    let mut out = String::new();
    for h in hunks {
        writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        )
        .unwrap();
        for line in &h.lines {
            let mark = match line.kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Removed => '-',
                DiffLineKind::Added => '+',
            };
            writeln!(out, "{mark}{}", line.text).unwrap();
        }
    }
    out
}

fn numbered(n: usize) -> String {
    (1..=n).fold(String::new(), |mut s, i| {
        writeln!(s, "line {i}").unwrap();
        s
    })
}

/// Applies `hunks` to `left` (git convention for the starts) and returns the result as lines.
fn apply(left: &str, hunks: &[DiffHunk]) -> Vec<String> {
    let lines: Vec<&str> = left.split_inclusive('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut at = 0_usize;
    for h in hunks {
        let start = if h.old_lines == 0 {
            h.old_start as usize
        } else {
            h.old_start as usize - 1
        };
        assert!(start >= at, "hunks overlap or are out of order");
        out.extend(lines[at..start].iter().map(|l| (*l).to_owned()));
        at = start;
        for line in &h.lines {
            match line.kind {
                DiffLineKind::Context => {
                    assert_eq!(lines[at].trim_end_matches('\n'), line.text, "context line");
                    out.push(lines[at].to_owned());
                    at += 1;
                }
                DiffLineKind::Removed => {
                    assert_eq!(lines[at].trim_end_matches('\n'), line.text, "removed line");
                    at += 1;
                }
                DiffLineKind::Added => out.push(format!("{}\n", line.text)),
            }
        }
    }
    out.extend(lines[at..].iter().map(|l| (*l).to_owned()));
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// Text diff.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn identical_texts_have_no_hunks() {
    let d = diff("a: 1\nb: 2\n", "a: 1\nb: 2\n");
    assert!(d.identical);
    assert!(!d.eol_only && !d.too_large);
    assert!(d.hunks.is_empty());
    assert!(diff("", "").identical);
}

#[test]
fn eol_only_hidden_but_flagged() {
    let d = diff("a: 1\r\nb: 2\r\n", "a: 1\nb: 2\n");
    assert!(!d.identical, "the bytes differ");
    assert!(d.eol_only, "and only the line endings do");
    assert!(d.hunks.is_empty(), "so there is nothing to show");
    assert!(!d.too_large);
    // A lone CR is a line ending too.
    let d = diff("a\rb\r", "a\nb\n");
    assert!(d.eol_only && d.hunks.is_empty());
}

#[test]
fn a_real_change_under_other_line_endings_is_a_change_and_the_hunks_have_no_cr() {
    let d = diff("a: 1\r\nb: 2\r\nc: 3\r\n", "a: 1\nb: 9\nc: 3\n");
    assert!(!d.identical && !d.eol_only);
    assert_eq!(d.hunks.len(), 1);
    for line in d.hunks.iter().flat_map(|h| &h.lines) {
        assert!(!line.text.contains('\r') && !line.text.contains('\n'), "{line:?}");
        // The inline ranges of a pair are `tests/inline.rs`'s; here they only have to stay inside the text.
        assert!(
            line.inline_changes
                .iter()
                .all(|r| r.start < r.end && r.end as usize <= line.text.len())
        );
    }
    let removed: Vec<_> = d.hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == DiffLineKind::Removed)
        .map(|l| l.text.as_str())
        .collect();
    assert_eq!(removed, ["b: 2"]);
}

#[test]
fn hunks_golden() {
    let left = "\
server:
  port: 8080
  host: localhost
  timeout: 30
logging:
  level: info
  file: /var/log/app.log
features:
  a: true
  b: false
  c: true
  d: true
  e: false
  f: true
  g: true
database:
  url: jdbc:postgresql://db/app
  pool: 10
";
    let right = "\
server:
  port: 8081
  host: localhost
  timeout: 30
logging:
  level: debug
  file: /var/log/app.log
features:
  a: true
  b: false
  c: true
  d: true
  e: false
  f: true
  g: true
database:
  url: jdbc:postgresql://db/app
  pool: 10
  max-lifetime: 600
";
    let d = diff(left, right);
    assert_eq!(d.hunks.len(), 2, "{}", render(&d.hunks));
    insta::assert_snapshot!("text_diff_hunks", render(&d.hunks));
}

#[test]
fn context_is_three_lines_and_close_changes_share_a_hunk() {
    let base = numbered(40);
    let change = |lines: &[usize]| {
        base.lines()
            .enumerate()
            .map(|(i, l)| {
                if lines.contains(&(i + 1)) {
                    format!("{l} changed\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect::<String>()
    };
    // One change: three lines of context on each side.
    let d = diff(&base, &change(&[20]));
    assert_eq!(d.hunks.len(), 1);
    let h = &d.hunks[0];
    assert_eq!(
        (h.old_start, h.old_lines, h.new_start, h.new_lines),
        (17, 7, 17, 7)
    );
    assert_eq!(h.lines.len(), 8, "3 + removed + added + 3");
    // Two changes with six unchanged lines between: the contexts touch, one hunk.
    assert_eq!(diff(&base, &change(&[20, 27])).hunks.len(), 1);
    // Seven unchanged lines between: two hunks.
    assert_eq!(diff(&base, &change(&[20, 28])).hunks.len(), 2);
    // A change on the first and the last line has only the context that exists.
    let d = diff(&base, &change(&[1, 40]));
    assert_eq!(d.hunks.len(), 2);
    assert_eq!((d.hunks[0].old_start, d.hunks[0].old_lines), (1, 4));
    assert_eq!((d.hunks[1].old_start, d.hunks[1].old_lines), (37, 4));
}

#[test]
fn insertion_into_an_empty_file_and_deletion_of_everything() {
    let d = diff("", "a\nb\n");
    assert_eq!(d.hunks.len(), 1);
    let h = &d.hunks[0];
    assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (0, 0, 1, 2));
    assert!(h.lines.iter().all(|l| l.kind == DiffLineKind::Added));
    let d = diff("a\nb\n", "");
    let h = &d.hunks[0];
    assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 2, 0, 0));
    assert!(h.lines.iter().all(|l| l.kind == DiffLineKind::Removed));
}

#[test]
fn a_missing_final_newline_is_a_difference_and_not_an_eol_difference() {
    let d = diff("a\nb", "a\nb\n");
    assert!(!d.identical && !d.eol_only);
    assert_eq!(d.hunks.len(), 1);
    // The last line is shown as replaced by itself: its text is the same and its terminator is not.
    let kinds: Vec<_> = d.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (DiffLineKind::Context, "a"),
            (DiffLineKind::Removed, "b"),
            (DiffLineKind::Added, "b"),
        ]
    );
}

#[test]
fn patience_aligns_on_unique_lines() {
    // Bram Cohen's example. One function is replaced by another and a line of the first is dropped. Many lines are alike
    // (`{`, `}`, a blank line), so an algorithm that does not anchor on the lines that occur once on each side pairs them
    // with the wrong ones: Myers shows `// Frobs foo heartily` and `int frobnitz` as removed and added again. Patience keeps
    // them, and shows the new function as one inserted block and the old one as one removed block.
    let old = r#"#include <stdio.h>

// Frobs foo heartily
int frobnitz(int foo)
{
    int i;
    for(i = 0; i < 10; i++)
    {
        printf("Your answer is: ");
        printf("%d\n", foo);
    }
}

int fact(int n)
{
    if(n > 1)
    {
        return fact(n-1) * n;
    }
    return 1;
}

int main(int argc, char **argv)
{
    frobnitz(fact(10));
}
"#;
    let new = r#"#include <stdio.h>

int fib(int n)
{
    if(n > 2)
    {
        return fib(n-1) + fib(n-2);
    }
    return 1;
}

// Frobs foo heartily
int frobnitz(int foo)
{
    int i;
    for(i = 0; i < 10; i++)
    {
        printf("%d\n", foo);
    }
}

int main(int argc, char **argv)
{
    frobnitz(fib(10));
}
"#;
    let d = diff(old, new);
    assert_eq!(d.hunks.len(), 1);
    let changed: Vec<(DiffLineKind, &str)> = d.hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind != DiffLineKind::Context)
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert!(
        !changed
            .iter()
            .any(|(_, t)| t.contains("Frobs foo heartily") || t.contains("int frobnitz")),
        "{}",
        render(&d.hunks)
    );
    insta::assert_snapshot!("patience_anchors_on_unique_lines", render(&d.hunks));
}

#[test]
fn too_large_is_deterministic() {
    let small = Limits {
        diff_lines: 10,
        ..Limits::default()
    };
    // At the limit: a diff. One over, on either side: the same outcome every time, and no hunks.
    let ten = numbered(10);
    let eleven = numbered(11);
    let at_limit = text_diff(&ten, &ten.replace("line 5", "line five"), &small);
    assert!(!at_limit.too_large);
    assert_eq!(at_limit.hunks.len(), 1);
    for (l, r) in [
        (&eleven, &ten),
        (&ten, &eleven),
        (&eleven, &eleven.replace("line 5", "x")),
    ] {
        let a = text_diff(l, r, &small);
        let b = text_diff(l, r, &small);
        assert_eq!(a, b);
        assert!(a.too_large && a.hunks.is_empty());
        assert!(!a.identical);
    }
    // Identical texts are identical however long they are.
    let same = text_diff(&eleven, &eleven, &small);
    assert!(same.identical && !same.too_large);
}

#[test]
fn a_diff_whose_work_would_be_quadratic_is_refused_before_it_starts() {
    let tight = Limits {
        diff_cells: 100,
        ..Limits::default()
    };
    // Two lines in turn, one text a step behind the other: every line has a partner, none occurs once (so there is nothing to
    // anchor on), and 20 x 20 lines is 400 cells.
    let alternating = |first: &str, second: &str| {
        (0..20).fold(String::new(), |mut s, i| {
            writeln!(s, "{}", if i % 2 == 0 { first } else { second }).unwrap();
            s
        })
    };
    let (ab, ba) = (alternating("a", "b"), alternating("b", "a"));
    let d = text_diff(&ab, &ba, &tight);
    assert!(d.too_large && d.hunks.is_empty() && !d.identical);
    // The same texts are fine under the default.
    assert!(!diff(&ab, &ba).too_large);
    // Lines that occur once in both texts are anchors, and they make a long pair of files cheap: the same lines in the
    // opposite order are one anchor and two plain stretches.
    let forward = numbered(20);
    let backward: String = forward.lines().rev().fold(String::new(), |mut s, l| {
        writeln!(s, "{l}").unwrap();
        s
    });
    assert!(!text_diff(&forward, &backward, &tight).too_large);
    // The common beginning and end do not count, and neither do the lines the other text lacks: one changed line in the
    // middle of long files costs nothing.
    let big = numbered(5_000);
    let d = text_diff(&big, &big.replace("line 2500\n", "changed\n"), &tight);
    assert!(!d.too_large);
    assert_eq!(d.hunks.len(), 1);
    // Two texts with nothing in common are one big change at no cost, however long: no line has a partner.
    let a: String = (0..6_000).fold(String::new(), |mut s, i| {
        writeln!(s, "a{i}").unwrap();
        s
    });
    let b = a.replace('a', "b");
    let d = text_diff(&a, &b, &tight);
    assert!(!d.too_large);
    assert_eq!(d.hunks.len(), 1);
    assert_eq!(d.hunks[0].lines.len(), 12_000);
    assert_eq!(apply(&a, &d.hunks).concat(), b);
}

#[test]
fn a_deletion_ahead_of_a_kept_line_numbers_both_sides_right() {
    // `similar` reports where a deletion falls in the new text, and where an insertion falls in the old text, inconsistently
    // for this input (a line deleted, then a kept line that is also inserted again). The header must not depend on it.
    let text = |v: &[u8]| {
        v.iter().fold(String::new(), |mut s, c| {
            writeln!(s, "l{c}").unwrap();
            s
        })
    };
    let left = text(&[1, 2, 0, 3, 3, 1, 3]);
    let right = text(&[2, 2, 0, 3]);
    let d = diff(&left, &right);
    assert_eq!(d.hunks.len(), 1);
    let h = &d.hunks[0];
    assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 7, 1, 4));
    assert_eq!(apply(&left, &d.hunks).concat(), right);
}

#[test]
fn a_6000_line_file_with_scattered_changes_gives_the_expected_hunks() {
    let left = numbered(6_000);
    // Every 50th line changes, one line is removed and one added.
    let mut right = String::new();
    for (i, line) in left.lines().enumerate() {
        let n = i + 1;
        if n == 1_001 {
            continue;
        }
        if n % 50 == 0 {
            writeln!(right, "{line} changed").unwrap();
        } else {
            writeln!(right, "{line}").unwrap();
        }
        if n == 3_001 {
            writeln!(right, "inserted").unwrap();
        }
    }
    let d = diff(&left, &right);
    assert_eq!(d.hunks.len(), 120, "6000 / 50 changes, each far from the next");
    assert_eq!(apply(&left, &d.hunks).concat(), right);
}

/// Every claim a diff makes, checked against the two texts and not against itself.
fn check(l: &str, r: &str) -> Result<(), TestCaseError> {
    let d = diff(l, r);
    prop_assert_eq!(d.identical, l == r);
    prop_assert!(!d.too_large);
    prop_assert_eq!(apply(l, &d.hunks).concat(), r.to_owned());
    // The counts in each header are the counts of its lines, and the hunks do not touch (they would have merged).
    let mut previous_end = 0_u32;
    for h in &d.hunks {
        let old = h.lines.iter().filter(|x| x.kind != DiffLineKind::Added).count();
        let new = h.lines.iter().filter(|x| x.kind != DiffLineKind::Removed).count();
        prop_assert_eq!(h.old_lines as usize, old);
        prop_assert_eq!(h.new_lines as usize, new);
        // Each side's lines are the lines of that text at the numbers in the header.
        let side = |text: &str, start: u32, keep: DiffLineKind| -> (Vec<String>, Vec<String>) {
            let all: Vec<&str> = text.lines().collect();
            let from = if start == 0 { 0 } else { start as usize - 1 };
            let theirs: Vec<String> = h
                .lines
                .iter()
                .filter(|x| x.kind != keep)
                .map(|x| x.text.clone())
                .collect();
            let ours: Vec<String> = all
                .iter()
                .skip(from)
                .take(theirs.len())
                .map(|s| (*s).to_owned())
                .collect();
            (ours, theirs)
        };
        let (ours, theirs) = side(l, h.old_start, DiffLineKind::Added);
        prop_assert_eq!(ours, theirs);
        let (ours, theirs) = side(r, h.new_start, DiffLineKind::Removed);
        prop_assert_eq!(ours, theirs);
        let start = if h.old_lines == 0 {
            h.old_start + 1
        } else {
            h.old_start
        };
        prop_assert!(start > previous_end, "hunks are in order and apart");
        previous_end = h.old_start + h.old_lines;
        // A hunk starts and ends with a change or with at most three lines of context.
        let lead = h
            .lines
            .iter()
            .take_while(|x| x.kind == DiffLineKind::Context)
            .count();
        let trail = h
            .lines
            .iter()
            .rev()
            .take_while(|x| x.kind == DiffLineKind::Context)
            .count();
        prop_assert!(lead <= 3 && trail <= 3);
        prop_assert!(h.lines.iter().any(|x| x.kind != DiffLineKind::Context));
    }
    Ok(())
}

proptest! {
    /// The hunks are a faithful description: applying them to the left text gives the right text. The alphabet is tiny so
    /// that lines repeat, which is what makes a diff hard.
    #[test]
    fn hunks_applied_to_the_left_give_the_right_prop(
        left in prop::collection::vec(0_u8..4, 0..60),
        right in prop::collection::vec(0_u8..4, 0..60),
    ) {
        let text = |v: &[u8]| v.iter().fold(String::new(), |mut s, c| { writeln!(s, "l{c}").unwrap(); s });
        check(&text(&left), &text(&right))?;
    }

    /// The same, for a text and an edited copy of it: a mixture of lines that occur once (the anchors) and lines that repeat,
    /// with a few edits, which is what the anchoring and the stretches between anchors are for.
    #[test]
    fn hunks_of_an_edited_copy_are_faithful_prop(
        lines in prop::collection::vec(0_u8..40, 20..300),
        edits in prop::collection::vec((any::<prop::sample::Index>(), 0_u8..3, 0_u8..40), 0..12),
    ) {
        // Every fifth line is unique; the rest come from a small alphabet.
        let original: Vec<String> = lines
            .iter()
            .enumerate()
            .map(|(i, c)| if i % 5 == 0 { format!("unique {i}") } else { format!("common {}", c % 8) })
            .collect();
        let mut edited = original.clone();
        for (at, action, c) in edits {
            let at = at.index(edited.len());
            match action {
                0 => { edited.remove(at); }
                1 => edited.insert(at, format!("new {c}")),
                _ => edited[at] = format!("common {}", c % 8),
            }
            if edited.is_empty() { edited.push("last".to_owned()); }
        }
        let join = |v: &[String]| v.iter().fold(String::new(), |mut s, l| { writeln!(s, "{l}").unwrap(); s });
        check(&join(&original), &join(&edited))?;
    }

    /// The same two texts give the same hunks, whatever line endings either is written with.
    #[test]
    fn line_endings_never_change_the_hunks_prop(
        left in prop::collection::vec(0_u8..4, 0..30),
        right in prop::collection::vec(0_u8..4, 0..30),
        crlf_left in any::<bool>(),
        crlf_right in any::<bool>(),
    ) {
        let text = |v: &[u8], eol: &str| v.iter().fold(String::new(), |mut s, c| { write!(s, "l{c}{eol}").unwrap(); s });
        let lf = diff(&text(&left, "\n"), &text(&right, "\n"));
        let mixed = diff(
            &text(&left, if crlf_left { "\r\n" } else { "\n" }),
            &text(&right, if crlf_right { "\r\n" } else { "\n" }),
        );
        prop_assert_eq!(&lf.hunks, &mixed.hunks);
        prop_assert!(!lf.eol_only);
        prop_assert_eq!(mixed.eol_only, left == right && (crlf_left != crlf_right) && !left.is_empty());
    }
}

proptest! {
    /// "Only the line endings differ" is the same statement as the engine's `normalise_eol` makes about two files.
    #[test]
    fn eol_only_agrees_with_normalise_eol_prop(
        left in prop::collection::vec(prop::sample::select(vec!["a", "b", "\n", "\r", "\r\n"]), 0..24),
        right in prop::collection::vec(prop::sample::select(vec!["a", "b", "\n", "\r", "\r\n"]), 0..24),
    ) {
        let (l, r) = (left.concat(), right.concat());
        let d = diff(&l, &r);
        let same_lines = engine::normalise_eol(&bytes::Bytes::from(l.clone())) == engine::normalise_eol(&bytes::Bytes::from(r.clone()));
        prop_assert_eq!(d.identical, l == r);
        prop_assert_eq!(d.eol_only, l != r && same_lines);
        prop_assert!(!d.too_large);
        if same_lines {
            prop_assert!(d.hunks.is_empty());
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Semantic diff.
// ---------------------------------------------------------------------------------------------------------------------

fn flat(text: &str) -> Vec<engine::FlatSetting> {
    support::yaml(text).settings
}

fn path(s: &str) -> SettingPath {
    SettingPath::parse(s).unwrap()
}

#[test]
fn semantic_added_removed_changed_type_changed() {
    let left = flat("keep: 1\nchanged: old\nremoved: gone\nretyped: 1\nflagged: true\nlist:\n  - a\n  - b\n");
    let right = flat("keep: 1\nchanged: new\nadded: here\nretyped: \"1\"\nflagged: yes\nlist:\n  - a\n");
    let changes = semantic_diff(&left, &right);
    let summary: Vec<(String, ChangeKind)> = changes.iter().map(|c| (c.path.to_string(), c.kind)).collect();
    assert_eq!(
        summary,
        [
            ("added".to_owned(), ChangeKind::Added),
            ("changed".to_owned(), ChangeKind::Changed),
            ("flagged".to_owned(), ChangeKind::TypeChanged),
            ("list[1]".to_owned(), ChangeKind::Removed),
            ("removed".to_owned(), ChangeKind::Removed),
            ("retyped".to_owned(), ChangeKind::TypeChanged),
        ],
        "sorted by path"
    );
    let by = |p: &str| changes.iter().find(|c| c.path.as_str() == p).unwrap();

    let added = by("added");
    assert!(added.left.is_none() && added.left_location.is_none());
    assert_eq!(added.right.as_ref().unwrap().text, "here");
    assert_eq!(added.right_location.as_ref().unwrap().path.as_str(), "added");

    let removed = by("removed");
    assert!(removed.right.is_none() && removed.right_location.is_none());
    assert_eq!(removed.left.as_ref().unwrap().text, "gone");

    let edited = by("changed");
    assert_eq!(edited.left.as_ref().unwrap().text, "old");
    assert_eq!(edited.right.as_ref().unwrap().text, "new");
    // Locations are the lines where each side writes the value: line 2 in both.
    assert_eq!(
        edited.left_location.as_ref().unwrap().lines,
        LineRange::new(2, 2).unwrap()
    );
    assert_eq!(
        edited.right_location.as_ref().unwrap().lines,
        LineRange::new(2, 2).unwrap()
    );

    // `yes` is not `true` (YAML 1.1 against 1.2): flagged, and a different type.
    let flagged = by("flagged");
    assert_eq!(flagged.left.as_ref().unwrap().ty, ValueType::Bool);
    let yes = flagged.right.as_ref().unwrap();
    assert_eq!(
        (yes.text.as_str(), yes.ty, yes.flag),
        ("yes", ValueType::String, Some(ValueFlag::YamlBooleanLike))
    );

    // The text is the same and the type is not: `1` and `"1"`.
    let retyped = by("retyped");
    assert_eq!(
        retyped.left.as_ref().unwrap().text,
        retyped.right.as_ref().unwrap().text
    );
}

#[test]
fn identical_settings_have_no_changes_whatever_the_file_order() {
    let a = flat("x: 1\ny: 2\nz:\n  q: 3\n");
    let b = flat("z:\n  q: 3\ny: 2\nx: 1\n");
    assert!(semantic_diff(&a, &b).is_empty());
    assert!(semantic_diff::<engine::FlatSetting, engine::FlatSetting>(&[], &[]).is_empty());
}

#[test]
fn semantic_diff_compares_effective_settings_with_parsed_ones() {
    let text = "a: 1\nb: 2\n";
    let settings = flat(text);
    let file = NfsPath::parse_prefix("svc/app.yml").unwrap();
    let layer = Layer {
        path: &file,
        hash: ContentHash::from_bytes([7; 32]),
        settings: &settings,
    };
    let eff = effective(
        MergeMode::WholeFile,
        &Layers {
            tenant: None,
            base: Some(layer),
            root: None,
        },
    )
    .unwrap();
    let git = flat("a: 1\nb: 3\nc: 4\n");
    let changes = semantic_diff(&eff.settings, &git);
    let summary: Vec<_> = changes.iter().map(|c| (c.path.to_string(), c.kind)).collect();
    assert_eq!(
        summary,
        [
            ("b".to_owned(), ChangeKind::Changed),
            ("c".to_owned(), ChangeKind::Added)
        ]
    );
    // The `SettingView` of the three kinds of rows agrees with the setting itself.
    let first = &settings[0];
    assert_eq!(first.path(), &path("a"));
    assert_eq!(first.value().text, "1");
    assert_eq!(first.lines(), LineRange::new(1, 1).unwrap());
}

#[test]
fn multi_document_paths_do_not_collide() {
    let left = flat("a: 1\n---\na: 2\n");
    let right = flat("a: 1\n---\na: 3\n");
    let changes = semantic_diff(&left, &right);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].path.as_str().starts_with("#1/"), "{}", changes[0].path);
}

// ---------------------------------------------------------------------------------------------------------------------
// Binary compare.
// ---------------------------------------------------------------------------------------------------------------------

fn facts(n: u8, size: u64) -> BinaryFacts {
    BinaryFacts {
        hash: ContentHash::from_bytes([n; 32]),
        size,
    }
}

#[test]
fn binary_by_hash_and_size() {
    let same = binary_compare(Some(facts(1, 10)), Some(facts(1, 10)));
    assert!(same.identical);
    assert_eq!(same.compare.left, same.compare.right);
    assert_eq!(
        (same.compare.left_size, same.compare.right_size),
        (Some(10), Some(10))
    );

    let other = binary_compare(Some(facts(1, 10)), Some(facts(2, 12)));
    assert!(!other.identical);
    assert_ne!(other.compare.left, other.compare.right);
    assert_eq!(
        (other.compare.left_size, other.compare.right_size),
        (Some(10), Some(12))
    );

    // The hash is what decides, but a size that disagrees with an equal hash is not called identical.
    assert!(!binary_compare(Some(facts(1, 10)), Some(facts(1, 11))).identical);
    // Same size, different content.
    assert!(!binary_compare(Some(facts(1, 10)), Some(facts(2, 10))).identical);
}

#[test]
fn binary_with_a_missing_side() {
    let left_only = binary_compare(Some(facts(1, 10)), None);
    assert!(!left_only.identical);
    assert!(left_only.compare.right.is_none() && left_only.compare.right_size.is_none());
    assert!(left_only.compare.left.is_some());
    let right_only = binary_compare(None, Some(facts(1, 10)));
    assert!(!right_only.identical && right_only.compare.left.is_none());
    // Nothing on either side: nothing differs.
    let none = binary_compare(None, None);
    assert!(none.identical);
    assert_eq!(none.compare.left, None);
}
