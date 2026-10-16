//! The blob endpoint's policy (T6, T9, S11, D79, D104): a hash that is the content of a denied file is not served.
//!
//! `GET /api/v1/blobs/{hash}` is hub-platform's and serves any stored blob to a Viewer. Which hashes must never leave is the
//! registry's knowledge, and this is how the registry tells it: [`RegistryBlobPolicy`] implements hub-platform's
//! `BlobPolicy`. A hash is refused when a file that has or has ever had that content is denied by **the one rule of
//! [`crate::denied::is_denied`]**: the stored flag, the class, or the hub's own deny globs. Two questions are asked, in this order:
//!
//! 1. **The flag.** Some file of some swimlane is flagged denied and has that content now: one probe of the partial index
//!    `file_state_denied_hash_idx`. This is [`DENIED_HASH_SQL`]. It reads nothing but `file_state`, so its cost does not depend on how many
//!    files have had the content, and a flagged file's hash is settled by it alone, however many other files have that content.
//! 2. **The rule, on the paths.** [`HASH_PATHS_SQL`] reads the files that have had the content, by `file_observations_hash_idx` (no
//!    other index is needed and none was added): for each, its path, its flag and class from `file_state` while the file exists, and
//!    from its observations when it does not (a file that was deleted loses its `file_state` row, so the flag is gone, but its newest
//!    observation that has a class still says whether it was denied). The rows go through `is_denied` with the hub's own globs. This
//!    is what closes three windows that the flag of the current content leaves open: a file that becomes denied was an ordinary file
//!    before, its earlier versions are in the store, and the history of the file names their hashes (the observation of an earlier
//!    version is read with the file's flag as it is now), so a policy that looked only at the version a file has now would be a way
//!    round the denial for every version before it; a hub that restarted with other `extra_deny_globs` while the agents have not yet
//!    reported the files again (the rows still say "ordinary", and every other read withholds the file because of the glob); and a
//!    denied file that was deleted (its old versions are still in the store and the history still names them).
//!
//!    There is no separate probe of the flag of the files that once had the content. It would have no bound (every observation of a popular
//!    hash, 20 to 37 ms at 100,000 and linear, for any viewer who knows the hash), and it would decide nothing this statement does not:
//!    the statement reads the same flag through its join, and it refuses a hash that has more sightings than it reads, so a denied
//!    sighting that is not among the rows it returns is refused all the same.
//!
//! **Bounded.** The second statement reads at most [`HASH_PATHS_LIMIT`] plus one rows. A hash that more observations than that have had
//! cannot be decided, and **an undecided hash is refused**: a 404 for content that was copied into more than a thousand places (at the
//! design scale the most common content has 49 observations) costs a viewer the endpoint's copy of it, where a guess that none of them
//! is denied could be the one leak (S10). The statement has the timeout of every registry statement.
//!
//! **Fail closed.** If a question cannot be answered (the database is down, a statement timed out, a row cannot be read, a path or a
//! class in it is not valid) the answer is "not allowed": withholding a blob for a moment is a 404, serving a denied file's content
//! once is a leak. The endpoint answers a refused hash exactly as it answers an unknown one, so a caller cannot tell the two apart. A
//! hash nothing refers to is allowed: a blob is stored before anything points at it, and the endpoint answers an unknown hash with
//! 404 itself.
//!
//! Nothing here logs or counts a hash, a path or a byte of content (S10): a refusal, and a hash that cannot be decided, is a counter (and at
//! most a debug line), because a viewer who knows a hash can ask for it at request rate; a failure to ask is a warning with a fixed word.

use async_trait::async_trait;
use domain::{ContentHash, FileClass, NfsPath};
use engine::DenyMatcher;
use hub_platform::blobs::endpoint::BlobPolicy;
use sqlx::{PgPool, Row as _};
use tracing::{debug, warn};

use crate::config::ConfigError;
use crate::db::Db;
use crate::metrics::RegistryMetrics;

/// Whether a denied file has this content now. `$1` is the hash; the predicate is that of the partial index `file_state_denied_hash_idx`,
/// so that this is one index probe. A file that had the content once is the paths statement's question ([`HASH_PATHS_SQL`]).
pub const DENIED_HASH_SQL: &str = "\
SELECT EXISTS (SELECT 1 FROM file_state WHERE nfs_hash = $1 AND denied AND nfs_hash IS NOT NULL)";

/// The most sightings of one hash that the policy reads to decide it. A hash that more than this many observations have had is refused as
/// undecided (see the module documentation). At the design scale (40 swimlanes of 2,000 files) the most common content has far fewer.
pub const HASH_PATHS_LIMIT: usize = 1_000;

