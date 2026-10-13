//! Inline (word-level) change ranges for the lines of a text diff (T9, D76): which characters of a replaced line changed.
//!
//! A removed line and the added line that replaces it (the first removed line of a change with the first added line, the
//! second with the second, and so on; lines without a partner are whole-line changes and get no ranges) are split into
//! tokens, and the tokens are diffed.
//!
//! # Tokens
//!
//! Tuned for YAML, where a line is a key, a colon and a value:
//!
//! - a run of letters, digits and `_` is one token (a word, a number, one segment of a key);
//! - a run of white space is one token;
//! - every other character, a colon, a dash, a dot, a quote, a bracket, a `$`, is a token of its own.
//!
//! So `timeout-ms: 30` is `timeout`, `-`, `ms`, `:`, ` `, `30`: renaming the key to `timeout-seconds` and the value to `45`
//! marks `ms` and `30` and nothing between them, and a version `1.2.3` changing to `1.2.4` marks the `3` only. Tokens are
//! whole characters, so every range starts and ends on a character boundary.
//!
//! # Ranges
//!
//! The tokens that differ are mapped to byte ranges in each line (a removed line gets the ranges of the tokens deleted, an
//! added line those inserted). Neighbouring ranges are joined when nothing but white space lies between them, so `red green`
//! turning into `cyan pink` is one range per line, not two.
//!
//! # Bounds
//!
//! The work depends on the input only (S21, determinism): a line longer than `Limits::inline_line_bytes` is not compared, and
//! the pairs of one diff share a budget of `Limits::diff_cells` cells, each pair costing the square of the number of tokens
//! left after the common beginning and end are set aside. A pair that does not fit gets no ranges, and the diff is as it
//! would be without them. The pairs are taken in order, so which pairs run out of budget is the same every time.

use domain::InlineRange;
use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::effective::len32;
use crate::limits::Limits;

/// The lines of a change that have a partner, side by side: `old[i]` is compared with `new[i]` for `i < min(len)`.
///
/// `old` and `new` are the texts of the removed and added lines, without terminators. The result holds, for each line, its
/// ranges (empty when it has none). `budget` is the cells left for the diff; it is reduced by the work done.
pub(super) fn pair_ranges(
    old: &[&str],
    new: &[&str],
    limits: &Limits,
    budget: &mut u64,
) -> (Vec<Vec<InlineRange>>, Vec<Vec<InlineRange>>) {
    let mut old_ranges = vec![Vec::new(); old.len()];
    let mut new_ranges = vec![Vec::new(); new.len()];
    for ((o, n), (or, nr)) in old
        .iter()
        .zip(new)
        .zip(old_ranges.iter_mut().zip(new_ranges.iter_mut()))
    {
        if let Some((a, b)) = line_ranges(o, n, limits, budget) {
            *or = a;
            *nr = b;
        }
    }
    (old_ranges, new_ranges)
}

/// The ranges of one pair: `None` when the pair is over a bound.
fn line_ranges(
    old: &str,
    new: &str,
    limits: &Limits,
    budget: &mut u64,
) -> Option<(Vec<InlineRange>, Vec<InlineRange>)> {
    if old.len() > limits.inline_line_bytes || new.len() > limits.inline_line_bytes {
        return None;
    }
    if old == new {
        return Some((Vec::new(), Vec::new()));
    }
    let (a, b) = (tokens(old), tokens(new));

    // The common beginning and end are equal tokens; only what is between them is diffed, and only that is paid for.
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let size = (mid_a.len() + mid_b.len()) as u64;
    let cost = size.saturating_mul(size);
    if cost > *budget {
        return None;
    }
    *budget -= cost;

    let mut old_changed = Vec::new();
    let mut new_changed = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, mid_a, mid_b) {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        old_changed.push((head + op.old_range().start, head + op.old_range().end));
        new_changed.push((head + op.new_range().start, head + op.new_range().end));
    }
    Some((
        byte_ranges(old, &a, &old_changed),
        byte_ranges(new, &b, &new_changed),
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// The tokens of a line, in order. They cover the line: their lengths add up to its length.
fn tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut current: Option<Class> = None;
    for (at, c) in line.char_indices() {
        let kind = class(c);
        // Words and white space continue a run of their own kind; any other character is a token by itself.
        let continues = current == Some(kind) && kind != Class::Other;
        if !continues && at > start {
            out.push(&line[start..at]);
            start = at;
        }
        current = Some(kind);
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// The byte ranges of the token ranges `changed` (in order, as token indices), with ranges that have only white space between
/// them joined.
fn byte_ranges(line: &str, tokens: &[&str], changed: &[(usize, usize)]) -> Vec<InlineRange> {
    // Where each token starts: the prefix sums of the token lengths.
    let mut starts = Vec::with_capacity(tokens.len() + 1);
    let mut at = 0_usize;
    for token in tokens {
        starts.push(at);
        at += token.len();
    }
    starts.push(at);

    let mut out: Vec<(usize, usize)> = Vec::new();
    for &(from, to) in changed {
        let (Some(&s), Some(&e)) = (starts.get(from), starts.get(to)) else {
            continue;
        };
        if s >= e {
            continue;
        }
        match out.last_mut() {
            Some(last)
                if line
                    .get(last.1..s)
                    .is_some_and(|gap| gap.chars().all(char::is_whitespace)) =>
            {
                last.1 = e;
            }
            _ => out.push((s, e)),
        }
    }
    out.into_iter()
        .map(|(start, end)| InlineRange {
            start: len32(start),
            end: len32(end),
        })
        .collect()
}
