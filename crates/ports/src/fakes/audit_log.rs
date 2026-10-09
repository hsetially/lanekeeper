use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::AuditId;

use super::tx::Pending;
use super::util::lock;
use crate::conformance::AuditProbe;
use crate::{AuditEntry, AuditError, AuditEvent, AuditLog, Tx, canonical_json, chain_hash, genesis_hash};

/// Most committed entries the fake keeps.
const MAX_ENTRIES: usize = 100_000;

#[derive(Default)]
struct State {
    next_id: i64,
    entries: Vec<AuditEntry>,
}

#[derive(Default)]
pub(crate) struct AuditInner {
    state: Mutex<State>,
}

impl AuditInner {
    pub(crate) fn append_committed(&self, id: AuditId, event: AuditEvent) {
        let mut st = lock(&self.state);
        let prev = st.entries.last().map_or_else(genesis_hash, |e| e.hash);
        // Canonicalisation of plain data does not fail; if it ever did, the entry is dropped rather than
        // chained over a wrong hash.
        let Ok(json) = canonical_json(&event) else { return };
        let hash = chain_hash(&prev, &json);
        st.entries.push(AuditEntry {
            id,
            prev_hash: prev,
            hash,
            event,
        });
    }
}

/// The real hash chain over an in-memory vector. An entry exists only once its transaction commits.
#[derive(Clone, Default)]
pub struct FakeAuditLog {
    inner: Arc<AuditInner>,
}

impl std::fmt::Debug for FakeAuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeAuditLog").finish_non_exhaustive()
    }
}

impl FakeAuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The committed chain, from the first entry.
    pub fn entries(&self) -> Vec<AuditEntry> {
        lock(&self.inner.state).entries.clone()
    }
}

#[async_trait]
impl AuditLog for FakeAuditLog {
    async fn record(&self, tx: &mut Tx<'_>, e: AuditEvent) -> Result<AuditId, AuditError> {
        if e.diff
            .as_ref()
            .is_some_and(|d| d.len() > AuditEvent::MAX_DIFF_BYTES)
        {
            return Err(AuditError::TooLarge);
        }
        let state = tx.mem().map_err(|_| AuditError::WrongTx)?;
        let id = {
            let mut st = lock(&self.inner.state);
            if st.entries.len() >= MAX_ENTRIES {
                return Err(AuditError::Unavailable);
            }
            st.next_id += 1;
            AuditId::from(st.next_id)
        };
        state.push(Pending::Audit(self.inner.clone(), id, Box::new(e)));
        Ok(id)
    }
}

#[async_trait]
impl AuditProbe for FakeAuditLog {
    async fn committed(&self) -> Vec<AuditEntry> {
        self.entries()
    }
}
