use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use super::tx::Pending;
use super::util::lock;
use crate::conformance::ToolWritesProbe;
use crate::{ToolWriteEntry, ToolWriteError, ToolWrites, Tx};

/// Most registrations the fake holds, committed and parked in open transactions together; a full fake refuses more at
/// `register`, like a registry whose queue is full. A registration it accepted is never dropped at commit.
const MAX_ENTRIES: usize = 10_000;

#[derive(Default)]
struct Registry {
    /// Committed, oldest first.
    entries: Vec<ToolWriteEntry>,
    /// Registrations accepted in a transaction that has not ended: they hold a place until it commits or rolls back.
    reserved: usize,
}

pub(crate) struct ToolWritesInner {
    registry: Mutex<Registry>,
    max: usize,
}

fn same_registration(a: &ToolWriteEntry, b: &ToolWriteEntry) -> bool {
    // The same (swimlane, path, hash, request) twice is one registration.
    a.swimlane == b.swimlane && a.path == b.path && a.new_hash == b.new_hash && a.request_id == b.request_id
}

/// A place in the registry, held by an accepted registration until its transaction ends. Committing makes the entry
/// visible; dropping it (rollback, or a transaction that was just dropped) gives the place back.
pub(crate) struct Reservation {
    inner: Arc<ToolWritesInner>,
    entry: Option<ToolWriteEntry>,
}

impl Reservation {
    pub(crate) fn entry(&self) -> Option<&ToolWriteEntry> {
        self.entry.as_ref()
    }

    /// Commit cannot fail: the place was taken at `register`.
    pub(crate) fn commit(mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        let mut registry = lock(&self.inner.registry);
        registry.reserved = registry.reserved.saturating_sub(1);
        if !registry.entries.iter().any(|x| same_registration(x, &entry)) {
            registry.entries.push(entry);
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.entry.is_some() {
            let mut registry = lock(&self.inner.registry);
            registry.reserved = registry.reserved.saturating_sub(1);
        }
    }
}

/// Registrations parked in the caller's transaction and made visible on commit.
#[derive(Clone)]
pub struct FakeToolWrites {
    inner: Arc<ToolWritesInner>,
}

impl Default for FakeToolWrites {
    fn default() -> Self {
        Self::with_capacity(MAX_ENTRIES)
    }
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

    /// A fake that holds at most `max` registrations, committed and parked together.
    pub fn with_capacity(max: usize) -> Self {
        Self {
            inner: Arc::new(ToolWritesInner {
                registry: Mutex::new(Registry::default()),
                max,
            }),
        }
    }

    /// The committed registrations, oldest first.
    pub fn entries(&self) -> Vec<ToolWriteEntry> {
        lock(&self.inner.registry).entries.clone()
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
        let entry = ToolWriteEntry {
            swimlane: swimlane.clone(),
            path: path.clone(),
            new_hash: *new_hash,
            user: user.clone(),
            request_id: request_id.clone(),
        };
        // A repeat is a no-op, even when the registry is full: it asks for no new place. It may repeat a committed
        // registration or one parked earlier in this transaction.
        if state.parked_tool_writes().any(|x| same_registration(x, &entry)) {
            return Ok(());
        }
        let mut registry = lock(&self.inner.registry);
        if registry.entries.iter().any(|x| same_registration(x, &entry)) {
            return Ok(());
        }
        // Committed and parked together are bounded, so that a commit never has to drop what was accepted.
        if registry.entries.len() + registry.reserved >= self.inner.max {
            return Err(ToolWriteError::Full);
        }
        registry.reserved += 1;
        drop(registry);
        state.push(Pending::ToolWrite(Reservation {
            inner: self.inner.clone(),
            entry: Some(entry),
        }));
        Ok(())
    }
}

#[async_trait]
impl ToolWritesProbe for FakeToolWrites {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.entries()
    }
}
