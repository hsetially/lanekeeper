//! The blob endpoint's policy (T6, S11, D79): a hash that is the content of a denied file is not served.
//!
//! `GET /api/v1/blobs/{hash}` is hub-platform's and serves any stored blob to a Viewer. Which hashes must never leave is the
//! registry's knowledge, and this is how the registry tells it: [`RegistryBlobPolicy`] implements hub-platform's
//! `BlobPolicy`, and a hash is refused when some file of some swimlane is denied *and* has that content (the partial index
//! `file_state_denied_hash_idx`, one index probe). A hash nothing refers to is allowed: a blob is stored before anything points at it,
//! and the endpoint answers an unknown hash with 404 itself.
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

/// Whether any denied file has this content. `$1` is the hash; the predicates are the partial index's own.
pub const DENIED_HASH_SQL: &str =
    "SELECT EXISTS (SELECT 1 FROM file_state WHERE nfs_hash = $1 AND denied AND nfs_hash IS NOT NULL)";

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
