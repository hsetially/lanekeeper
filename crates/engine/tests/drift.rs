//! T5 (D19, D20): `drift_state`, the table of `docs/domain-model.md` one row at a time, and then every combination of
//! hashes against a second, independent statement of the same table.
//!
//! B is the baseline, N the hash on NFS and G the hash in Git. `None` is the missing file (the hash value `∅`).
// The reference implementation states the table with plain `Option`s on purpose, to share nothing with the engine's `Baseline`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::option_option
)]

use bytes::Bytes;
use domain::{ContentHash, DriftState, FileClass};
use engine::{Baseline, Drift, DriftInput, DriftLabel, IntentionalMark, drift_hash, drift_state};

fn h(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

fn input(baseline: Baseline, nfs: Option<ContentHash>, git: Option<ContentHash>) -> DriftInput {
    DriftInput {
        baseline,
        nfs,
        git,
        intentional: None,
    }
}

fn state(baseline: Baseline, nfs: Option<ContentHash>, git: Option<ContentHash>) -> Drift {
    drift_state(&input(baseline, nfs, git))
}

const A: u8 = 1;
const B: u8 = 2;
const C: u8 = 3;

// ---------------------------------------------------------------------------------------------------------------------
// The seven rows of the table.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn row_1_no_baseline_and_nfs_equals_git_is_in_sync_and_adopts_nfs_as_the_baseline() {
    let d = state(Baseline::NotRecorded, Some(h(A)), Some(h(A)));
    assert_eq!(d.state, DriftState::InSync);
    assert!(d.set_baseline_to_nfs, "B = N");
    assert_eq!(d.label, None);
}

#[test]
fn row_2_no_baseline_and_nfs_differs_from_git_is_unknown() {
    let d = state(Baseline::NotRecorded, Some(h(A)), Some(h(B)));
    assert_eq!(d.state, DriftState::Unknown);
    assert!(!d.set_baseline_to_nfs);
}

#[test]
fn row_3_nfs_and_git_both_equal_the_baseline_is_in_sync() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(A)), Some(h(A)));
    assert_eq!(d.state, DriftState::InSync);
    assert!(!d.set_baseline_to_nfs, "B is already N");
}

#[test]
fn row_4_nfs_equals_the_baseline_and_git_moved_is_git_ahead() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(A)), Some(h(B)));
    assert_eq!(d.state, DriftState::GitAhead);
    assert!(!d.set_baseline_to_nfs);
}

#[test]
fn row_5_git_equals_the_baseline_and_nfs_moved_is_nfs_ahead() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(A)));
    assert_eq!(d.state, DriftState::NfsAhead);
    assert!(!d.set_baseline_to_nfs);
}

#[test]
fn row_6_both_moved_to_the_same_content_is_in_sync_and_adopts_nfs_as_the_baseline() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(B)));
    assert_eq!(d.state, DriftState::InSync);
    assert!(d.set_baseline_to_nfs, "B = N");
}

#[test]
fn row_7_both_moved_to_different_content_is_a_conflict() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(C)));
    assert_eq!(d.state, DriftState::Conflict);
    assert!(!d.set_baseline_to_nfs);
}

// ---------------------------------------------------------------------------------------------------------------------
// `untracked` is checked first.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn untracked_is_a_file_on_nfs_that_git_lacks_and_that_has_no_baseline() {
    let d = state(Baseline::NotRecorded, Some(h(A)), None);
    assert_eq!(d.state, DriftState::Untracked);
    assert!(!d.set_baseline_to_nfs);
}

#[test]
fn untracked_needs_the_absence_of_a_baseline() {
    // With a baseline the same hashes are an ordinary row: here N differs from B and G equals B (= ∅).
    let d = state(Baseline::Recorded(None), Some(h(A)), None);
    assert_eq!(d.state, DriftState::NfsAhead);
    // And a baseline that has the file makes a file missing in Git "deleted in Git".
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(A)), None);
    assert_eq!(d.state, DriftState::GitAhead);
}

#[test]
fn untracked_is_checked_before_the_table() {
    // Without the first check this would be row 2 (`unknown`).
    assert_eq!(
        state(Baseline::NotRecorded, Some(h(B)), None).state,
        DriftState::Untracked
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// A missing file is the hash ∅, and the table still applies.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn missing_everywhere_without_a_baseline_is_in_sync() {
    let d = state(Baseline::NotRecorded, None, None);
    assert_eq!(d.state, DriftState::InSync);
}

#[test]
fn deleted_on_nfs_is_nfs_ahead_with_its_label() {
    let d = state(Baseline::Recorded(Some(h(A))), None, Some(h(A)));
    assert_eq!(d.state, DriftState::NfsAhead);
    assert_eq!(d.label, Some(DriftLabel::DeletedOnNfs));
}

#[test]
fn new_in_git_is_git_ahead_with_its_label() {
    // The baseline has no file, NFS has none, and Git has one.
    let d = state(Baseline::Recorded(None), None, Some(h(A)));
    assert_eq!(d.state, DriftState::GitAhead);
    assert_eq!(d.label, Some(DriftLabel::NewInGit));
}

#[test]
fn deleted_in_git_is_git_ahead_with_its_label() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(A)), None);
    assert_eq!(d.state, DriftState::GitAhead);
    assert_eq!(d.label, Some(DriftLabel::DeletedInGit));
}