/// The files that have had a content, one row for each observation of it: the path, the flag and the class that decide whether the file
/// is denied. `$1` is the hash and `$2` the row limit (`HASH_PATHS_LIMIT + 1`, so that a hash with more is seen to have more).
///
/// While the file exists its row in `file_state` decides (flag and class as they are now: a file that stopped being denied is readable
/// again). When it does not (it was deleted and the pipeline dropped its row) the class is the newest one the file's observations
/// recorded, found by the file's history index (`file_observations_history_idx`, a probe that stops at the first observation with a
/// class: a deletion has none); that observation's own class is the last resort. The statement reads `file_observations` through
/// `file_observations_hash_idx` and `file_state` by an index on its key, and no table by a scan (an `EXPLAIN` snapshot holds it).
pub const HASH_PATHS_SQL: &str = "\
SELECT o.path, COALESCE(f.denied, false) AS flagged, \
       COALESCE(f.class, \
                (SELECT n.class FROM file_observations n \
                 WHERE f.path IS NULL AND n.swimlane_id = o.swimlane_id AND n.path = o.path AND n.class IS NOT NULL \
                 ORDER BY n.observed_at DESC, n.id DESC LIMIT 1), \
                o.class) AS class \
FROM file_observations o \
LEFT JOIN file_state f ON f.swimlane_id = o.swimlane_id AND f.path = o.path \
WHERE o.hash = $1 AND o.hash IS NOT NULL \
LIMIT $2";

/// What the policy found out about a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// No file that has had the content is denied.
    Allowed,
    /// A file that has had the content is denied.
    Denied,
    /// The question could not be decided: more files have had the content than are read, or a row is not valid.
    Undecided,
}

/// One file that has had a content, as [`HASH_PATHS_SQL`] says it: its path, its flag, and its class (`None` when no observation of it
/// recorded one). No `Debug`: a path is not something to print by accident.
#[derive(Clone)]
struct Sighting {
    path: String,
    flagged: bool,
    class: Option<String>,
}

/// The one rule over the files that have had a content. Pure, so that each way to refuse is a unit test.
fn verdict(deny: &DenyMatcher, sightings: &[Sighting]) -> Verdict {
    if sightings.len() > HASH_PATHS_LIMIT {
        return Verdict::Undecided;
    }
    let mut undecided = false;
    for sighting in sightings {
        let Ok(path) = NfsPath::parse(&sighting.path) else {
            undecided = true;
            continue;
        };
        let class = match sighting.class.as_deref().map(str::parse::<FileClass>) {
            // No observation recorded a class: the flag and the hub's globs decide.
            None => FileClass::Structured,
            Some(Ok(class)) => class,
            Some(Err(_)) => {
                undecided = true;
                continue;
            }
        };
        if crate::denied::is_denied(deny, &path, sighting.flagged, class) {
            // A denied file settles it, whatever else could not be read.
            return Verdict::Denied;
        }
    }
    if undecided {
        Verdict::Undecided
    } else {
        Verdict::Allowed
    }
}

/// Refuses the hashes of denied files.
pub struct RegistryBlobPolicy {
    db: Db,
    deny: DenyMatcher,
    metrics: RegistryMetrics,
}

impl std::fmt::Debug for RegistryBlobPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBlobPolicy").finish_non_exhaustive()
    }
}

