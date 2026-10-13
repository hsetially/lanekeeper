//! C2 `redundant_tenant_file` (domain-model, "Consistency checks"): a tenant file that is the base file, said twice.
//!
//! It has no effect today, and it keeps the branch from receiving future changes to base (the file replaces base in
//! `whole_file` mode), so it should be deleted. Structured files are equal when their settings are (order, comments and
//! quoting do not count; `yes` and `true` are different values). Every other file is equal when its hash is.

use domain::{FindingKind, TenantSet};

use super::{CheckDetail, CheckFile, CheckResult, EqualBy, deployed_tenant, message};
use crate::diff::semantic_diff;

/// C2 for one tenant file of a tenant that the swimlane deploys. `base` is the base file of the same logical file on the
/// swimlane's NFS; without it there is nothing to be equal to.
pub fn c2_redundant_tenant_file(
    file: &CheckFile<'_>,
    base: Option<&CheckFile<'_>>,
    tenants: &TenantSet,
) -> Option<CheckResult> {
    let tenant = deployed_tenant(file, tenants)?;
    let base = base?;
    let equal_by = if file.hash == base.hash {
        EqualBy::Bytes
    } else {
        let (own, other) = (file.structured.as_ref()?, base.structured.as_ref()?);
        if !semantic_diff(own.settings, other.settings).is_empty() {
            return None;
        }
        EqualBy::Settings
    };
    let how = match equal_by {
        EqualBy::Bytes => "is identical to",
        EqualBy::Settings => "has the same settings as",
    };
    let text = format!(
        "The file for {tenant} {how} the base file, so it only keeps {tenant} from base changes; it can be deleted."
    );
    Some(CheckResult {
        kind: FindingKind::RedundantTenantFile,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations: Vec::new(),
        detail: CheckDetail::RedundantTenantFile {
            tenant: tenant.clone(),
            equal_by,
        },
    })
}
