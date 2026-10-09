//! Port traits between components, with in-memory fakes behind the "fakes" feature and conformance tests.
//!
//! Owned by prompt 01. Read `AGENTS.md` and `prompts/01*.md` before changing this crate.
//! This crate is a contract path: changes go through the `lanekeeper-contract-change` process.
//!
//! - Every trait in `docs/interfaces.md` lives here, with a per-port error enum that carries no input text.
//!   The signatures in that document are the shape; the details settled by prompt 01 are in rustdoc.
//! - Feature `fakes`: an in-memory fake of every port in [`fakes`], bounded and with realistic behaviour.
//!   Other crates use these in their tests and never a sibling crate's real implementation.
//! - Feature `conformance` (implied by `fakes`): `conformance::<port>(impl)` contract tests that every
//!   implementation runs, and the route-table harness for S4.
//! - Feature `postgres`: `Tx::from_pg`, for the real implementations.
#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod audit;
mod auth;
mod blob;
mod docs;
mod events;
mod gateway;
mod git;
mod identity;
mod kms;
mod lease;
mod notify;
mod read;
mod sink;
mod tx;
mod write;

#[cfg(feature = "conformance")]
pub mod conformance;
#[cfg(feature = "fakes")]
pub mod fakes;

pub use audit::{
    AuditAction, AuditActor, AuditEntry, AuditError, AuditEvent, AuditLog, AuditVia, canonical_json,
    chain_hash, genesis_hash, verify_chain,
};
pub use auth::{AuthError, MAX_TOKEN_BYTES, TokenVerifier, UserError, Users};
pub use blob::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};
pub use docs::{DocError, DocSearch, MAX_DOC_HITS, MAX_DOC_PATTERN_BYTES, MAX_GREP_CONTEXT};
pub use events::{BusError, EventBus, EventFilter, EventKind};
pub use gateway::{AgentGateway, GatewayError};
pub use git::{GitError, GitReader};
pub use identity::{AckSeq, AgentIdentity, GoogleIdentity, SentinelIdentity, VerifiedUser};
pub use kms::{
    KeyRef, KeyRefError, KmsEnvelope, KmsError, KmsSigner, SecretError, SecretSource, Signature, WrappedKey,
    is_valid_secret_name,
};
pub use lease::{
    LeaseBackend, LeaseError, LeaseGuard, Leases, MAX_LEASE_TTL, check_ttl, is_valid_lease_name,
};
pub use notify::{Notification, NotificationKind, Notifier, NotifyError};
pub use read::{ReadError, RegistryRead};
pub use sink::{ReportSink, SentinelSink, SinkError};
pub use tx::{OwnedTx, Tx, TxError, TxFactory, WrongTxKind};
pub use write::{WriteError, WriteService};

/// A boxed, pinned stream, as returned by [`EventBus::subscribe`].
pub use futures::stream::BoxStream;
