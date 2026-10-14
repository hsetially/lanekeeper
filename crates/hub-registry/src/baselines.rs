//! What every writer of baselines shares (A24, D78): the reference that keeps the raw version behind a baseline from being collected.
//!
//! A baseline row holds the hash for drift (`hash`) and the raw version the baseline was taken from (`blob_hash`). The raw version is
//! what "restore a lost version" needs, and retention may delete a blob nothing refers to (D78), so a baseline that names a blob also
//! has a `blob_refs` row: `ref_kind` `baseline`, `ref_id` `"{swimlane}:{path}"`. Three places write baselines and all three do it
//! through [`replace_refs`], so they cannot disagree about it:
//!
//! - the sync report (`crate::sync`, T4): files that equal Git at the commits the report names;
//! - the pipeline (`crate::pipeline::persist`, T3): a path where NFS and Git converge on an adopted swimlane;
//! - the adoption pass (`crate::adoption`, T5): a confirmation.
//!
//! The rule: a baseline that replaces another drops the reference of the version it replaces (unless it is the same version), and a
//! baseline that has a blob holds one reference for it. A denied file has no blob (D79: its content is never stored), so it has no
//! reference. A reference is only written for a blob that is stored: the foreign key would refuse the row, and a baseline must never
//! be what makes a swimlane's commit fail.

use std::collections::BTreeMap;

use domain::SwimlaneId;

use crate::db::{Db, Txn};
use crate::error::RegistryError;

/// The raw versions behind the baselines of some paths, as they are now (read before the baselines are written).
pub const CURRENT_BLOBS_SQL: &str = "\
SELECT path, blob_hash::bytea FROM baselines \
WHERE swimlane_id = $1 AND path = ANY($2::text[]) AND blob_hash IS NOT NULL";

/// Drop the references of the versions that baselines replaced.
pub const DROP_REFS_SQL: &str = "\
DELETE FROM blob_refs r USING unnest($1::bytea[], $2::text[]) AS x(hash, ref_id) \
WHERE r.hash = x.hash AND r.ref_kind = 'baseline' AND r.ref_id = x.ref_id";

/// Reference the raw versions behind the new baselines, for the blobs that are stored.
pub const INSERT_REFS_SQL: &str = "\
INSERT INTO blob_refs (hash, ref_kind, ref_id) \
SELECT x.hash, 'baseline', x.ref_id FROM unnest($1::bytea[], $2::text[]) AS x(hash, ref_id) \
WHERE EXISTS (SELECT 1 FROM blobs b WHERE b.hash = x.hash) \
ON CONFLICT DO NOTHING";

/// One path whose baseline is written: the raw version it had, and the one it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefChange {
    pub path: String,
    /// `baselines.blob_hash` before the write.
    pub old: Option<Vec<u8>>,
    /// The raw version behind the new baseline. `None`: a denied file, or a version that is not stored.
    pub new: Option<Vec<u8>>,
}

/// The name of a path's reference.
pub(crate) fn ref_id(swimlane: &SwimlaneId, path: &str) -> String {
    format!("{swimlane}:{path}")
}

/// The raw versions behind the baselines of `paths`, by path. Read inside the transaction that is about to replace them.
pub(crate) async fn current_blobs(
    db: &Db,
    tx: &mut Txn,
    swimlane: &SwimlaneId,
    paths: &[&str],
) -> Result<BTreeMap<String, Vec<u8>>, RegistryError> {
    let rows: Vec<(String, Vec<u8>)> = db
        .run(
            "baseline_blobs",
            sqlx::query_as(CURRENT_BLOBS_SQL)
                .bind(swimlane.as_str())
                .bind(paths)
                .fetch_all(&mut **tx),
        )
        .await?;
    Ok(rows.into_iter().collect())
}

/// Move the references to the new versions: drop the one of each version that a baseline replaced, add the one of each version that a
/// baseline now stands on. Call it in the transaction of the baselines, after they are written.
pub(crate) async fn replace_refs(
    db: &Db,
    tx: &mut Txn,
    swimlane: &SwimlaneId,
    changes: &[RefChange],
) -> Result<(), RegistryError> {
    let (mut old_hashes, mut old_ids): (Vec<Vec<u8>>, Vec<String>) = (Vec::new(), Vec::new());
    let (mut new_hashes, mut new_ids): (Vec<Vec<u8>>, Vec<String>) = (Vec::new(), Vec::new());
    for change in changes {
        if let Some(old) = &change.old {
            if change.new.as_ref() != Some(old) {
                old_hashes.push(old.clone());
                old_ids.push(ref_id(swimlane, &change.path));
            }
        }
        if let Some(new) = &change.new {
            new_hashes.push(new.clone());
            new_ids.push(ref_id(swimlane, &change.path));
        }
    }
    if !old_hashes.is_empty() {
        db.run(
            "baseline_refs_drop",
            sqlx::query(DROP_REFS_SQL)
                .bind(&old_hashes)
                .bind(&old_ids)
                .execute(&mut **tx),
        )
        .await?;
    }
    if !new_hashes.is_empty() {
        db.run(
            "baseline_refs",
            sqlx::query(INSERT_REFS_SQL)
                .bind(&new_hashes)
                .bind(&new_ids)
                .execute(&mut **tx),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_named_by_the_swimlane_and_the_path() {
        let lane = SwimlaneId::parse("sit1").unwrap();
        assert_eq!(ref_id(&lane, "app/core.yml"), "sit1:app/core.yml");
    }

    #[test]
    fn the_statements_name_every_bind_they_use() {
        for (sql, last) in [(CURRENT_BLOBS_SQL, 2), (DROP_REFS_SQL, 2), (INSERT_REFS_SQL, 2)] {
            assert!(sql.contains(&format!("${last}")), "{sql}");
            assert!(!sql.contains(&format!("${}", last + 1)), "{sql}");
        }
        for sql in [CURRENT_BLOBS_SQL, DROP_REFS_SQL, INSERT_REFS_SQL] {
            assert!(!sql.contains("OFFSET"), "{sql}");
        }
    }
}
