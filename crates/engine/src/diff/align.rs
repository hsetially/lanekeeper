//! Aligning two texts, line by line (T5, D65): which lines are the same, and in what order.
//!
//! The lines are numbered (every distinct line text has one number), and the alignment is Patience with a cost that is known
//! before any of the expensive work starts:
//!
//! 1. The common beginning and end of a stretch are matched and set aside.
//! 2. Lines the other side's stretch lacks cannot match anything and are left out.
//! 3. The lines that occur exactly once on each side are anchors. The longest run of anchors that is in the same order on
//!    both sides is kept as matches, and the stretches between two anchors are aligned the same way, to a depth of
//!    `MAX_DEPTH`. (An anchor is a line both sides have once, so a repeated line, such as a closing brace, never pairs with
//!    the wrong one.)
//! 4. A stretch with no anchor, or at the depth limit, is a leaf, and a leaf is aligned by Myers.
//!
//! Steps 1 to 3 cost a few passes over the lines. Myers is the expensive part, about the product of the lengths of a leaf. So
//! the whole tree of stretches is built first, the leaves' products are added up, and if the sum is over the budget the answer
//! is "too large" and Myers never runs. That is what makes the cost of a diff a function of the input and not of the clock,
//! and why two long files with the same skeleton (the same keys, other values) are cheap and two with only a shared
//! vocabulary are refused.
//!
//! The result is the runs of equal lines, built from the lines reported equal and from nothing else: every run is checked to
//! be in order and to pair equal lines, so the hunks made from it can only describe a true alignment.

use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};

/// A run of lines that are the same in both texts: `len` lines from `old` and from `new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    pub old: usize,
    pub new: usize,
    pub len: usize,
}

/// How many times the stretches between anchors are anchored again before they are left to Myers.
const MAX_DEPTH: u32 = 12;

/// Adds a run, joining it to the last one when they touch in both texts.
fn push_run(runs: &mut Vec<Run>, run: Run) {
    if run.len == 0 {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.old + last.len == run.old && last.new + last.len == run.new => last.len += run.len,
        _ => runs.push(run),
    }
}

/// A part of both texts that still has to be aligned.
struct Stretch {
    old: Range<usize>,
    new: Range<usize>,
    depth: u32,
}

/// Occurrence counts per line number, for one stretch at a time.
struct Counts {
    old: Vec<u8>,
    new: Vec<u8>,
    /// Where in `new` a line was last seen.
    new_at: Vec<usize>,
}

impl Counts {
    fn new(distinct: usize) -> Self {
        Self {
            old: vec![0; distinct],
            new: vec![0; distinct],
            new_at: vec![0; distinct],
        }
    }

    fn add(&mut self, old: &[u32], new: &[u32], old_range: &Range<usize>, new_range: &Range<usize>) {
        for &id in old.get(old_range.clone()).unwrap_or_default() {
            if let Some(c) = self.old.get_mut(id as usize) {
                *c = c.saturating_add(1);
            }
        }
        for at in new_range.clone() {
            let Some(&id) = new.get(at) else { continue };
            if let (Some(c), Some(p)) = (self.new.get_mut(id as usize), self.new_at.get_mut(id as usize)) {
                *c = c.saturating_add(1);
                *p = at;
            }
        }
    }

    fn clear(&mut self, old: &[u32], new: &[u32], old_range: &Range<usize>, new_range: &Range<usize>) {
        for &id in old.get(old_range.clone()).unwrap_or_default() {
            if let Some(c) = self.old.get_mut(id as usize) {
                *c = 0;
            }
        }
        for &id in new.get(new_range.clone()).unwrap_or_default() {
            if let Some(c) = self.new.get_mut(id as usize) {
                *c = 0;
            }
        }
    }

    fn in_both(&self, id: u32) -> bool {
        self.old.get(id as usize).is_some_and(|&c| c > 0) && self.new.get(id as usize).is_some_and(|&c| c > 0)
    }

    fn once_in_both(&self, id: u32) -> bool {
        self.old.get(id as usize) == Some(&1) && self.new.get(id as usize) == Some(&1)
    }
}

/// The longest run of `(old position, new position)` pairs, given in the order of the old positions, whose new positions
/// increase too (patience sorting).
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; pairs.len()];
    for (index, &(_, position)) in pairs.iter().enumerate() {
        let slot = tails.partition_point(|&t| pairs.get(t).is_some_and(|p| p.1 < position));
        if let Some(p) = previous.get_mut(index) {
            *p = slot.checked_sub(1).and_then(|s| tails.get(s).copied());
        }
        match tails.get_mut(slot) {
            Some(t) => *t = index,
            None => tails.push(index),
        }
    }
    let mut chain = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(index) = at {
        if let Some(&pair) = pairs.get(index) {
            chain.push(pair);
        }
        at = previous.get(index).copied().flatten();
    }
    chain.reverse();
    chain
}

/// The cost of a leaf: the product of its two lengths, each counted with one extra so that a long leaf against a short one
/// is not free.
fn leaf_cost(old: usize, new: usize) -> u64 {
    ((old as u64) + 1).saturating_mul((new as u64) + 1)
}

