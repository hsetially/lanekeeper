//! A delta sink that keeps what it is given.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use agent::tree::delta::{DeltaSink, SinkError};
use async_trait::async_trait;
use domain::{ContentHash, ScanDelta};

#[derive(Debug, Default)]
pub struct CollectSink {
    deltas: Mutex<Vec<ScanDelta>>,
    fail_after: Mutex<Option<usize>>,
}

impl CollectSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept this many messages, then refuse the rest as a closed connection would.
    pub fn fail_after(&self, messages: usize) {
        *self.fail_after.lock().unwrap() = Some(messages);
    }

    pub fn deltas(&self) -> Vec<ScanDelta> {
        self.deltas.lock().unwrap().clone()
    }

    pub fn take(&self) -> Vec<ScanDelta> {
        std::mem::take(&mut *self.deltas.lock().unwrap())
    }

    /// Apply the logical delta to a mirror of path -> bytes, as the hub does.
    pub fn apply_to(mirror: &mut std::collections::BTreeMap<String, Vec<u8>>, deltas: &[ScanDelta]) {
        for d in deltas {
            for p in &d.removed {
                mirror.remove(p.as_str());
            }
            for e in &d.entries {
                if let Some(bytes) = &e.bytes {
                    mirror.insert(e.path.as_str().to_owned(), bytes.to_vec());
                }
            }
        }
    }

    pub fn roots(deltas: &[ScanDelta]) -> Vec<(Option<ContentHash>, ContentHash)> {
        deltas.iter().map(|d| (d.base_root, d.new_root)).collect()
    }
}

#[async_trait]
impl DeltaSink for CollectSink {
    async fn deliver(&self, delta: ScanDelta) -> Result<(), SinkError> {
        let mut deltas = self.deltas.lock().unwrap();
        if let Some(limit) = *self.fail_after.lock().unwrap() {
            if deltas.len() >= limit {
                return Err(SinkError::Closed);
            }
        }
        deltas.push(delta);
        Ok(())
    }
}
