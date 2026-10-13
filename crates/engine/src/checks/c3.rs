//! C3 `uneven_tenant_files` (domain-model, "Consistency checks"): tenant files exist for some of the swimlane's tenants and
//! not for others. Informational: the tenants without a file read base (or nothing, if there is no base file).

use std::collections::{BTreeMap, BTreeSet};

use domain::{FindingKind, LogicalFile, TenantId, TenantSet};

use super::{CheckDetail, CheckFile, CheckResult, deployed_tenant, message};

/// C3 over the swimlane's files: one result per logical file that some deployed tenants have a file for and others do not.
/// A swimlane with one tenant has no unevenness. The result's path is the logical file.
pub fn c3_uneven_tenant_files(files: &[CheckFile<'_>], tenants: &TenantSet) -> Vec<CheckResult> {
    if tenants.len() < 2 {
        return Vec::new();
    }
    let mut having: BTreeMap<&LogicalFile, BTreeSet<&TenantId>> = BTreeMap::new();
    for file in files {
        if let Some(tenant) = deployed_tenant(file, tenants) {
            having.entry(&file.facts.logical).or_default().insert(tenant);
        }
    }
    let mut out = Vec::new();
    for (logical, with) in having {
        if with.len() == tenants.len() {
            continue;
        }
        let without_file: Vec<TenantId> = tenants.iter().filter(|t| !with.contains(t)).cloned().collect();
        let with_file: Vec<TenantId> = with.into_iter().cloned().collect();
        let text = format!(
            "Tenant files exist for {} but not for {}.",
            names(&with_file),
            names(&without_file)
        );
        let Some(message) = message(&text) else { continue };
        out.push(CheckResult {
            kind: FindingKind::UnevenTenantFiles,
            path: Some(logical.as_path().clone()),
            message,
            locations: Vec::new(),
            detail: CheckDetail::UnevenTenantFiles {
                with_file,
                without_file,
            },
        });
    }
    out
}

fn names(tenants: &[TenantId]) -> String {
    tenants
        .iter()
        .map(TenantId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
