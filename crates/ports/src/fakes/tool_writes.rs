use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use super::tx::Pending;
use super::util::lock;
use crate::conformance::ToolWritesProbe;
use crate::{ToolWriteEntry, ToolWriteError, ToolWrites, Tx};

/// Most committed registrations the fake keeps; a full fake refuses more, like a registry whose queue is full.
const MAX_ENTRIES: usize = 10_000;

#[derive(Default)]
pub(crate) struct ToolWritesInner {
    entries: Mutex<Vec<ToolWriteEntry>>,
}

impl ToolWritesInner {
    pub(crate) fn append_committed(&self, e: ToolWriteEntry) {
        let mut entries = lock(&self.entries);
        // The same (swimlane, path, hash, request) twice is one registration.
        let same = |x: &ToolWriteEntry| {
            x.swimlane == e.swimlane
                && x.path == e.path
                && x.new_hash == e.new_hash
                && x.request_id == e.request_id
        };
        if !entries.iter().any(same) && entries.len() < MAX_ENTRIES {
            entries.push(e);
        }
    }
}

/// Registrations parked in the caller's transaction and made visible on commit.
#[derive(Clone, Default)]
pub struct FakeToolWrites {
    inner: Arc<ToolWritesInner>,
}

impl std::fmt::Debug for FakeToolWrites {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeToolWrites").finish_non_exhaustive()
    }
}

impl FakeToolWrites {
    pub fn new() -> Self {
        Self::default()
    }

    /// The committed registrations, oldest first.
    pub fn entries(&self) -> Vec<ToolWriteEntry> {
        lock(&self.inner.entries).clone()
    }
}

#[async_trait]
impl ToolWrites for FakeToolWrites {
    async fn register(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        new_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        let state = tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
        if lock(&self.inner.entries).len() >= MAX_ENTRIES {
            return Err(ToolWriteError::Full);
        }
        state.push(Pending::ToolWrite(
            self.inner.clone(),
            Box::new(ToolWriteEntry {
                swimlane: swimlane.clone(),
                path: path.clone(),
                new_hash: *new_hash,
                user: user.clone(),
                request_id: request_id.clone(),
            }),
        ));
        Ok(())
    }
}

#[async_trait]
impl ToolWritesProbe for FakeToolWrites {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.entries()
    }
}
