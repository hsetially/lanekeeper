//! The blob endpoint's policy (T6, T9, S11, D79, D104): a hash that is the content of a denied file is not served.
//!
//! `GET /api/v1/blobs/{hash}` is hub-platform's and serves any stored blob to a Viewer. Which hashes must never leave is the
//! registry's knowledge, and this is how the registry tells it: [`RegistryBlobPolicy`] implements hub-platform's
//! `BlobPolicy`, and a hash is refused when some file of some swimlane is denied *and* has that content now (the partial index
//! `file_state_denied_hash_idx`, one index probe) **or has ever had it** (T9): a file that becomes denied was an ordinary file before, its
//! earlier versions are in the store, and the history of the file names their hashes, so a policy that looked only at the version it has
//! now would be a way round the denial for every version before it. That second question is a probe of the index on the observations'
//! hashes and one primary-key probe of `file_state` for each observation that has the hash (a handful: the versions of files that hold
//! this content), and it stops at the first denied file. A hash nothing refers to is allowed: a blob is stored before anything points at
//! it, and the endpoint answers an unknown hash with 404 itself.
//!
//! **Fail closed.** If the question cannot be answered (the database is down, the statement timed out) the answer is "not allowed":
//! withholding a blob for a moment is a 404, serving a denied file's content once is a leak. The endpoint answers a refused hash exactly
//! as it answers an unknown one, so a caller cannot tell the two apart.

use async_trait::async_trait;
use domain::ContentHash;
use hub_platform::blobs::endpoint::BlobPolicy;
use sqlx::PgPool;
use tracing::warn;

use crate::db::Db;
use crate::metrics::RegistryMetrics;

/// Whether any denied file has this content, or has had it. `$1` is the hash; the predicates are those of the partial indexes
/// (`file_state_denied_hash_idx`, `file_observations_hash_idx`).
pub const DENIED_HASH_SQL: &str = "\
SELECT EXISTS (SELECT 1 FROM file_state WHERE nfs_hash = $1 AND denied AND nfs_hash IS NOT NULL) \
    OR EXISTS (SELECT 1 FROM file_observations o JOIN file_state f ON f.swimlane_id = o.swimlane_id AND f.path = o.path \
               WHERE o.hash = $1 AND o.hash IS NOT NULL AND f.denied)";

/// Refuses the hashes of denied files.
pub struct RegistryBlobPolicy {
    db: Db,
    metrics: RegistryMetrics,
}

impl std::fmt::Debug for RegistryBlobPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBlobPolicy").finish_non_exhaustive()
    }
}

impl RegistryBlobPolicy {
    #[must_use]
    pub fn new(pool: PgPool, config: &crate::RegistryConfig, metrics: RegistryMetrics) -> Self {
        Self {
            db: Db::new(pool, config.query_timeout),
            metrics,
        }
    }
}

#[async_trait]
impl BlobPolicy for RegistryBlobPolicy {
    async fn allowed(&self, hash: &ContentHash) -> bool {
        let denied: Result<bool, _> = self
            .db
            .run(
                "blob_policy",
                sqlx::query_scalar(DENIED_HASH_SQL)
                    .bind(hash.as_bytes().as_slice())
                    .fetch_one(self.db.pool()),
            )
            .await;
        if let Ok(denied) = denied {
            if denied {
                self.metrics.blob_withheld();
            }
            !denied
        } else {
            warn!(
                op = "blob_policy",
                "the policy could not be read: the blob is withheld"
            );
            false
        }
    }
}
