//! C2 `redundant_tenant_file` (domain-model, "Consistency checks"): a tenant file that is the base file, said twice.
//!
//! It has no effect today, and it keeps the branch from receiving future changes to base (the file replaces base in
//! `whole_file` mode), so it should be deleted. Structured files are equal when their settings are (order, comments and
//! quoting do not count; `yes` and `true` are different values). Every other file is equal when its hash is.
//!
//! The settings of a structured file are the pairs that `flatten` could make into a setting. A pair it had to leave out (a
//! `.properties` key that is empty or has a control character, a YAML key that is not a scalar) is not among them, so two
//! files can have the same settings and differ in what they left out: such a file is equal by its bytes or not at all.

use domain::{FindingKind, TenantSet};

use super::{CheckDetail, CheckFile, CheckResult, EqualBy, Structured, deployed_tenant, message};
use crate::diff::semantic_diff;
use crate::parse::ParseWarning;

/// True when `flatten` left a pair out of the file's settings, or may have: it kept at most `Limits::warnings` warnings, and
/// the ones it dropped are not known.
fn left_pairs_out(file: &Structured<'_>) -> bool {
    file.parse_warnings_dropped > 0
        || file.parse_warnings.iter().any(|w| {
            matches!(
                w,
                ParseWarning::UnusableKeySkipped { .. } | ParseWarning::ComplexKeySkipped { .. }
            )
        })
}

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
        if left_pairs_out(own)
            || left_pairs_out(other)
            || !semantic_diff(own.settings, other.settings).is_empty()
        {
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
