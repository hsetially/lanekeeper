//! C5 `drift` (domain-model, "Consistency checks"): the file on NFS is not what Git and the baseline say, and nobody has
//! said that this is fine. Every drift state is a finding except `in_sync` and `intentional_divergence`.

use domain::{DriftState, FindingKind};

use super::{CheckDetail, CheckFile, CheckResult, message};
use crate::drift::DriftLabel;

/// C5 for one file, from the result of `drift_state`. `None` when the file has no drift result or is not drifting.
pub fn c5_drift(file: &CheckFile<'_>) -> Option<CheckResult> {
    let drift = file.drift?;
    let text = match (drift.state, drift.label) {
        (DriftState::InSync | DriftState::IntentionalDivergence, _) => return None,
        (DriftState::GitAhead, Some(DriftLabel::NewInGit)) => {
            "Git has a file that is missing on NFS (new in Git)."
        }
        (DriftState::GitAhead, Some(DriftLabel::DeletedInGit)) => {
            "The file was deleted in Git and is still on NFS."
        }
        (DriftState::GitAhead, _) => "Git has changes that NFS does not have.",
        (DriftState::NfsAhead, Some(DriftLabel::DeletedOnNfs)) => {
            "The file was deleted on NFS after the baseline."
        }
        (DriftState::NfsAhead, _) => "NFS has changes that Git does not have.",
        (DriftState::Conflict, _) => "NFS and Git both changed after the baseline, in different ways.",
        (DriftState::Unknown, _) => "There is no baseline for this file, and NFS differs from Git.",
        (DriftState::Untracked, _) => "The file is on NFS and has no source in Git.",
    };
    Some(CheckResult {
        kind: FindingKind::Drift,
        path: Some(file.path.clone()),
        message: message(text)?,
        locations: Vec::new(),
        detail: CheckDetail::Drift {
            state: drift.state,
            label: drift.label,
        },
    })
}
