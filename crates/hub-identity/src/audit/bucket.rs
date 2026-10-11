//! The bucket that holds a second copy of every checkpoint (S9).
//!
//! The production bucket has a locked retention policy and object versioning, so an object cannot be changed or removed before
//! its retention ends, not even by the hub's own service account. The hub may only *create* objects: [`CheckpointBucket::put`]
//! never overwrites. A database owner who rewrites history cannot also rewrite the bucket, so the nightly verification
//! catches the difference.
//!
//! - [`GcsCheckpointBucket`] talks to the Cloud Storage JSON API through the crate's [`HttpsClient`] (TLS 1.3, host allowlist
//!   that must include `storage.googleapis.com`, a timeout and a size cap on every call). Create-only is the server's rule:
//!   `ifGenerationMatch=0` makes an upload of an existing name fail with `412`.
//! - [`MemoryBucket`] is the same contract in memory, for tests and local development.
//! - [`AccessTokenSource`] gives the bucket an `OAuth2` access token. Production uses [`super::metadata::MetadataTokenSource`]
//!   (Workload Identity through the GKE metadata server); the token is a [`Secret`] and never appears in an error or a log line.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use url::Url;

use crate::http::{HttpsClient, HttpsRequest};

/// The prefix of every checkpoint object.
pub const OBJECT_PREFIX: &str = "audit/checkpoints/";

/// The largest object written or read. A checkpoint is a few hundred bytes.
pub const MAX_OBJECT_BYTES: usize = 16 * 1024;

/// The most objects the in-memory bucket holds; further writes fail.
const MEMORY_BUCKET_MAX_OBJECTS: usize = 100_000;

/// Why a bucket call failed. Carries no object name, URL, token or body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BucketError {
    #[error("checkpoint bucket is unavailable")]
    Unavailable,
    /// The bucket refused the credentials or the request (`401`, `403`, `400`).
    #[error("checkpoint bucket rejected the request")]
    Rejected,
    #[error("checkpoint object is too large")]
    TooLarge,
    /// The object or bucket name is not one this crate writes.
    #[error("checkpoint object name is not valid")]
    InvalidName,
}

/// The result of a create-only upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Created,
    /// An object with that name exists. It was not touched.
    AlreadyExists,
}

/// `audit/checkpoints/<seq>.json`.
pub fn object_name(seq: u64) -> String {
    format!("{OBJECT_PREFIX}{seq}.json")
}

/// True for the names [`object_name`] makes: the prefix, a `u64` in decimal without a leading zero, `.json`.
pub fn valid_object_name(name: &str) -> bool {
    name.strip_prefix(OBJECT_PREFIX)
        .and_then(|rest| rest.strip_suffix(".json"))
        .is_some_and(|digits| {
            digits.bytes().all(|b| b.is_ascii_digit())
                && (digits == "0" || !digits.starts_with('0'))
                && digits.parse::<u64>().is_ok()
        })
}

#[async_trait]
pub trait CheckpointBucket: Send + Sync + 'static {
    /// Create `name` with `bytes`, or report that it already exists. Never replaces an object.
    async fn put(&self, name: &str, bytes: Bytes) -> Result<PutOutcome, BucketError>;
    /// The object, or `None` when there is none.
    async fn get(&self, name: &str) -> Result<Option<Bytes>, BucketError>;
}

/// A checkpoint bucket in memory.
#[derive(Debug, Default)]
pub struct MemoryBucket {
    objects: Mutex<BTreeMap<String, Bytes>>,
}

impl MemoryBucket {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many objects it holds.
    pub fn len(&self) -> usize {
        self.objects.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The names it holds, in order.
    pub fn names(&self) -> Vec<String> {
        self.objects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

#[async_trait]
impl CheckpointBucket for MemoryBucket {
    async fn put(&self, name: &str, bytes: Bytes) -> Result<PutOutcome, BucketError> {
        if !valid_object_name(name) {
            return Err(BucketError::InvalidName);
        }
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(BucketError::TooLarge);
        }
        let mut objects = self.objects.lock().unwrap_or_else(PoisonError::into_inner);
        if objects.contains_key(name) {
            return Ok(PutOutcome::AlreadyExists);
        }
        if objects.len() >= MEMORY_BUCKET_MAX_OBJECTS {
            return Err(BucketError::Unavailable);
        }
        objects.insert(name.to_owned(), bytes);
        Ok(PutOutcome::Created)
    }

    async fn get(&self, name: &str) -> Result<Option<Bytes>, BucketError> {
        if !valid_object_name(name) {
            return Err(BucketError::InvalidName);
        }
        Ok(self
            .objects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned())
    }
}

/// An OAuth2 access token for the bucket's service account.
#[async_trait]
pub trait AccessTokenSource: Send + Sync + 'static {
    async fn token(&self) -> Result<Secret<String>, BucketError>;
}

/// Cloud Storage over the JSON API.
pub struct GcsCheckpointBucket {
    https: Arc<dyn HttpsClient>,
    tokens: Arc<dyn AccessTokenSource>,
    bucket: String,
}

impl std::fmt::Debug for GcsCheckpointBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsCheckpointBucket")
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

/// Bucket names: 3 to 63 characters of lower-case letters, digits, `-`, `_` and `.`, starting and ending with a letter or
/// digit.
fn valid_bucket_name(name: &str) -> bool {
    let b = name.as_bytes();
    (3..=63).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'.'))
        && b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
}