#[test]
fn no_label_for_the_other_states() {
    for d in [
        state(Baseline::Recorded(Some(h(A))), Some(h(A)), Some(h(B))),
        state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(A))),
        state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(C))),
        state(Baseline::NotRecorded, Some(h(A)), Some(h(B))),
        state(Baseline::NotRecorded, Some(h(A)), None),
    ] {
        assert_eq!(d.label, None, "{:?}", d.state);
    }
}

#[test]
fn deleted_on_both_sides_since_the_baseline_is_in_sync() {
    // Row 6 with ∅ on both: N and G both moved, to the same content (none).
    let d = state(Baseline::Recorded(Some(h(A))), None, None);
    assert_eq!(d.state, DriftState::InSync);
    assert!(d.set_baseline_to_nfs);
}

// ---------------------------------------------------------------------------------------------------------------------
// `intentional_divergence`.
// ---------------------------------------------------------------------------------------------------------------------

fn marked(
    baseline: Baseline,
    nfs: Option<ContentHash>,
    git: Option<ContentHash>,
    mark_nfs: Option<ContentHash>,
    mark_git: Option<ContentHash>,
) -> Drift {
    drift_state(&DriftInput {
        baseline,
        nfs,
        git,
        intentional: Some(IntentionalMark {
            nfs: mark_nfs,
            git: mark_git,
        }),
    })
}

#[test]
fn intentional_divergence_overrides_the_four_divergent_states_while_both_hashes_hold() {
    let recorded = Baseline::Recorded(Some(h(A)));
    let cases = [
        // git_ahead
        (recorded, Some(h(A)), Some(h(B)), DriftState::GitAhead),
        // nfs_ahead
        (recorded, Some(h(B)), Some(h(A)), DriftState::NfsAhead),
        // conflict
        (recorded, Some(h(B)), Some(h(C)), DriftState::Conflict),
        // unknown
        (Baseline::NotRecorded, Some(h(A)), Some(h(B)), DriftState::Unknown),
    ];
    for (baseline, n, g, underlying) in cases {
        let d = marked(baseline, n, g, n, g);
        assert_eq!(d.state, DriftState::IntentionalDivergence, "{underlying:?}");
        assert_eq!(
            d.underlying, underlying,
            "the state the table gives stays visible"
        );
        assert!(!d.mark_stale);
    }
}

#[test]
fn intentional_divergence_clears_as_soon_as_nfs_changes() {
    let recorded = Baseline::Recorded(Some(h(A)));
    // Marked at (N = B, G = C). Now N moved on.
    let d = marked(recorded, Some(h(A)), Some(h(C)), Some(h(B)), Some(h(C)));
    assert_eq!(d.state, DriftState::GitAhead, "the table's state returns");
    assert!(d.mark_stale, "the caller clears the mark");
}

#[test]
fn intentional_divergence_clears_as_soon_as_git_changes() {
    let recorded = Baseline::Recorded(Some(h(A)));
    let d = marked(recorded, Some(h(B)), Some(h(A)), Some(h(B)), Some(h(C)));
    assert_eq!(d.state, DriftState::NfsAhead);
    assert!(d.mark_stale);
}

#[test]
fn intentional_divergence_never_overrides_in_sync_or_untracked() {
    // The mark is for a state that no longer exists: the two now agree.
    let d = marked(
        Baseline::Recorded(Some(h(A))),
        Some(h(A)),
        Some(h(A)),
        Some(h(A)),
        Some(h(A)),
    );
    assert_eq!(d.state, DriftState::InSync);
    assert!(d.mark_stale);
    // Untracked is not a divergence somebody can accept; it is adopted or deleted.
    let d = marked(Baseline::NotRecorded, Some(h(A)), None, Some(h(A)), None);
    assert_eq!(d.state, DriftState::Untracked);
    assert!(d.mark_stale);
}

