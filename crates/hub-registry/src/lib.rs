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
//! - [`pipeline`] (T3): the incremental pipeline. It subscribes to `FileObserved` and `GitHeadMoved`, recomputes only the (swimlane, path)
//!   pairs they affect with the engine, and writes drift, findings and the effective tree hashes with their events in one transaction for
//!   each swimlane; a reconcile finds what an event missed, and a nightly job recomputes everything and repairs any divergence.
//!
//! - [`adoption`] (T5): the adoption pass (`AdoptionService`): the branch ranking, the hints, the best-matching base commit, the secret scan,
//!   the per-file resolutions and the confirmation that sets the baselines and unlocks editing, and the marks of an intentional
//!   divergence. Every call requires an active Admin (an Operator for the marks). [`baselines`] is the rule for the raw version behind a
//!   baseline that the pipeline, a sync report and a confirmation share.
//!
//! - [`sync`] (T4, T8): what the hub does when a sync Job has put Git's content on NFS (`record_sync`): the full scan, C7 first, baselines at the
//!   commits the report names, and the deployment record. [`branches`] says which tenant branches are deployable. A sync window (D75)
//!   holds the swimlane's drift alerts while it is open; when it closes the same processing runs for every tenant of the swimlane with one
//!   scan, and the alerts are evaluated once ([`sync::alerts`], [`sync::window`]).
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

pub mod adoption;
pub mod baselines;
pub(crate) mod batch;
pub mod blob_policy;
pub mod branches;
pub mod clock;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod index;
pub mod ingest;
pub mod metrics;
pub mod pipeline;
pub mod read;
pub mod sync;

pub use adoption::{
    AdoptionDeps, AdoptionError, AdoptionReason, AdoptionService, AdoptionStatus, AdoptionStep, AdoptionView,
    Bulk, ConfirmOutcome, DifferingFile, DifferingFiles, Hint, HintSource, RankedBranch, Reason, Resolution,
    ResolutionKind, ResolutionView, SecretHit,
};
pub use blob_policy::RegistryBlobPolicy;
pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{ConfigError, RegistryConfig};
pub use error::RegistryError;
pub use http::{registry_route_specs, registry_routes};
pub use index::{
    Affected, GitIndex, GitIndexDeps, IndexDeps, IndexJob, IndexReport, Indexed, LaneImpact, Offer, Row,
    SettingsIndexer, SettingsSweeper, SettingsWorker, StoredTree, SweepReport, TreeHashStore, TreeWrite,
    index_blob,
};
pub use ingest::{AgentIngest, IngestDeps, ToolWriteRecord, ToolWrites, fuzzing};
pub use metrics::RegistryMetrics;
pub use pipeline::{
    BatchReport, LaneReport, NightlyReport, Pipeline, PipelineDeps, PipelineHandle, ReconcileOutcome, Work,
};
pub use read::warmup::WarmupTask;
pub use read::{Named, ReadDeps, RegistryApi, RegistryService, Tagged};
pub use sync::{
    Alerts, AlertsDeps, SyncDeps, SyncError, SyncOutcome, SyncService, SyncSource, WATCH_JOB, WatchReport,
};