impl GcsCheckpointBucket {
    pub const HOST: &'static str = "storage.googleapis.com";

    /// `https` must allow [`GcsCheckpointBucket::HOST`].
    pub fn new(
        https: Arc<dyn HttpsClient>,
        tokens: Arc<dyn AccessTokenSource>,
        bucket: &str,
    ) -> Result<Self, BucketError> {
        if !valid_bucket_name(bucket) {
            return Err(BucketError::InvalidName);
        }
        Ok(Self {
            https,
            tokens,
            bucket: bucket.to_owned(),
        })
    }

    async fn authorize(&self, request: HttpsRequest) -> Result<HttpsRequest, BucketError> {
        let token = self.tokens.token().await?;
        request
            .with_header("authorization", &format!("Bearer {}", token.expose()))
            .map_err(|_| BucketError::Rejected)
    }
}

fn status_error(status: u16) -> BucketError {
    match status {
        400 | 401 | 403 => BucketError::Rejected,
        _ => BucketError::Unavailable,
    }
}

#[async_trait]
impl CheckpointBucket for GcsCheckpointBucket {
    async fn put(&self, name: &str, bytes: Bytes) -> Result<PutOutcome, BucketError> {
        if !valid_object_name(name) {
            return Err(BucketError::InvalidName);
        }
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(BucketError::TooLarge);
        }
        let mut url = Url::parse(&format!(
            "https://{}/upload/storage/v1/b/{}/o",
            Self::HOST,
            self.bucket
        ))
        .map_err(|_| BucketError::InvalidName)?;
        url.query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", name)
            // The precondition that makes the upload create-only: the object must not exist.
            .append_pair("ifGenerationMatch", "0");
        let request = self
            .authorize(HttpsRequest::post(url, "application/json", bytes))
            .await?;
        let response = self.https.send(request).await.map_err(|e| {
            tracing::warn!(error = %e, "checkpoint upload failed");
            BucketError::Unavailable
        })?;
        match response.status {
            200 | 201 => Ok(PutOutcome::Created),
            412 => Ok(PutOutcome::AlreadyExists),
            other => {
                tracing::warn!(status = other, "checkpoint upload refused");
                Err(status_error(other))
            }
        }
    }

    async fn get(&self, name: &str) -> Result<Option<Bytes>, BucketError> {
        if !valid_object_name(name) {
            return Err(BucketError::InvalidName);
        }
        let mut url = Url::parse(&format!("https://{}/storage/v1/b/{}/o/", Self::HOST, self.bucket))
            .map_err(|_| BucketError::InvalidName)?;
        // `push` encodes the slashes of the object name, as the API wants.
        url.path_segments_mut()
            .map_err(|()| BucketError::InvalidName)?
            .pop_if_empty()
            .push(name);
        url.query_pairs_mut().append_pair("alt", "media");
        let request = self.authorize(HttpsRequest::get(url)).await?;
        let response = self.https.send(request).await.map_err(|e| {
            tracing::warn!(error = %e, "checkpoint download failed");
            BucketError::Unavailable
        })?;
        match response.status {
            200 if response.body.len() > MAX_OBJECT_BYTES => Err(BucketError::TooLarge),
            200 => Ok(Some(response.body)),
            404 => Ok(None),
            other => {
                tracing::warn!(status = other, "checkpoint download refused");
                Err(status_error(other))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_names_round_trip_and_junk_is_refused() {
        assert_eq!(object_name(42), "audit/checkpoints/42.json");
        assert!(valid_object_name(&object_name(0)));
        assert!(valid_object_name(&object_name(u64::MAX)));
        for bad in [
            "",
            "audit/checkpoints/.json",
            "audit/checkpoints/042.json",
            "audit/checkpoints/4 2.json",
            "audit/checkpoints/../x.json",
            "audit/checkpoints/1.json/",
            "other/1.json",
            "audit/checkpoints/99999999999999999999.json",
            "audit/checkpoints/-1.json",
        ] {
            assert!(!valid_object_name(bad), "{bad}");
        }
    }

    #[test]
    fn bucket_names_follow_the_cloud_storage_rules() {
        assert!(valid_bucket_name("lk-audit-checkpoints"));
        assert!(valid_bucket_name("a.b_c-d"));
        for bad in ["", "ab", "-abc", "abc-", "ABC", "a b c", "a/b", &"a".repeat(64)] {
            assert!(!valid_bucket_name(bad), "{bad}");
        }
    }
}
