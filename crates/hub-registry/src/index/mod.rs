//! Indexes (T2, D65): what is computed once and read many times.
//!
//! | Index | Where | Written by |
//! |---|---|---|
//! | settings index, one set of rows per unique structured blob | `settings` | [`SettingsIndexer`] (offered by ingestion, off the acknowledgement path) and the leased [`SettingsSweeper`] |
//! | Git side of drift: which swimlane paths a moved head changes, and the normalised hash of a Git file, cached by object id | `git` | [`GitIndex`] reads only; the `git_tree_index` table has one writer, hub-platform's `TreeIndexPersist` (plan A22) |
//! | effective tree hashes, one per directory, and the effective hash of each logical file | `tree_hashes` | [`TreeHashStore`], called by the pipeline (T3) with the engine's [`engine::LaneState`] |
//!
//! Every write here is idempotent, so a replay of the same deltas leaves the same tables (the T2 verify).

pub mod git;
pub mod settings;
mod sweeper;
pub mod tree_hashes;

pub use git::{Affected, GitIndex, GitIndexDeps, LaneImpact};
pub use settings::{
    IndexDeps, IndexJob, IndexReport, Indexed, Offer, Row, SettingsIndexer, SettingsWorker, index_blob,
};
pub use sweeper::{SettingsSweeper, SweepReport};
pub use tree_hashes::{StoredTree, TreeHashStore, TreeWrite};
