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
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError>;

    /// `None` when the hash is unknown (never an error).
    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError>;

    /// The known blobs among `hs`, in the order first requested, each at most once. Unknown hashes are
    /// left out. More than [`MAX_GET_MANY`] hashes is [`BlobError::TooMany`].
    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError>;
}
