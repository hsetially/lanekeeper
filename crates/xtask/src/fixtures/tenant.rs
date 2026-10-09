//! Tenant forks: which base files a tenant branch copies, and how each copy differs from base.

use super::content::{Doc, HEAD_VERSION};
use super::rng::Rng;

/// How a tenant copy relates to the base file at its head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkKind {
    /// Byte-identical to base (check C2).
    Redundant,
    /// Copied at an older version: the entries added since are missing (check C1).
    Stale(u8),
    /// Tenant-specific content.
    Modified,
}

#[derive(Debug, Clone, Copy)]
pub struct TenantFile {
    /// Index into the catalog.
    pub doc: usize,
    pub kind: ForkKind,
}

#[derive(Debug, Clone)]
pub struct TenantPlan {
    pub id: String,
    pub files: Vec<TenantFile>,
}

/// Files every tenant copies. For `sit1` the kind is fixed to match the real-data examples (C1, C2).
const FORCED: &[&str] = &[
    "tx-infinity-api/entryGroupConfig.yml",
    "tx-infinity-api/account-sorting-config.yml",
    "tx-infinity-api/miniStatementConfig.yml",
    "document-service/resources/receipt-ci_1.bmp",
    "tx-infinity-api/tx-infinity-core.yml",
    "jwt-proxy-injector-service/mappingsItemEvaluationE2ETest",
    "ui/remote-itm-teller/ui-common-config.yaml",
    "security/security-roles.yml",
    "application.yml",
];

fn sit1_kind(path: &str) -> Option<ForkKind> {
    match path {
        "tx-infinity-api/entryGroupConfig.yml" => Some(ForkKind::Stale(2)),
        "tx-infinity-api/account-sorting-config.yml"
        | "tx-infinity-api/miniStatementConfig.yml"
        | "document-service/resources/receipt-ci_1.bmp" => Some(ForkKind::Redundant),
        "tx-infinity-api/tx-infinity-core.yml"
        | "jwt-proxy-injector-service/mappingsItemEvaluationE2ETest" => Some(ForkKind::Modified),
        _ => None,
    }
}

fn random_kind(rng: &mut Rng, doc: &Doc) -> ForkKind {
    let roll = rng.below(100);
    if doc.format == super::content::Format::Binary {
        return if roll < 15 {
            ForkKind::Redundant
        } else {
            ForkKind::Modified
        };
    }
    if roll < 8 {
        ForkKind::Redundant
    } else if roll < 20 && doc.born == 1 && doc.has_later_chunks() {
        let newest = doc.newest_chunk_version();
        ForkKind::Stale(if rng.chance(50) { 1 } else { newest - 1 })
    } else {
        ForkKind::Modified
    }
}

/// Chooses the `count` files tenant `tenant` copies. `root` is the generator's root stream.
#[must_use]
pub fn plan(root: &Rng, tenant: &str, count: usize, docs: &[Doc]) -> TenantPlan {
    let mut rng = root.fork(&format!("tenant/{tenant}"));
    let mut files = Vec::with_capacity(count);
    let mut taken = vec![false; docs.len()];

    for forced in FORCED {
        if let Some(i) = docs.iter().position(|d| d.path == *forced) {
            let kind = if tenant == "sit1" { sit1_kind(forced) } else { None }
                .unwrap_or_else(|| random_kind(&mut rng, &docs[i]));
            files.push(TenantFile { doc: i, kind });
            taken[i] = true;
        }
    }
    let mut rest: Vec<usize> = (0..docs.len())
        .filter(|&i| !taken[i] && docs[i].path != "channels.yml")
        .collect();
    rng.shuffle(&mut rest);
    for i in rest {
        if files.len() >= count {
            break;
        }
        files.push(TenantFile {
            doc: i,
            kind: random_kind(&mut rng, &docs[i]),
        });
    }
    files.sort_by_key(|f| f.doc);
    TenantPlan {
        id: tenant.to_owned(),
        files,
    }
}

/// The bytes of a tenant copy.
#[must_use]
pub fn render(doc: &Doc, kind: ForkKind, tenant: &str) -> Vec<u8> {
    match kind {
        ForkKind::Redundant => doc.render(HEAD_VERSION, None),
        ForkKind::Stale(v) => doc.render(v, Some(tenant)),
        ForkKind::Modified => doc.render(HEAD_VERSION, Some(tenant)),
    }
}
