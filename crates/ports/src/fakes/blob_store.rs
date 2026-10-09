use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use domain::ContentHash;

use super::util::lock;
use crate::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};

/// Default capacity: enough for any test, small enough to notice a runaway loop.
const DEFAULT_CAPACITY_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
struct State {
    blobs: HashMap<ContentHash, Bytes>,
    total: usize,
}

/// Content-addressed: the key is the SHA-256 of the bytes, and storing twice stores once.
#[derive(Clone)]
pub struct FakeBlobStore {
    state: Arc<Mutex<State>>,
    capacity: usize,
}

impl std::fmt::Debug for FakeBlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBlobStore")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl FakeBlobStore {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY_BYTES)
    }

    /// A store that answers [`BlobError::Full`] once it holds `capacity_bytes`.
    pub fn with_capacity(capacity_bytes: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            capacity: capacity_bytes,
        }
    }

    /// How many distinct blobs are stored.
    pub fn len(&self) -> usize {
        lock(&self.state).blobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for FakeBlobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BlobStore for FakeBlobStore {
    async fn put(&self, bytes: Bytes) -> Result<ContentHash, BlobError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(BlobError::TooLarge);
        }
        let h = content_hash(&bytes);
        let mut st = lock(&self.state);
        if st.blobs.contains_key(&h) {
            return Ok(h);
        }
        if st.total + bytes.len() > self.capacity {
            return Err(BlobError::Full);
        }
        st.total += bytes.len();
        st.blobs.insert(h, bytes);
        Ok(h)
    }

    async fn get(&self, h: &ContentHash) -> Result<Option<Bytes>, BlobError> {
        Ok(lock(&self.state).blobs.get(h).cloned())
    }

    async fn get_many(&self, hs: &[ContentHash]) -> Result<Vec<(ContentHash, Bytes)>, BlobError> {
        if hs.len() > MAX_GET_MANY {
            return Err(BlobError::TooMany);
        }
        let st = lock(&self.state);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for h in hs {
            if seen.insert(*h) {
                if let Some(b) = st.blobs.get(h) {
                    out.push((*h, b.clone()));
                }
            }
        }
        Ok(out)
    }
}
