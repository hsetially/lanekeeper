//! C4 `orphan_file` (domain-model, "Consistency checks"): a file that nothing will use.
//!
//! - a tenant file whose suffix is a branch the swimlane does not deploy: no service of the swimlane asks for it (D84);
//! - an untracked file: Git has no such file, so it is outside the review process.

use domain::{FileKind, FindingKind, TenantSet};

use super::{CheckDetail, CheckFile, CheckResult, OrphanReason, message};

/// C4 for one file. The suffix comes first: a file that is both untracked and for another branch is reported for the
/// suffix, which is the stronger statement.
pub fn c4_orphan_file(file: &CheckFile<'_>, tenants: &TenantSet) -> Option<CheckResult> {
    let (reason, text) = match &file.facts.tenant {
        Some(tenant) if !tenants.contains(tenant) => (
            OrphanReason::TenantNotDeployed(tenant.clone()),
            format!(
                "The name carries the suffix of {tenant}, which this swimlane does not deploy, so no service reads the file."
            ),
        ),
        _ if file.facts.kind == FileKind::Untracked => (
            OrphanReason::Untracked,
            "Git has no such file: it is only on NFS.".to_owned(),
        ),
        _ => return None,
    };
    Some(CheckResult {
        kind: FindingKind::OrphanFile,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations: Vec::new(),
        detail: CheckDetail::OrphanFile { reason },
    })
}