impl RegistryBlobPolicy {
    /// # Errors
    ///
    /// One of the configuration's `extra_deny_globs` is not a valid glob (every other component that holds the hub's globs refuses it the
    /// same way).
    pub fn new(
        pool: PgPool,
        config: &crate::RegistryConfig,
        metrics: RegistryMetrics,
    ) -> Result<Self, ConfigError> {
        let deny = DenyMatcher::with_globs(config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        Ok(Self {
            db: Db::new(pool, config.query_timeout),
            deny,
            metrics,
        })
    }

    /// The sightings of a hash, or `None` when they could not be read.
    async fn sightings(&self, hash: &ContentHash) -> Option<Vec<Sighting>> {
        let limit = i64::try_from(HASH_PATHS_LIMIT + 1).ok()?;
        let rows = self
            .db
            .run(
                "blob_policy_paths",
                sqlx::query(HASH_PATHS_SQL)
                    .bind(hash.as_bytes().as_slice())
                    .bind(limit)
                    .fetch_all(self.db.pool()),
            )
            .await
            .ok()?;
        rows.iter()
            .map(|row| {
                Some(Sighting {
                    path: row.try_get("path").ok()?,
                    flagged: row.try_get("flagged").ok()?,
                    class: row.try_get("class").ok()?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl BlobPolicy for RegistryBlobPolicy {
    async fn allowed(&self, hash: &ContentHash) -> bool {
        // The flag first: one index probe, and it answers a flagged file's hash however many files have that content.
        let flagged: Result<bool, _> = self
            .db
            .run(
                "blob_policy",
                sqlx::query_scalar(DENIED_HASH_SQL)
                    .bind(hash.as_bytes().as_slice())
                    .fetch_one(self.db.pool()),
            )
            .await;
        let Ok(flagged) = flagged else {
            warn!(
                op = "blob_policy",
                "the policy could not be read: the blob is withheld"
            );
            return false;
        };
        if flagged {
            self.metrics.blob_withheld();
            return false;
        }
        // Then the rule, on the files that have had the content: the hub's own globs, and the files whose rows are gone.
        let Some(sightings) = self.sightings(hash).await else {
            warn!(
                op = "blob_policy_paths",
                "the policy could not be read: the blob is withheld"
            );
            return false;
        };
        match verdict(&self.deny, &sightings) {
            Verdict::Allowed => true,
            Verdict::Denied => {
                self.metrics.blob_withheld();
                false
            }
            Verdict::Undecided => {
                // Expected for a content that was copied into more places than the policy reads, and any viewer who knows such a hash can
                // ask for it at request rate: it is counted, as every refusal is, and it is not a warning. The line says nothing of which
                // hash it was (S10).
                debug!(
                    op = "blob_policy_paths",
                    "the policy could not decide the content of this many files: the blob is withheld"
                );
                self.metrics.blob_withheld();
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(path: &str, flagged: bool, class: Option<&str>) -> Sighting {
        Sighting {
            path: path.to_owned(),
            flagged,
            class: class.map(str::to_owned),
        }
    }

    fn globs(globs: &[&str]) -> DenyMatcher {
        DenyMatcher::with_globs(globs.iter().copied()).unwrap()
    }

    #[test]
    fn the_one_rule_decides_each_file_that_has_had_the_content() {
        let none = DenyMatcher::default();
        let ordinary = seen("app/core.yml", false, Some("structured"));
        assert_eq!(verdict(&none, &[ordinary.clone()]), Verdict::Allowed);
        assert_eq!(verdict(&none, &[]), Verdict::Allowed, "a hash no file has had");
        // The flag, the class and the hub's globs each deny, as `is_denied` says.
        assert_eq!(
            verdict(&none, &[seen("app/core.yml", true, Some("structured"))]),
            Verdict::Denied
        );
        assert_eq!(
            verdict(&none, &[seen("app/core.yml", false, Some("denied"))]),
            Verdict::Denied
        );
        assert_eq!(
            verdict(&globs(&["*core*"]), &[ordinary.clone()]),
            Verdict::Denied,
            "the hub's own glob, whatever the row says"
        );
        assert_eq!(
            verdict(&globs(&["*other*"]), &[ordinary.clone()]),
            Verdict::Allowed
        );
        // One denied file among ordinary ones refuses the hash.
        assert_eq!(
            verdict(
                &none,
                &[ordinary.clone(), seen("keys/server.pem", false, None), ordinary]
            ),
            Verdict::Denied,
            "the engine's default globs are in every matcher"
        );
        // No class anywhere: the flag and the globs decide, so a file that was never classified is not a reason to refuse by itself.
        assert_eq!(verdict(&none, &[seen("a/b.yml", false, None)]), Verdict::Allowed);
    }

    #[test]
    fn what_cannot_be_read_is_undecided_and_a_denied_file_still_settles_it() {
        let none = DenyMatcher::default();
        assert_eq!(
            verdict(&none, &[seen("../escape", false, Some("structured"))]),
            Verdict::Undecided,
            "a path that is not valid"
        );
        assert_eq!(
            verdict(&none, &[seen("a/b.yml", false, Some("not-a-class"))]),
            Verdict::Undecided,
            "a class that is not one"
        );
        assert_eq!(
            verdict(
                &none,
                &[
                    seen("a/b.yml", false, Some("not-a-class")),
                    seen("a/c.yml", true, None)
                ]
            ),
            Verdict::Denied,
            "a denied file settles it, whatever else could not be read"
        );
    }

    #[test]
    fn more_sightings_than_are_read_are_undecided_even_when_none_is_denied() {
        let none = DenyMatcher::default();
        let one = seen("app/core.yml", false, Some("structured"));
        let at_the_limit = vec![one.clone(); HASH_PATHS_LIMIT];
        assert_eq!(verdict(&none, &at_the_limit), Verdict::Allowed);
        let over = vec![one; HASH_PATHS_LIMIT + 1];
        assert_eq!(verdict(&none, &over), Verdict::Undecided);
    }

    #[test]
    fn the_statement_is_bounded_and_keyed_by_the_hash() {
        assert!(HASH_PATHS_SQL.contains("LIMIT $2"));
        assert!(HASH_PATHS_SQL.contains("o.hash = $1 AND o.hash IS NOT NULL"));
        assert!(!HASH_PATHS_SQL.contains("OFFSET"));
    }

    #[test]
    fn the_probe_of_the_flag_is_one_index_probe_of_file_state_and_reads_no_observation() {
        // A second `EXISTS` over the observations had no bound: it read every observation of a popular hash (20 to 37 ms at 100,000, linear).
        assert_eq!(DENIED_HASH_SQL.matches("EXISTS").count(), 1);
        assert!(
            DENIED_HASH_SQL
                .contains("FROM file_state WHERE nfs_hash = $1 AND denied AND nfs_hash IS NOT NULL")
        );
        assert!(!DENIED_HASH_SQL.contains("file_observations"));
        assert!(!DENIED_HASH_SQL.contains("JOIN"));
    }
}
