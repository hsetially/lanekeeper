//! Drift (T5, D19, D20): is a file on NFS the same as the baseline and as Git?
//!
//! For one swimlane and one NFS path three hashes matter: **B**, the baseline (what the file was when somebody last
//! accepted it), **N**, the file on NFS now, and **G**, the file in Git now (found through the path mapping). A missing file
//! is the hash `∅`, which this module spells `None`, and the table of `docs/domain-model.md` applies to it like to any other
//! value. The function takes the three hashes and an optional "intentional divergence" mark, and returns the state.
//!
//! Hashes are compared after line endings are normalised for `structured` and `text` files: the caller makes them with
//! [`drift_hash`] (and stores the baseline the same way), so that a file saved with other line endings is not drift.
//!
//! The function reads no clock and no file, and the result depends on its input only.

use bytes::Bytes;
use domain::{ContentHash, DriftState, FileClass};

use crate::tree_hash::normalised_hash;

/// The hash of a file for drift: [`normalised_hash`] of the content, or `None` for a missing file (`∅`). An empty file is a
/// file, with the hash of no bytes.
pub fn drift_hash(content: Option<&Bytes>, class: FileClass) -> Option<ContentHash> {
    content.map(|data| normalised_hash(data, class))
}

/// What is known about the baseline B of one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// No baseline has been recorded yet (a path seen for the first time).
    NotRecorded,
    /// A baseline was recorded. `None` is a baseline of `∅`: the file was missing when it was accepted.
    Recorded(Option<ContentHash>),
}

/// The hashes recorded when somebody marked a divergence as intended. `None` is `∅`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntentionalMark {
    pub nfs: Option<ContentHash>,
    pub git: Option<ContentHash>,
}

/// Everything `drift_state` needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriftInput {
    pub baseline: Baseline,
    /// N: the file on NFS now, `None` when it is missing.
    pub nfs: Option<ContentHash>,
    /// G: the file in Git now (base file from the base repository's default branch, tenant file from the tenant's branch),
    /// `None` when it is missing.
    pub git: Option<ContentHash>,
    /// The mark of an intentional divergence, when somebody set one.
    pub intentional: Option<IntentionalMark>,
}

/// How the UI labels a missing file (the second table of the drift section of `docs/domain-model.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftLabel {
    /// `nfs_ahead` with N = `∅`.
    DeletedOnNfs,
    /// `git_ahead` with N = `∅`: Git has a file that the baseline and NFS lack.
    NewInGit,
    /// `git_ahead` with G = `∅`.
    DeletedInGit,
}

/// The result of `drift_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drift {
    /// What to show. `IntentionalDivergence` when a mark overrides the table's state.
    pub state: DriftState,
    /// What the table gives without any mark.
    pub underlying: DriftState,
    /// The table says "set B = N" (N and G moved to the same content, or there was no baseline and they agree): the caller
    /// records N as the new baseline.
    pub set_baseline_to_nfs: bool,
    /// The label for a missing file, for the underlying state.
    pub label: Option<DriftLabel>,
    /// A mark was given and no longer holds (N or G changed since it was set, or there is nothing to accept any more): the
    /// caller clears it.
    pub mark_stale: bool,
}

/// The drift state of one path.
///
/// 1. `untracked` first: N is present, G is `∅` and there is no baseline.
/// 2. Otherwise the table: without a baseline `in_sync` (and B = N) when N = G, else `unknown`; with one, `in_sync` when N
///    and G both equal B, `git_ahead` when only N does, `nfs_ahead` when only G does, and when neither does, `in_sync`
///    (B = N) if N = G and `conflict` if not.
/// 3. `intentional_divergence` replaces `git_ahead`, `nfs_ahead`, `conflict` and `unknown` while N and G both still equal the
///    hashes of the mark. When either has changed the table's state stands and the mark is reported stale.
pub fn drift_state(input: &DriftInput) -> Drift {
    let DriftInput {
        baseline,
        nfs,
        git,
        intentional,
    } = *input;

    let (underlying, set_baseline_to_nfs) = match baseline {
        Baseline::NotRecorded if nfs.is_some() && git.is_none() => (DriftState::Untracked, false),
        Baseline::NotRecorded if nfs == git => (DriftState::InSync, true),
        Baseline::NotRecorded => (DriftState::Unknown, false),
        Baseline::Recorded(b) => match (nfs == b, git == b) {
            (true, true) => (DriftState::InSync, false),
            (true, false) => (DriftState::GitAhead, false),
            (false, true) => (DriftState::NfsAhead, false),
            (false, false) if nfs == git => (DriftState::InSync, true),
            (false, false) => (DriftState::Conflict, false),
        },
    };

    let label = match underlying {
        DriftState::NfsAhead if nfs.is_none() => Some(DriftLabel::DeletedOnNfs),
        DriftState::GitAhead if nfs.is_none() => Some(DriftLabel::NewInGit),
        DriftState::GitAhead if git.is_none() => Some(DriftLabel::DeletedInGit),
        _ => None,
    };

    let can_be_accepted = matches!(
        underlying,
        DriftState::GitAhead | DriftState::NfsAhead | DriftState::Conflict | DriftState::Unknown
    );
    let holds = intentional.is_some_and(|mark| mark.nfs == nfs && mark.git == git);
    let state = if can_be_accepted && holds {
        DriftState::IntentionalDivergence
    } else {
        underlying
    };

    Drift {
        state,
        underlying,
        set_baseline_to_nfs,
        label,
        mark_stale: intentional.is_some() && state != DriftState::IntentionalDivergence,
    }
}
