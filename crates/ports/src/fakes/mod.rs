//! In-memory fakes of every port (feature `fakes`).
//!
//! Other crates use these in their tests and never a sibling crate's real implementation. They are
//! deliberately realistic where behaviour matters to callers, and every one is bounded:
//!
//! - the gateway fake supports timeouts; the blob store is content-addressed and has a capacity;
//! - the event bus has bounded subscribers and emits `Resync` when one lags; leases expire;
//! - the audit fake keeps the real hash chain, and a rolled-back transaction leaves nothing behind;
//! - the write fake never writes blind, honours idempotency keys and role floors.
//!
//! They do not compute facts (diffs, drift, effective config): that is `crates/engine`'s job. Every fake
//! passes the matching suite in [`crate::conformance`].

mod agent_gateway;
mod audit_log;
mod blob_store;
mod doc_search;
mod event_bus;
mod git_reader;
mod kms;
mod leases;
mod notifier;
mod registry;
mod report_sink;
mod secret_source;
mod sentinel_sink;
mod token_verifier;
mod tx;
mod users;
mod util;
mod write_service;

pub use agent_gateway::FakeAgentGateway;
pub use audit_log::FakeAuditLog;
pub use blob_store::FakeBlobStore;
pub use doc_search::FakeDocSearch;
pub use event_bus::FakeEventBus;
pub use git_reader::FakeGit;
pub use kms::{FakeKmsEnvelope, FakeKmsSigner};
pub use leases::FakeLeases;
pub use notifier::FakeNotifier;
pub use registry::FakeRegistry;
pub use report_sink::FakeReportSink;
pub use secret_source::FakeSecretSource;
pub use sentinel_sink::FakeSentinelSink;
pub use token_verifier::FakeTokenVerifier;
pub use tx::{FakeOwnedTx, FakeTxFactory, FakeTxState};
pub use users::FakeUsers;
pub use write_service::FakeWriteService;
