use std::sync::Arc;

use async_trait::async_trait;
use domain::{AuditId, DomainEvent};

use super::audit_log::AuditInner;
use super::event_bus::BusInner;
use crate::{AuditEvent, OwnedTx, Tx, TxError, TxFactory};

/// What a fake port parked in a transaction, to apply on commit.
pub(crate) enum Pending {
    Audit(Arc<AuditInner>, AuditId, Box<AuditEvent>),
    Publish(Arc<BusInner>, Box<DomainEvent>),
}

/// The in-memory stand-in for a database transaction: fakes park their writes here and they take effect
/// only on [`FakeTxState::commit`]. Dropping the state, or [`FakeTxState::rollback`], discards them.
#[derive(Default)]
pub struct FakeTxState {
    pending: Vec<Pending>,
}

impl std::fmt::Debug for FakeTxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeTxState")
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl FakeTxState {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, p: Pending) {
        self.pending.push(p);
    }

    /// Make everything parked in this transaction visible, in the order it was parked.
    pub fn commit(self) {
        for p in self.pending {
            match p {
                Pending::Audit(log, id, event) => log.append_committed(id, *event),
                Pending::Publish(bus, event) => bus.fan_out(*event),
            }
        }
    }

    pub fn rollback(self) {}
}

/// Starts fake transactions. Independent of any particular fake: parked writes carry their target.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeTxFactory;

impl FakeTxFactory {
    pub fn new() -> Self {
        Self
    }
}

/// A fake transaction that owns its state.
#[derive(Debug, Default)]
pub struct FakeOwnedTx {
    state: FakeTxState,
}

#[async_trait]
impl OwnedTx for FakeOwnedTx {
    fn tx(&mut self) -> Tx<'_> {
        Tx::in_memory(&mut self.state)
    }

    async fn commit(self: Box<Self>) -> Result<(), TxError> {
        self.state.commit();
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<(), TxError> {
        self.state.rollback();
        Ok(())
    }
}

#[async_trait]
impl TxFactory for FakeTxFactory {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError> {
        Ok(Box::new(FakeOwnedTx::default()))
    }
}
