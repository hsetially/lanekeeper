//! The text diff (T5, D65, P7): hunks of lines for a virtualised view.
//!
//! The steps, in order, each cheaper than the next:
//!
//! 1. Equal texts are `identical`. (Equal bytes: no work at all.)
//! 2. Line endings are normalised on both sides (`\r\n` and a lone `\r` become `\n`, as `normalise_eol` does). If the texts
//!    are equal now, the only difference is the line endings: no hunks, `eol_only` is set. The UI can say so without
//!    drawing every line as changed.
//! 3. The size is checked against `Limits`. A text with more than `diff_lines` lines on either side gets the `too_large`
//!    outcome and no hunks.
//! 4. The lines are aligned (`align.rs`): Patience, which anchors on the lines that occur once on each side, so that a
//!    repeated line (a closing brace, a blank line) never pairs with the wrong one, and which reads better on configuration
//!    than Myers (D65). Its cost is worked out before it runs; if it is over `diff_cells`, the outcome is `too_large` as
//!    well. Both limits are counts, so the outcome never depends on how busy the machine is.
//! 5. The alignment is a list of runs of matched lines. The lines between runs are the changes, and changes with at most
//!    twice `DIFF_CONTEXT_LINES` lines between them share a hunk.
//!
//! A line is compared with its terminator, so a missing final newline is a difference (the line is shown as removed and
//! added, with the same text). The text of a `DiffLine` has no terminator and no `\r`. The numbering of a hunk is the
//! unified diff's: starts are 1-based, and a side with no lines in the hunk (which only happens when that whole text is
//! empty) starts at 0.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use domain::{DiffHunk, DiffLine, DiffLineKind};

use super::align::{Run, align};
use crate::effective::len32;
use crate::limits::Limits;

/// Lines of unchanged text kept on each side of a change.
pub const DIFF_CONTEXT_LINES: usize = 3;

/// The result of `text_diff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    /// The two texts are the same, byte for byte.
    pub identical: bool,
    /// The texts differ, and the only difference is their line endings.
    pub eol_only: bool,
    /// The texts are over a limit (`Limits::diff_lines` or `Limits::diff_cells`) and were not compared. Not `identical`.
    pub too_large: bool,
    /// Empty when `identical`, `eol_only` or `too_large`.
    pub hunks: Vec<DiffHunk>,
}

impl TextDiff {
    const fn without_hunks(identical: bool, eol_only: bool, too_large: bool) -> Self {
        Self {
            identical,
            eol_only,
            too_large,
            hunks: Vec::new(),
        }
    }
}

