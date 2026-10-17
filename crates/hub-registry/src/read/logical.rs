//! The logical file and the tenant of the files a read names by path (`contract/08-web`, CG2).
//!
//! `tree`, the drift list and the settings search read the classification (`file_state.logical_file`) in the rows they read already. The
//! findings and the text search name a file by swimlane and path only, so this module reads the classification of those places in one
//! statement, however many there are: a bounded `unnest` joined to the primary key `(swimlane_id, path)`.
//!
//! # What a place with no row gets
//!
//! A finding can name a path that is not a tracked file: a folder (C11), the logical path of a check that spans copies (C3 names the logical
//! file itself, which can exist as tenant copies only), or a file that is gone. The registry holds no classification for it, and invents none:
//!
//! - the one finding kind whose path is a logical file *by construction* (`uneven_tenant_files`, `engine::checks::c3`) names it as its own
//!   logical file, with no tenant;
//! - any other place with no row has neither (`None`), which a client reads as "no tracked file here". A text hit always has a row: the corpus
//!   is made from the rows, and a file removed since is a hit that names no logical file, not an error.

use std::collections::HashMap;

use domain::{FindingKind, LogicalFile, NfsPath, SwimlaneId, TenantId};
use ports::ReadError;
use sqlx::Row;

use super::rows;
use super::service::Inner;

/// The classification of places: `$1` the swimlanes and `$2` the paths, side by side (the same length). One probe of the primary key each.
pub const PLACES_LOGICAL_SQL: &str = "\
SELECT fs.swimlane_id, fs.path, fs.logical_file \
FROM unnest($1::text[], $2::text[]) AS h(lane, path) \
JOIN file_state fs ON fs.swimlane_id = h.lane AND fs.path = h.path";

/// The most places one statement is asked about. A page is at most 500 findings and a text search at most `Limits::hits_total` hits (500 by
/// default); the cap keeps a larger configuration from sending an unbounded array (rule 5), and the places beyond it get none.
pub const MAX_PLACES: usize = 2_000;

/// A file's logical file and tenant.
pub type Names = (Option<LogicalFile>, Option<TenantId>);

/// The classifications that were found, by `(swimlane, path)`.
#[derive(Debug)]
pub struct Found(HashMap<(String, String), (LogicalFile, Option<TenantId>)>);

impl Found {
    /// The names of the file at `path` in `swimlane`: those the row holds, or none.
    pub fn of(&self, swimlane: &SwimlaneId, path: &NfsPath) -> Names {
        match self
            .0
            .get(&(swimlane.as_str().to_owned(), path.as_str().to_owned()))
        {
            Some((logical, tenant)) => (Some(logical.clone()), tenant.clone()),
            None => (None, None),
        }
    }
}

/// What a finding of this kind names when its path has no row: the logical file itself for the one kind whose path is one, else nothing.
pub fn finding_fallback(kind: FindingKind, path: &NfsPath) -> Names {
    if kind == FindingKind::UnevenTenantFiles {
        (LogicalFile::from_nfs(path.clone()).ok(), None)
    } else {
        (None, None)
    }
}

impl Inner {
    /// The classification of these places, in one statement (none when the list is empty). Places past [`MAX_PLACES`] are not asked about.
    pub async fn logical_of(&self, places: &[(&SwimlaneId, &NfsPath)]) -> Result<Found, ReadError> {
        let mut seen: Vec<(&str, &str)> = places
            .iter()
            .take(MAX_PLACES)
            .map(|(s, p)| (s.as_str(), p.as_str()))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        let mut found = HashMap::with_capacity(seen.len());
        if seen.is_empty() {
            return Ok(Found(found));
        }
        let lanes: Vec<&str> = seen.iter().map(|(l, _)| *l).collect();
        let paths: Vec<&str> = seen.iter().map(|(_, p)| *p).collect();
        let fetched = self
            .db
            .run(
                "read_logical_files",
                sqlx::query(PLACES_LOGICAL_SQL)
                    .bind(&lanes)
                    .bind(&paths)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        for row in &fetched {
            let lane: String = row.get("swimlane_id");
            let path: String = row.get("path");
            let logical: String = row.get("logical_file");
            let parsed = NfsPath::parse(&path).map_err(|_| crate::error::RegistryError::Corrupt)?;
            found.insert((lane, path), rows::logical_and_tenant(&parsed, &logical)?);
        }
        Ok(Found(found))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    #[test]
    fn only_the_uneven_tenant_files_check_names_a_logical_file_without_a_row() {
        let (logical, tenant) = finding_fallback(FindingKind::UnevenTenantFiles, &path("a/core.yml"));
        assert_eq!(logical.as_ref().map(LogicalFile::as_str), Some("a/core.yml"));
        assert_eq!(tenant, None);
        for kind in FindingKind::ALL {
            if kind != FindingKind::UnevenTenantFiles {
                assert_eq!(
                    finding_fallback(kind, &path("a/core.yml")),
                    (None, None),
                    "{kind:?}: a place with no row is no tracked file"
                );
            }
        }
    }
}
