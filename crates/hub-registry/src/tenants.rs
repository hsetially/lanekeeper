//! The tenants of a swimlane (T10, D84, D105; Q34, A30).
//!
//! A swimlane deploys a **set** of tenants, never exactly one (Q34), and a tenant id is the name of its branch in the tenant repository
//! (D84). This module is where the registry says what that set is, who may change it, and which member stands for the swimlane where a
//! view needs one.
//!
//! # Where the set comes from
//!
//! | Source | What it is | What it does |
//! |---|---|---|
//! | the adoption pass (T5, `crate::adoption`) | an admin confirms the branches the swimlane was deployed from | the **only writer of `swimlane_tenants`** in this crate: one transaction with the baselines, one `SettingsChanged` audit event when the set changes |
//! | release and environment hints (`crate::adoption::hints`, Q19) | the Helm chart names and `SPRING_PROFILES_ACTIVE` of the Deployments name a deployable branch | **evidence** shown to the admin in the pass; never membership: the hub does not add a tenant because the cluster mentions one |
//! | suffixes seen on NFS (`crate::pipeline::tenant_rematch`, C4) | a tenant file whose suffix is a deployable branch the set does not hold | **evidence** that raises a review (an `adoptions` row, `tenant_suffix_changed`) and a C4 finding; never membership |
//! | an admin's change | an edit of the set outside the pass | goes through `swimlane_tenants` and nothing else: the pipeline's rules fingerprint holds the set, so the next batch computes the whole swimlane again. No route exists for it yet (A7); this crate never writes the table for it |
//!
//! So the set is one table, read by everything that needs it ([`stored`] for the pickup computation; the pipeline, ingestion and the read
//! side read the same rows with their own statements and the same order), and nothing decides it on its own. The checks use the **whole**
//! set: C3 (`uneven_tenant_files`) compares each tenant's copies with the others', C4 (`orphan_file`) flags a suffix that is not in it. The
//! engine takes the set as `LaneInput::tenants`; `tests/tenants.rs` holds that to the rows.
//!
//! # The tenant that stands for a swimlane (A30)
//!
//! A view of one swimlane that needs one tenant (the effective config, a column of the grid, a side of a comparison) takes the **first
//! tenant in sorted order**, and the answer names it (the `X-Lanekeeper-Tenants` header, `read::routes`). [`standing`] is that rule and the
//! only place it is written: it orders by the tenant id's own `Ord` (the bytes of the id), which is what a [`TenantSet`] iterates in. It
//! does not trust the order of the rows it is given. A database's `ORDER BY tenant_id` follows the collation of the cluster, and under
//! `en_US.UTF-8` a hyphen is ignored when ordering, so `sit-b` sorts after `sit1` there and before it in bytes: two views that each took
//! "the first row" could name different tenants for the same swimlane.
//!
//! # Which files a swimlane reads
//!
//! A tenant file whose suffix is not one of the swimlane's tenants is read by nobody (the engine's `NeverRead`): no service asks for it, and
//! a change to it changes nothing for anybody. [`tenant_suffix`] finds the suffix of a stored file from its path and its logical file, and
//! [`is_read`] is the question. The pickup computation uses them so that such a file is never pending.

use domain::{LogicalFile, NfsPath, SwimlaneId, TenantId, TenantSet};

use crate::db::{Db, Txn};
use crate::error::RegistryError;

/// The statement that reads a swimlane's stored set: `$1` the swimlane, `$2` the limit. One row more than a set may hold, so that a set that
/// is over the bound is seen as such.
pub const STORED_SQL: &str =
    "SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id LIMIT $2";

/// The tenants a swimlane has stored, in the caller's transaction. A row that is not a tenant id is left out (nothing writes one; the
/// column's CHECK is the only bound on it), and a set over [`TenantSet::MAX_TENANTS`] is [`RegistryError::Corrupt`].
///
/// # Errors
///
/// The database failed, or the set is too large.
pub(crate) async fn stored(db: &Db, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<TenantSet, RegistryError> {
    let ids: Vec<String> = db
        .run(
            "tenants_stored",
            sqlx::query_scalar(STORED_SQL)
                .bind(swimlane.as_str())
                .bind(i64::try_from(TenantSet::MAX_TENANTS + 1).unwrap_or(i64::MAX))
                .fetch_all(&mut **tx),
        )
        .await?;
    TenantSet::new(ids.iter().filter_map(|t| TenantId::parse(t).ok())).map_err(|_| RegistryError::Corrupt)
}

/// The tenant that stands for a swimlane in the effective config, the grid and a comparison (A30): the first of `tenants` in sorted
/// order, whatever order they are given in. `None` for a swimlane with no tenant.
pub fn standing<'a>(tenants: impl IntoIterator<Item = &'a TenantId>) -> Option<&'a TenantId> {
    tenants.into_iter().min()
}

/// The suffix a tenant copy of a file carries: `core-sit1.yml` of the logical file `core.yml` is `sit1`. `None` for the base file itself,
/// for a file in another folder than its logical file, or when what follows the logical name is not a tenant id.
///
/// The cut is the path mapping's: a name splits into stem and extension at its last dot (a leading or a trailing dot is no extension), and
/// the suffix goes between them, or at the end of a name with no extension.
pub fn tenant_suffix(path: &NfsPath, logical: &LogicalFile) -> Option<TenantId> {
    fn split(name: &str) -> (&str, Option<&str>) {
        match name.rfind('.') {
            Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
            _ => (name, None),
        }
    }
    let base = logical.as_path();
    if path.parent() != base.parent() {
        return None;
    }
    let (stem, extension) = split(path.file_name());
    let (base_stem, base_extension) = split(base.file_name());
    if extension != base_extension {
        return None;
    }
    let suffix = stem.strip_prefix(base_stem)?.strip_prefix('-')?;
    TenantId::parse(suffix).ok()
}

