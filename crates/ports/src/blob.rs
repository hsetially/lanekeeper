//! The blob store port: content-addressed file versions (D47).

use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;
use sha2::{Digest, Sha256};

/// The largest blob a store must accept: a 2 MiB file (`docs/performance.md`) with room for a doc upload.
pub const MAX_BLOB_BYTES: usize = 4 * 1024 * 1024;

/// The most hashes one `get_many` call may ask for.
pub const MAX_GET_MANY: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    #[error("blob is too large")]
    TooLarge,
    #[error("too many blobs requested")]
    TooMany,
    /// The store is at its capacity limit.
    #[error("blob store is full")]
    Full,
    #[error("blob store is unavailable")]
    Unavailable,
}

/// The SHA-256 of `bytes`, which is the blob's address.
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    ContentHash::from_bytes(Sha256::digest(bytes).into())
}

#[async_trait]
pub trait BlobStore: Send + Sync + 'static {
    /// Idempotent: storing the same bytes twice returns the same hash and stores them once. The hash is
    /// [`content_hash`] of the bytes exactly as given (line endings are not normalised here).
    ///
    /// **Storing a blob that already exists refreshes its age.** A store that keeps an age per blob (the Postgres store does:
    /// `blobs.touched_at`, migration `0008`) sets it to now when the blob is stored, and again whenever the same bytes are stored
    /// once the age is older than the store's refresh interval (an hour there). The interval rate-limits the refresh, so that a hot
    /// blob that is stored a thousand times is written once and a re-store of a recent blob writes nothing; the age therefore trails
    /// the last `put` by at most that interval, plus the wait for a row lock when the blob is being refreshed or collected at that
    /// moment (bounded by the put's own timeout), and is never ahead of it. Why: a writer stores a blob *before* the transaction that
    /// references it commits (an ingest delta, a staged part of one that waits up to 24 hours, a draft), and the registry's
    /// retention deletes blobs that nothing references and whose age is beyond a grace period. A re-store that kept the old age
    /// would let an old, unreferenced blob whose content came back (a file reverted to a version from months ago) be collected
    /// between the `put` and the commit of the reference, and nothing in the schema would stop that commit. The grace period must
    /// exceed the refresh interval plus the longest wait for the reference.
    ///
    /// **Rules for a collector (retention) and for the code that references blobs.** A collector must test `touched_at < now() -
    /// grace` in the same statement that locks or deletes the row (`DELETE ... WHERE touched_at < ...`, or `SELECT ... FOR UPDATE SKIP
    /// LOCKED` with the same condition), never from an earlier read, so that a refresh that commits in between wins. A job that
    /// locks first (`FOR UPDATE [SKIP LOCKED]`) must delete in the same transaction while it still holds the lock; a lock that is
    /// released before the delete protects nothing. Keep the retention transaction short and set `lock_timeout` on it, because a
    /// `put` of an old blob that the transaction has locked waits behind it, up to the put's own timeout. A reference taken by an
    /// existence check without a `put` (the registry's `sync/record.rs`, `adoption/confirm.rs` and the guarded insert in
    /// `baselines.rs` check that the blob exists and do not store it) is not protected by the refresh: only a `blob_refs` row protects
    /// such a reference (its foreign key is RESTRICT), and `baselines.blob_hash`, `file_state.nfs_hash` and `pr_links.nfs_hash` have
    /// no foreign key (nor do the other hash columns of the registry's tables), so the retention's reference check must read all of
    /// those columns. Any other writer of `blobs` must refresh it the same way, and any code that references a blob without storing
    /// it must be covered by the retention's reference check.
    ///
    /// A read ([`BlobStore::get`], [`BlobStore::get_many`]) is not a store and never refreshes. A store that keeps no age (the
    /// in-memory fake) satisfies this trivially. The age is the store's own: the port has no accessor for it, so the conformance
    /// suite (`conformance::blob_store`, behind the `conformance` feature) cannot check it, and only a store's own tests do (the
    /// Postgres store's, over a real database).
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError>;

    /// `None` when the hash is unknown (never an error).
    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError>;

    /// The known blobs among `hs`, in the order first requested, each at most once. Unknown hashes are
    /// left out. More than [`MAX_GET_MANY`] hashes is [`BlobError::TooMany`].
    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError>;
}
