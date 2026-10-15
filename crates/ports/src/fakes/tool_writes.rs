use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{ContentHash, NfsPath, RequestId, SwimlaneId, UserId};

use super::tx::Pending;
use super::util::lock;
use crate::conformance::ToolWritesProbe;
use crate::{ToolDeleteEntry, ToolWriteEntry, ToolWriteError, ToolWrites, Tx};

/// Most registrations the fake holds, hash and delete registrations together, committed and parked in open transactions
/// together; a full fake refuses more at `register` and `register_delete`, like a registry whose queue is full. A
/// registration it accepted is never dropped at commit.
const MAX_ENTRIES: usize = 10_000;

#[derive(Default)]
struct Registry {
    /// Committed hash registrations, oldest first.
    entries: Vec<ToolWriteEntry>,
    /// Committed delete registrations, oldest first.
    deletes: Vec<ToolDeleteEntry>,
    /// Registrations accepted in a transaction that has not ended: they hold a place until it commits or rolls back.
    reserved: usize,
}

impl Registry {
    /// Places taken: the committed registrations of both kinds, and the ones parked in open transactions.
    fn taken(&self) -> usize {
        self.entries.len() + self.deletes.len() + self.reserved
    }
}

pub(crate) struct ToolWritesInner {
    registry: Mutex<Registry>,
    max: usize,
}

/// The same (swimlane, path, hash, request) twice is one registration.
fn same_write(a: &ToolWriteEntry, b: &ToolWriteEntry) -> bool {
    a.swimlane == b.swimlane && a.path == b.path && a.new_hash == b.new_hash && a.request_id == b.request_id
}

/// The same (swimlane, path, deleted hash, request) twice is one delete registration: the user is not part of the key, and a
/// delete of another version of the file is another registration.
fn same_delete(a: &ToolDeleteEntry, b: &ToolDeleteEntry) -> bool {
    a.swimlane == b.swimlane
        && a.path == b.path
        && a.deleted_hash == b.deleted_hash
        && a.request_id == b.request_id
}

/// A registration of either kind. A write and a delete are never the same registration, even for one swimlane, path and
/// request: each is compared only with its own kind.
pub(crate) enum Registration {
    Write(ToolWriteEntry),
    Delete(ToolDeleteEntry),
}

impl Registration {
    /// Whether `other` is this registration again.
    pub(crate) fn repeated_by(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Write(a), Self::Write(b)) => same_write(a, b),
            (Self::Delete(a), Self::Delete(b)) => same_delete(a, b),
            _ => false,
        }
    }

    /// Whether the registry already holds this registration, committed.
    fn committed_in(&self, registry: &Registry) -> bool {
        match self {
            Self::Write(e) => registry.entries.iter().any(|x| same_write(x, e)),
            Self::Delete(e) => registry.deletes.iter().any(|x| same_delete(x, e)),
        }
    }
}

/// A place in the registry, held by an accepted registration until its transaction ends. Committing makes the entry
/// visible; dropping it (rollback, or a transaction that was just dropped) gives the place back.
pub(crate) struct Reservation {
    inner: Arc<ToolWritesInner>,
    registration: Registration,
    /// The place is still held: cleared by `commit`, which hands it over to the committed entry.
    holds_place: bool,
}

impl Reservation {
    pub(crate) fn registration(&self) -> &Registration {
        &self.registration
    }

    /// Commit cannot fail: the place was taken at `register` or `register_delete`.
    pub(crate) fn commit(mut self) {
        // The place becomes the committed entry's, so that dropping `self` below gives nothing back.
        self.holds_place = false;
        let mut registry = lock(&self.inner.registry);
        registry.reserved = registry.reserved.saturating_sub(1);
        if !self.registration.committed_in(&registry) {
            match &self.registration {
                Registration::Write(e) => registry.entries.push(e.clone()),
                Registration::Delete(e) => registry.deletes.push(e.clone()),
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.holds_place {
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

    /// A fake that holds at most `max` registrations, hash and delete registrations together, committed and parked together.
    pub fn with_capacity(max: usize) -> Self {
        Self {
            inner: Arc::new(ToolWritesInner {
                registry: Mutex::new(Registry::default()),
                max,
            }),
        }
    }

    /// The committed hash registrations, oldest first.
    pub fn entries(&self) -> Vec<ToolWriteEntry> {
        lock(&self.inner.registry).entries.clone()
    }

    /// The committed delete registrations, oldest first.
    pub fn delete_entries(&self) -> Vec<ToolDeleteEntry> {
        lock(&self.inner.registry).deletes.clone()
    }

    /// Park `registration` in `tx`, unless it is a repeat of one the registry already holds (committed, or parked earlier in
    /// this transaction), or the registry is full.
    fn park(&self, tx: &mut Tx<'_>, registration: Registration) -> Result<(), ToolWriteError> {
        let state = tx.mem().map_err(|_| ToolWriteError::WrongTx)?;
        // A repeat is a no-op, even when the registry is full: it asks for no new place. It may repeat a committed
        // registration or one parked earlier in this transaction.
        if state
            .parked_tool_registrations()
            .any(|x| x.repeated_by(&registration))
        {
            return Ok(());
        }
        let mut registry = lock(&self.inner.registry);
        if registration.committed_in(&registry) {
            return Ok(());
        }
        // Committed and parked together, of both kinds, are bounded, so that a commit never has to drop what was accepted.
        if registry.taken() >= self.inner.max {
            return Err(ToolWriteError::Full);
        }
        registry.reserved += 1;
        drop(registry);
        state.push(Pending::ToolWrite(Reservation {
            inner: self.inner.clone(),
            registration,
            holds_place: true,
        }));
        Ok(())
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
        self.park(
            tx,
            Registration::Write(ToolWriteEntry {
                swimlane: swimlane.clone(),
                path: path.clone(),
                new_hash: *new_hash,
                user: user.clone(),
                request_id: request_id.clone(),
            }),
        )
    }

    async fn register_delete(
        &self,
        tx: &mut Tx<'_>,
        swimlane: &SwimlaneId,
        path: &NfsPath,
        deleted_hash: &ContentHash,
        user: &UserId,
        request_id: &RequestId,
    ) -> Result<(), ToolWriteError> {
        self.park(
            tx,
            Registration::Delete(ToolDeleteEntry {
                swimlane: swimlane.clone(),
                path: path.clone(),
                deleted_hash: *deleted_hash,
                user: user.clone(),
                request_id: request_id.clone(),
            }),
        )
    }
}

#[async_trait]
impl ToolWritesProbe for FakeToolWrites {
    async fn registered(&self) -> Vec<ToolWriteEntry> {
        self.entries()
    }

    /// The fake registers deletes, so it opts in: the suite holds it to every delete rule.
    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        Some(self.delete_entries())
    }
}
