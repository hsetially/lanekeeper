//! The semantic diff (T5): which settings differ between two versions of a structured file.
//!
//! Comparing settings, not lines, is what makes a reordered file or a reformatted one show no change, and what makes `yes`
//! and `true` show as different: the value text and its type tag must both match (`docs/domain-model.md`).

use std::cmp::Ordering;

use domain::{ChangeKind, LineRange, SettingChange, SettingLocation, SettingPath, SettingValue};

use crate::effective::EffectiveValue;
use crate::index::SettingRow;
use crate::parse::FlatSetting;

/// A setting as the diff reads it: where it is, what it is and which lines it is written on. Implemented by the three kinds
/// of setting the engine produces, so that a parsed file, a row of the settings index and an effective config compare with
/// one another.
pub trait SettingView {
    fn path(&self) -> &SettingPath;
    fn value(&self) -> &SettingValue;
    fn lines(&self) -> LineRange;
}

impl SettingView for FlatSetting {
    fn path(&self) -> &SettingPath {
        &self.path
    }
    fn value(&self) -> &SettingValue {
        &self.value
    }
    fn lines(&self) -> LineRange {
        self.lines
    }
}

impl SettingView for SettingRow {
    fn path(&self) -> &SettingPath {
        &self.path
    }
    fn value(&self) -> &SettingValue {
        &self.value
    }
    fn lines(&self) -> LineRange {
        self.lines
    }
}

impl SettingView for EffectiveValue {
    fn path(&self) -> &SettingPath {
        &self.path
    }
    fn value(&self) -> &SettingValue {
        &self.value
    }
    fn lines(&self) -> LineRange {
        self.origin.lines
    }
}

fn location<S: SettingView>(setting: &S) -> SettingLocation {
    SettingLocation {
        path: setting.path().clone(),
        lines: setting.lines(),
    }
}

/// The settings that differ between `left` and `right`, sorted by path.
///
/// A setting only on the right is `Added`, one only on the left `Removed`. A setting on both sides is `Changed` when its
/// text differs, and `TypeChanged` when its type does (so `1` and `"1"` differ, and so do `true` and `yes`). Each change
/// carries both values and both locations, `None` on the side where the setting is absent. The input may be in any order,
/// and a path that is listed twice on one side counts once, the last time (flattened files never have that).
pub fn semantic_diff<L: SettingView, R: SettingView>(left: &[L], right: &[R]) -> Vec<SettingChange> {
    let mut old: Vec<&L> = left.iter().collect();
    let mut new: Vec<&R> = right.iter().collect();
    // Stable, so that "the last one wins" holds for a repeated path.
    old.sort_by(|a, b| a.path().cmp(b.path()));
    new.sort_by(|a, b| a.path().cmp(b.path()));
    dedup_last(&mut old);
    dedup_last(&mut new);

    let mut out = Vec::new();
    let (mut old_at, mut new_at) = (0, 0);
    loop {
        let (previous, next) = (old.get(old_at), new.get(new_at));
        let order = match (previous, next) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(before), Some(after)) => before.path().cmp(after.path()),
        };
        match (order, previous, next) {
            (Ordering::Less, Some(removed), _) => {
                out.push(SettingChange {
                    path: removed.path().clone(),
                    kind: ChangeKind::Removed,
                    left: Some(removed.value().clone()),
                    right: None,
                    left_location: Some(location(*removed)),
                    right_location: None,
                });
                old_at += 1;
            }
            (Ordering::Greater, _, Some(added)) => {
                out.push(SettingChange {
                    path: added.path().clone(),
                    kind: ChangeKind::Added,
                    left: None,
                    right: Some(added.value().clone()),
                    left_location: None,
                    right_location: Some(location(*added)),
                });
                new_at += 1;
            }
            (Ordering::Equal, Some(before), Some(after)) => {
                if before.value() != after.value() {
                    let kind = if before.value().ty == after.value().ty {
                        ChangeKind::Changed
                    } else {
                        ChangeKind::TypeChanged
                    };
                    out.push(SettingChange {
                        path: before.path().clone(),
                        kind,
                        left: Some(before.value().clone()),
                        right: Some(after.value().clone()),
                        left_location: Some(location(*before)),
                        right_location: Some(location(*after)),
                    });
                }
                old_at += 1;
                new_at += 1;
            }
            // `order` was made from `previous` and `next`, so no other combination exists.
            _ => break,
        }
    }
    out
}

/// Keeps the last of each run of equal paths in a list sorted by path.
fn dedup_last<S: SettingView>(sorted: &mut Vec<&S>) {
    let mut kept: Vec<&S> = Vec::with_capacity(sorted.len());
    for s in sorted.drain(..) {
        match kept.last_mut() {
            Some(last) if last.path() == s.path() => *last = s,
            _ => kept.push(s),
        }
    }
    *sorted = kept;
}
