//! C7 `overwritten_nfs_change` (domain-model, "Consistency checks"): a sync replaced content that was only on NFS.
//!
//! The change was in `nfs_ahead` or `conflict`, so nobody had accepted it, and a sync job wrote over it. The finding
//! carries the hash of the version that was lost, so that the caller can link to it and offer to restore it.

use domain::{AttributionSource, DriftState, FindingKind};

use super::{CheckDetail, CheckFile, CheckResult, message};

/// C7 for one file, from what was seen at the last change. `None` unless a sync job changed a file that was ahead on NFS.
pub fn c7_overwritten_nfs_change(file: &CheckFile<'_>) -> Option<CheckResult> {
    let overwrite = file.overwrite?;
    if !matches!(
        overwrite.previous_state,
        DriftState::NfsAhead | DriftState::Conflict
    ) || overwrite.source != AttributionSource::SyncJob
        || overwrite.previous_nfs == Some(file.hash)
    {
        return None;
    }
    let before = if overwrite.previous_state == DriftState::Conflict {
        "in conflict"
    } else {
        "ahead on NFS"
    };
    let text = format!(
        "A sync replaced a change that was only on NFS (the file was {before}); the version it replaced can be restored."
    );
    Some(CheckResult {
        kind: FindingKind::OverwrittenNfsChange,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations: Vec::new(),
        detail: CheckDetail::OverwrittenNfsChange {
            previous_state: overwrite.previous_state,
            lost_version: overwrite.previous_nfs,
            source: overwrite.source,
        },
    })
}
