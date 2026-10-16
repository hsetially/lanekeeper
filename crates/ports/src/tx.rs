//! `Tx`: an opaque handle to the caller's open transaction (plan 01, Q3).
//!
//! `AuditLog::record` and `EventBus::publish_in_tx` take `&mut Tx` so that the state change, its audit
//! event and its outbox event commit or roll back together (D80). Real implementations build a `Tx` from a
//! `sqlx` connection; fakes build one over an in-memory state that needs no database.
//!
//! `domain` stays free of `sqlx`: only this crate, behind the `postgres` feature, names it.

use std::fmt;
use std::marker::PhantomData;

use async_trait::async_trait;

#[cfg(feature = "fakes")]
use crate::fakes::FakeTxState;

/// A transaction in progress. It borrows the underlying connection, so it cannot outlive it.
///
/// The handle is opaque on purpose: a caller hands it to a port and can see nothing inside it, so it cannot know what a port
/// runs, nor rely on that failing cleanly. A port whose failure must not abort the caller's transaction therefore opens a
/// savepoint of its own: that is the contract of `ToolWrites::register` and `register_delete` (see the docs of
/// [`crate::ToolWrites`]). A caller that holds the connection can also wrap the call in a savepoint of its own, as defence in
/// depth (raw SQL, as the docs of [`Tx::pg`] say).
pub struct Tx<'a> {
    inner: Inner<'a>,
}

enum Inner<'a> {
    #[cfg(feature = "postgres")]
    Pg(&'a mut sqlx::PgConnection),
    #[cfg(feature = "fakes")]
    Mem(&'a mut FakeTxState),
    /// Built only by `Tx::detached`, with feature `conformance`, for the tool-write conformance suites; never built otherwise,
    /// and then it only keeps `'a` in use when no backend feature is enabled.
    #[allow(dead_code)]
    Detached(PhantomData<&'a mut ()>),
}

/// The transaction is not of the kind the caller asked for (for example a fake `Tx` given to a Postgres
/// implementation). Always a wiring bug, never user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("transaction is not backed by the expected store")]
pub struct WrongTxKind;

// `'a` is named for the constructors behind the `postgres` and `fakes` features.
#[allow(clippy::elidable_lifetime_names)]
impl<'a> Tx<'a> {
    /// Wrap a Postgres connection that is inside a `BEGIN`. Use `Tx::from_pg(&mut *sqlx_transaction)`.
    #[cfg(feature = "postgres")]
    pub fn from_pg(conn: &'a mut sqlx::PgConnection) -> Self {
        Self {
            inner: Inner::Pg(conn),
        }
    }

    /// Wrap the in-memory transaction state of the fakes.
    #[cfg(feature = "fakes")]
    pub fn in_memory(state: &'a mut FakeTxState) -> Self {
        Self {
            inner: Inner::Mem(state),
        }
    }

    /// A transaction backed by nothing: neither a Postgres connection nor an in-memory state, so `pg` and `mem` refuse it, and
    /// an implementation that checks its transaction answers `WrongTx`, whatever store it is built on. Only the tool-write
    /// conformance suites (`tool_writes`, `tool_writes_with_deletes`, which call `register` and `register_delete` with it)
    /// build one and exercise that; no other port's suite does. Not public, so nothing else can make one.
    #[cfg(feature = "conformance")]
    pub(crate) fn detached() -> Self {
        Self {
            inner: Inner::Detached(PhantomData),
        }
    }

    /// The Postgres connection, for real implementations.
    ///
    /// In Postgres a failed statement aborts the whole transaction (SQLSTATE `25P02` for every later statement, the caller's
    /// audit insert included). An implementation that must leave the caller's transaction usable when its statements fail
    /// (`ToolWrites`) opens a savepoint of its own on this connection, runs its statements in it, and rolls back to it before
    /// it returns an error. That is the implementation's contract; a caller may also wrap the call in a savepoint of its own.
    ///
    /// - Open the savepoint with raw `SAVEPOINT name`, `RELEASE SAVEPOINT name` and `ROLLBACK TO SAVEPOINT name` statements on
    ///   this connection (the registry's implementation does). Do not use a drop-guard based nested transaction for it, such as
    ///   `sqlx::Connection::begin`: in sqlx 0.8.6 a nested transaction whose guard is dropped half-way (a cancelled future)
    ///   rolls back the caller's WHOLE transaction, and after a raw `BEGIN` it issues a second `BEGIN`. A cancelled future
    ///   must not roll back the caller's transaction.
    /// - A `SET LOCAL` (a `lock_timeout`, say) made inside the savepoint is reverted by a rollback to it but persists after
    ///   `RELEASE`, into the caller's later statements. A registration needs a bounded wait, so the implementation restores
    ///   the previous value after it releases the savepoint, or uses another bound.
    /// - A lost connection is not covered: the transaction is gone with the connection.
    #[cfg(feature = "postgres")]
    pub fn pg(&mut self) -> Result<&mut sqlx::PgConnection, WrongTxKind> {
        if let Inner::Pg(conn) = &mut self.inner {
            Ok(&mut **conn)
        } else {
            Err(WrongTxKind)
        }
    }

    /// The in-memory state, for fakes.
    #[cfg(feature = "fakes")]
    pub fn mem(&mut self) -> Result<&mut FakeTxState, WrongTxKind> {
        if let Inner::Mem(state) = &mut self.inner {
            Ok(&mut **state)
        } else {
            Err(WrongTxKind)
        }
    }
}

impl fmt::Debug for Tx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.inner {
            #[cfg(feature = "postgres")]
            Inner::Pg(_) => "postgres",
            #[cfg(feature = "fakes")]
            Inner::Mem(_) => "in_memory",
            Inner::Detached(_) => "detached",
        };
        f.debug_struct("Tx").field("kind", &kind).finish()
    }
}

/// Why a transaction could not be started or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TxError {
    #[error("transaction store is unavailable")]
    Unavailable,
    #[error("transaction could not be committed")]
    CommitFailed,
}

/// A transaction that owns its connection, for tests and the conformance suites.
///
/// Production code gets a `Tx` from `sqlx` directly; this trait exists so that the audit and outbox
/// conformance suites can start, commit and roll back a transaction against any implementation.
#[async_trait]
pub trait OwnedTx: Send {
    /// Borrow the transaction to hand to a port call.
    fn tx(&mut self) -> Tx<'_>;
    async fn commit(self: Box<Self>) -> Result<(), TxError>;
    async fn rollback(self: Box<Self>) -> Result<(), TxError>;
}

/// Starts transactions. Implemented next to each real `AuditLog` and `EventBus`, and by the fakes.
#[async_trait]
pub trait TxFactory: Send + Sync + 'static {
    async fn begin(&self) -> Result<Box<dyn OwnedTx>, TxError>;
}