/// Whether the swimlane reads a file that carries `suffix`: a file with none (a base file) is read, and a tenant copy is read when the
/// tenant is one of the swimlane's.
pub fn is_read(suffix: Option<&TenantId>, tenants: &TenantSet) -> bool {
    suffix.is_none_or(|t| tenants.contains(t))
}

#[cfg(test)]
mod tests {
    use domain::PathMappingRule;
    use engine::classify_path;

    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId::parse(s).unwrap()
    }

    fn path(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    fn logical(s: &str) -> LogicalFile {
        LogicalFile::parse(s).unwrap()
    }

    fn set(names: &[&str]) -> TenantSet {
        TenantSet::new(names.iter().map(|n| tenant(n))).unwrap()
    }

    #[test]
    fn the_first_tenant_in_sorted_order_stands_whatever_order_the_rows_come_in() {
        let given = [tenant("sit3"), tenant("sit1"), tenant("sit2")];
        assert_eq!(standing(&given).map(TenantId::as_str), Some("sit1"));
        let reversed: Vec<TenantId> = given.iter().rev().cloned().collect();
        assert_eq!(standing(&reversed), standing(&given));
        assert_eq!(
            standing(&set(&["b", "a"]).iter().cloned().collect::<Vec<_>>()),
            Some(&tenant("a"))
        );
        assert_eq!(standing(&Vec::<TenantId>::new()), None, "no tenant, none stands");
    }

    #[test]
    fn the_order_is_the_ids_own_and_not_a_collations() {
        // In bytes `-` < `1` < `_` < `a`. A cluster under en_US.UTF-8 ignores the hyphen and would put `sit-b` after `sit1`; the rule
        // must not depend on it, and must be the order a `TenantSet` iterates in.
        let rows = [tenant("sit1"), tenant("sit-b"), tenant("sit_a"), tenant("sita")];
        assert_eq!(standing(&rows).map(TenantId::as_str), Some("sit-b"));
        let as_set = TenantSet::new(rows.iter().cloned()).unwrap();
        assert_eq!(standing(as_set.iter()), as_set.iter().next());
    }

    #[test]
    fn the_suffix_is_what_the_path_adds_to_its_logical_file() {
        let cases = [
            ("a/core-sit1.yml", "a/core.yml", Some("sit1")),
            (
                "a/tx-infinity-core-sit1.yml",
                "a/tx-infinity-core.yml",
                Some("sit1"),
            ),
            (
                "a/tx-infinity-core-sit1-b.yml",
                "a/tx-infinity-core.yml",
                Some("sit1-b"),
            ),
            ("a/Dockerfile-sit2", "a/Dockerfile", Some("sit2")),
            ("core-sit1.yml", "core.yml", Some("sit1")),
            // The base file, another extension, another folder, a suffix that is no tenant id.
            ("a/core.yml", "a/core.yml", None),
            ("a/core-sit1.yaml", "a/core.yml", None),
            ("b/core-sit1.yml", "a/core.yml", None),
            ("a/core-SIT1.yml", "a/core.yml", None),
            ("a/core-.yml", "a/core.yml", None),
            ("a/core2.yml", "a/core.yml", None),
        ];
        for (p, l, expected) in cases {
            assert_eq!(
                tenant_suffix(&path(p), &logical(l))
                    .as_ref()
                    .map(TenantId::as_str),
                expected,
                "{p} of {l}"
            );
        }
    }

    #[test]
    fn the_suffix_agrees_with_the_engines_reading_of_a_path() {
        // The engine's own cut (`classify_path`) is the truth: whatever tenant it finds in a name, `tenant_suffix` names the same one for the
        // logical file it found (the pipeline classifies against every deployable branch, which is the set given here).
        let rule = PathMappingRule::default();
        let deployable = set(&["sit1", "sit1-b", "sit2", "sit3"]);
        for p in [
            "a/core-sit1.yml",
            "a/core-sit1-b.yml",
            "a/tx-infinity-core-sit2.yml",
            "a/core.yml",
            "Dockerfile-sit1",
            "a/.hidden-sit1",
            "a/archive.tar-sit2.gz",
            "a/core-sit3.yml",
            "a/core-sit9.yml",
        ] {
            let file = classify_path(&rule, &path(p), &deployable, |_| true).unwrap();
            assert_eq!(
                tenant_suffix(&path(p), &file.logical),
                file.tenant,
                "{p}: the engine reads it as {:?} of {}",
                file.kind,
                file.logical
            );
        }
    }

    #[test]
    fn a_tenant_copy_is_read_only_when_the_swimlane_deploys_the_tenant() {
        let deployed = set(&["sit1", "sit2"]);
        assert!(is_read(None, &deployed), "a base file is read");
        assert!(is_read(Some(&tenant("sit2")), &deployed));
        assert!(
            !is_read(Some(&tenant("sit9")), &deployed),
            "nobody asks for a tenant that is not deployed"
        );
        assert!(
            !is_read(Some(&tenant("sit1")), &TenantSet::default()),
            "a swimlane with no tenant reads no tenant copy"
        );
    }
}