/// The runs of equal lines of two numbered texts, in order. `distinct` is one more than the largest line number. `None`
/// when the leaves that Myers would have to align cost more than `budget` cells in all.
pub(super) fn align(old: &[u32], new: &[u32], distinct: usize, budget: u64) -> Option<Vec<Run>> {
    let mut runs: Vec<Run> = Vec::new();
    // The leaves, as the positions of the lines in each text that they keep.
    let mut leaves: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();
    let mut cost = 0_u64;
    let mut counts = Counts::new(distinct);
    let mut stack = vec![Stretch {
        old: 0..old.len(),
        new: 0..new.len(),
        depth: 0,
    }];

    while let Some(Stretch {
        old: mut o,
        new: mut n,
        depth,
    }) = stack.pop()
    {
        let (left, right) = (
            old.get(o.clone()).unwrap_or_default(),
            new.get(n.clone()).unwrap_or_default(),
        );
        let prefix = left.iter().zip(right).take_while(|(a, b)| a == b).count();
        let suffix = left
            .iter()
            .skip(prefix)
            .rev()
            .zip(right.iter().skip(prefix).rev())
            .take_while(|(a, b)| a == b)
            .count();
        runs.push(Run {
            old: o.start,
            new: n.start,
            len: prefix,
        });
        runs.push(Run {
            old: o.end - suffix,
            new: n.end - suffix,
            len: suffix,
        });
        (o.start, n.start) = (o.start + prefix, n.start + prefix);
        (o.end, n.end) = (o.end - suffix, n.end - suffix);
        if o.is_empty() || n.is_empty() {
            continue;
        }

        counts.add(old, new, &o, &n);
        let keep_old: Vec<usize> = o
            .clone()
            .filter(|&i| old.get(i).is_some_and(|&id| counts.in_both(id)))
            .collect();
        let keep_new: Vec<usize> = n
            .clone()
            .filter(|&i| new.get(i).is_some_and(|&id| counts.in_both(id)))
            .collect();
        let anchors: Vec<(usize, usize)> = keep_old
            .iter()
            .filter_map(|&i| {
                let id = *old.get(i)?;
                counts
                    .once_in_both(id)
                    .then(|| (i, counts.new_at.get(id as usize).copied().unwrap_or(0)))
            })
            .collect();
        counts.clear(old, new, &o, &n);

        let chain = longest_increasing(&anchors);
        if chain.is_empty() || depth >= MAX_DEPTH {
            if !keep_old.is_empty() && !keep_new.is_empty() {
                cost = cost.saturating_add(leaf_cost(keep_old.len(), keep_new.len()));
                if cost > budget {
                    return None;
                }
                leaves.push((keep_old, keep_new));
            }
            continue;
        }
        let (mut old_from, mut new_from) = (o.start, n.start);
        for (i, j) in chain {
            runs.push(Run {
                old: i,
                new: j,
                len: 1,
            });
            stack.push(Stretch {
                old: old_from..i,
                new: new_from..j,
                depth: depth + 1,
            });
            (old_from, new_from) = (i + 1, j + 1);
        }
        stack.push(Stretch {
            old: old_from..o.end,
            new: new_from..n.end,
            depth: depth + 1,
        });
    }

    align_leaves(&leaves, old, new, &mut runs);
    Some(settle(runs, old, new))
}

/// Myers over each leaf: the runs of equal lines it finds are added to `runs`.
fn align_leaves(leaves: &[(Vec<usize>, Vec<usize>)], old: &[u32], new: &[u32], runs: &mut Vec<Run>) {
    for (keep_old, keep_new) in leaves {
        let a: Vec<u32> = keep_old.iter().filter_map(|&i| old.get(i).copied()).collect();
        let b: Vec<u32> = keep_new.iter().filter_map(|&i| new.get(i).copied()).collect();
        for op in capture_diff_slices(Algorithm::Myers, &a, &b) {
            let (tag, in_a, in_b) = op.as_tag_tuple();
            if tag != DiffTag::Equal {
                continue;
            }
            for (x, y) in in_a.zip(in_b) {
                if let (Some(&i), Some(&j)) = (keep_old.get(x), keep_new.get(y)) {
                    runs.push(Run {
                        old: i,
                        new: j,
                        len: 1,
                    });
                }
            }
        }
    }
}

/// The runs in order and joined where they touch, keeping only what is true: in order in both texts, and pairing equal lines.
fn settle(mut runs: Vec<Run>, old: &[u32], new: &[u32]) -> Vec<Run> {
    runs.retain(|r| r.len > 0);
    runs.sort_by_key(|r| (r.old, r.new));
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    let (mut next_old, mut next_new) = (0, 0);
    for run in runs {
        let in_range = run.old + run.len <= old.len() && run.new + run.len <= new.len();
        let in_order = run.old >= next_old && run.new >= next_new;
        let equal = in_range && old.get(run.old..run.old + run.len) == new.get(run.new..run.new + run.len);
        if in_order && equal {
            (next_old, next_new) = (run.old + run.len, run.new + run.len);
            push_run(&mut out, run);
        }
    }
    out
}