#[test]
fn a_mark_with_a_missing_file_holds_while_the_file_stays_missing() {
    // Deleted on NFS, intentionally: the mark records N = ∅.
    let d = marked(Baseline::Recorded(Some(h(A))), None, Some(h(A)), None, Some(h(A)));
    assert_eq!(d.state, DriftState::IntentionalDivergence);
    assert_eq!(d.underlying, DriftState::NfsAhead);
    assert_eq!(
        d.label,
        Some(DriftLabel::DeletedOnNfs),
        "the label of the underlying state"
    );
    // The file comes back: the mark no longer matches.
    let d = marked(
        Baseline::Recorded(Some(h(A))),
        Some(h(B)),
        Some(h(A)),
        None,
        Some(h(A)),
    );
    assert_eq!(d.state, DriftState::NfsAhead);
    assert!(d.mark_stale);
}

#[test]
fn no_mark_means_not_stale_and_underlying_is_the_state() {
    let d = state(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(C)));
    assert_eq!(d.underlying, d.state);
    assert!(!d.mark_stale);
}

// ---------------------------------------------------------------------------------------------------------------------
// Every combination, against the table written out a second time.
// ---------------------------------------------------------------------------------------------------------------------

/// The table of `docs/domain-model.md`, row by row, with none of the structure of the implementation.
fn reference(baseline: Option<Option<u8>>, n: Option<u8>, g: Option<u8>) -> (DriftState, bool) {
    // "Check for `untracked` first. If N is present, G is ∅ and there is no baseline."
    if baseline.is_none() && n.is_some() && g.is_none() {
        return (DriftState::Untracked, false);
    }
    match baseline {
        None => {
            if n == g {
                (DriftState::InSync, true)
            } else {
                (DriftState::Unknown, false)
            }
        }
        Some(b) => {
            if n == b && g == b {
                (DriftState::InSync, false)
            } else if n == b {
                (DriftState::GitAhead, false)
            } else if g == b {
                (DriftState::NfsAhead, false)
            } else if n == g {
                (DriftState::InSync, true)
            } else {
                (DriftState::Conflict, false)
            }
        }
    }
}

#[test]
fn every_combination_of_hashes_matches_the_table() {
    let values: [Option<u8>; 4] = [None, Some(A), Some(B), Some(C)];
    let mut baselines: Vec<Option<Option<u8>>> = vec![None];
    baselines.extend(values.iter().map(|v| Some(*v)));
    let mut checked = 0;
    for baseline in &baselines {
        for n in values {
            for g in values {
                let b = match baseline {
                    None => Baseline::NotRecorded,
                    Some(v) => Baseline::Recorded(v.map(h)),
                };
                let got = state(b, n.map(h), g.map(h));
                let (want_state, want_set) = reference(*baseline, n, g);
                assert_eq!(
                    (got.state, got.set_baseline_to_nfs),
                    (want_state, want_set),
                    "B={baseline:?} N={n:?} G={g:?}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 5 * 4 * 4);
}

#[test]
fn drift_state_is_a_function_of_its_input() {
    let i = input(Baseline::Recorded(Some(h(A))), Some(h(B)), Some(h(C)));
    assert_eq!(drift_state(&i), drift_state(&i));
}

// ---------------------------------------------------------------------------------------------------------------------
// Line endings do not count for text; they do for bytes.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn line_endings_are_normalised_for_structured_and_text_files() {
    let lf = Bytes::from_static(b"a: 1\nb: 2\n");
    let crlf = Bytes::from_static(b"a: 1\r\nb: 2\r\n");
    for class in [FileClass::Structured, FileClass::Text] {
        assert_eq!(
            drift_hash(Some(&lf), class),
            drift_hash(Some(&crlf), class),
            "{class:?}"
        );
    }
    // A file that differs only in line endings is in sync.
    let n = drift_hash(Some(&crlf), FileClass::Structured);
    let g = drift_hash(Some(&lf), FileClass::Structured);
    assert_eq!(state(Baseline::NotRecorded, n, g).state, DriftState::InSync);
}

#[test]
fn line_endings_count_for_binary_and_denied_files() {
    let lf = Bytes::from_static(b"\x89PNG\n\x00");
    let crlf = Bytes::from_static(b"\x89PNG\r\n\x00");
    for class in [FileClass::Binary, FileClass::Denied] {
        assert_ne!(
            drift_hash(Some(&lf), class),
            drift_hash(Some(&crlf), class),
            "{class:?}"
        );
    }
}

#[test]
fn a_missing_file_has_no_hash() {
    assert_eq!(drift_hash(None, FileClass::Structured), None);
    assert_eq!(drift_hash(None, FileClass::Binary), None);
}

#[test]
fn an_empty_file_is_not_a_missing_file() {
    let empty = Bytes::new();
    assert!(drift_hash(Some(&empty), FileClass::Text).is_some());
    assert_ne!(
        state(
            Baseline::Recorded(Some(h(A))),
            drift_hash(Some(&empty), FileClass::Text),
            Some(h(A))
        )
        .label,
        Some(DriftLabel::DeletedOnNfs)
    );
}
