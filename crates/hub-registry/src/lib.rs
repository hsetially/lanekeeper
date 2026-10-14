//! Ingestion, indexes, baselines, adoption, drift, pickup states, findings, as-served verification, read API.
//!
//! Owned by prompt 05. Read `AGENTS.md` and `prompts/05*.md` before changing this crate.
//!
//! The registry turns what agents send (scan deltas, cluster reports) and what Git does into indexed, incrementally maintained
//! facts, and serves them. It is built task by task:
//!
//! - [`ingest`] (T1): the [`ports::ReportSink`] over Postgres. Source facts only: blobs, observations, the NFS columns of
//!   `file_state`, Merkle roots, Deployments, release hints and sync windows. Every state change commits with its audit event and
//!   its outbox events in one transaction (D80).
//! - [`index`] (T2): the settings index (one set of rows per unique structured blob, flagged values stored redacted), the Git side of
//!   drift (which swimlane paths a moved head changes, and the normalised hash of a Git file), and the effective tree hashes. Derived
//!   facts, idempotent, and off the acknowledgement path.
//!
//! Nothing here reads a clock or an environment variable directly: instants come from a [`clock::Clock`] and every limit is a
//! field of [`config::RegistryConfig`] with a tested default. Queries are runtime-checked `sqlx::query` with binds (plan A39);
//! no SQL is built from input. Errors carry no input text (S21), and logs carry ids, paths, hashes and counts, never content (S10).
#![forbid(unsafe_code)]
// Documenting `# Errors` on every small function adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod clock;
pub mod config;
pub mod db;
pub mod error;
pub mod index;
pub mod ingest;
pub mod metrics;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{ConfigError, RegistryConfig};
pub use error::RegistryError;
pub use index::{
    Affected, GitIndex, GitIndexDeps, IndexDeps, IndexJob, IndexReport, Indexed, LaneImpact, Offer, Row,
    SettingsIndexer, SettingsSweeper, SettingsWorker, StoredTree, SweepReport, TreeHashStore, TreeWrite,
    index_blob,
};
pub use ingest::{AgentIngest, IngestDeps, ToolWriteRecord, ToolWrites, fuzzing};
pub use metrics::RegistryMetrics;