/// `\r\n` and a lone `\r` as `\n`; the same buffer (borrowed) when there is no `\r`.
fn normalised(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\r') {
        let (before, after) = rest.split_at(at);
        out.push_str(before);
        out.push('\n');
        // `after` starts with the `\r`, one byte.
        rest = after.get(1..).unwrap_or("");
        if let Some(next) = rest.strip_prefix('\n') {
            rest = next;
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The runs of equal lines of two texts, in order, or `None` when aligning them would cost more than `limits` allow.
fn align_texts(old: &[&str], new: &[&str], limits: &Limits) -> Option<Vec<Run>> {
    // Every distinct line gets a number, in the order of the lines' text so that the same input always gets the same
    // numbers, and the alignment compares numbers, not text.
    let mut ids: BTreeMap<&str, u32> = BTreeMap::new();
    for text in old.iter().chain(new) {
        let next = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        ids.entry(text).or_insert(next);
    }
    let number = |lines: &[&str]| -> Vec<u32> {
        lines
            .iter()
            .map(|l| ids.get(l).copied().unwrap_or(u32::MAX))
            .collect()
    };
    align(&number(old), &number(new), ids.len(), limits.diff_cells as u64)
}

/// Compares two texts line by line. The texts are decoded text (`decode_text`); line endings may differ between them.
///
/// See the module documentation for the order of the steps and for what each field of the result means.
pub fn text_diff(left: &str, right: &str, limits: &Limits) -> TextDiff {
    if left == right {
        return TextDiff::without_hunks(true, false, false);
    }
    let (l, r) = (normalised(left), normalised(right));
    if l == r {
        return TextDiff::without_hunks(false, true, false);
    }

    let old: Vec<&str> = l.split_inclusive('\n').collect();
    let new: Vec<&str> = r.split_inclusive('\n').collect();
    if old.len() > limits.diff_lines || new.len() > limits.diff_lines {
        return TextDiff::without_hunks(false, false, true);
    }
    let Some(runs) = align_texts(&old, &new, limits) else {
        return TextDiff::without_hunks(false, false, true);
    };
    TextDiff {
        identical: false,
        eol_only: false,
        too_large: false,
        hunks: hunks(&runs, &old, &new),
    }
}

fn line(kind: DiffLineKind, text: &str) -> DiffLine {
    DiffLine {
        kind,
        text: text.strip_suffix('\n').unwrap_or(text).to_owned(),
        // Word-level ranges are filled in by `diff::inline` (T9).
        inline_changes: Vec::new(),
    }
}

/// A block of removed and added lines between two runs: `old` lines of the left text are replaced by `new` lines of the right.
struct Change {
    old: Range<usize>,
    new: Range<usize>,
}

/// The changes between the runs, in order: everything the runs do not cover.
fn changes(runs: &[Run], old_len: usize, new_len: usize) -> Vec<Change> {
    let mut out = Vec::new();
    let (mut old_at, mut new_at) = (0, 0);
    for run in runs {
        if old_at < run.old || new_at < run.new {
            out.push(Change {
                old: old_at..run.old,
                new: new_at..run.new,
            });
        }
        (old_at, new_at) = (run.old + run.len, run.new + run.len);
    }
    if old_at < old_len || new_at < new_len {
        out.push(Change {
            old: old_at..old_len,
            new: new_at..new_len,
        });
    }
    out
}

/// The hunks of an alignment: each change with `DIFF_CONTEXT_LINES` lines of unchanged text around it, and changes that are
/// close enough for their contexts to touch in one hunk.
fn hunks(runs: &[Run], old: &[&str], new: &[&str]) -> Vec<DiffHunk> {
    let all = changes(runs, old.len(), new.len());
    let mut out = Vec::new();
    let mut at = 0;
    while at < all.len() {
        // The changes of this hunk: each next one starts within twice the context of where the last one ended.
        let mut end = at + 1;
        while end < all.len() && all[end].old.start - all[end - 1].old.end <= 2 * DIFF_CONTEXT_LINES {
            end += 1;
        }
        out.push(hunk(
            &all[at..end],
            at.checked_sub(1).map(|p| &all[p]),
            all.get(end),
            old,
            new,
        ));
        at = end;
    }
    out
}

/// One hunk from consecutive changes. `before` and `after` are the changes on either side, which limit the context.
fn hunk(
    group: &[Change],
    before: Option<&Change>,
    after: Option<&Change>,
    old: &[&str],
    new: &[&str],
) -> DiffHunk {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return DiffHunk {
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            lines: Vec::new(),
        };
    };
    let lead = (first.old.start - before.map_or(0, |b| b.old.end)).min(DIFF_CONTEXT_LINES);
    let trail = (after.map_or(old.len(), |a| a.old.start) - last.old.end).min(DIFF_CONTEXT_LINES);
    let old_from = first.old.start - lead;
    let new_from = first.new.start - lead;

    let mut lines = Vec::new();
    let context = |lines: &mut Vec<DiffLine>, from: usize, count: usize| {
        lines.extend(
            old.iter()
                .skip(from)
                .take(count)
                .map(|t| line(DiffLineKind::Context, t)),
        );
    };
    context(&mut lines, old_from, lead);
    for (n, change) in group.iter().enumerate() {
        lines.extend(
            old[change.old.clone()]
                .iter()
                .map(|t| line(DiffLineKind::Removed, t)),
        );
        lines.extend(
            new[change.new.clone()]
                .iter()
                .map(|t| line(DiffLineKind::Added, t)),
        );
        if let Some(next) = group.get(n + 1) {
            context(&mut lines, change.old.end, next.old.start - change.old.end);
        }
    }
    context(&mut lines, last.old.end, trail);

    let old_lines = last.old.end + trail - old_from;
    let new_lines = last.new.end + trail - new_from;
    DiffHunk {
        old_start: start(old_from, old_lines),
        old_lines: len32(old_lines),
        new_start: start(new_from, new_lines),
        new_lines: len32(new_lines),
        lines,
    }
}

/// The unified diff's start: 1-based; for a side with no lines, the line before it.
fn start(from: usize, count: usize) -> u32 {
    len32(if count == 0 { from } else { from + 1 })
}
