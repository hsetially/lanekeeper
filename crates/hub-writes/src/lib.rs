//! Edits, uploads, deletes, reverts, notify, restarts, PRs, approvals, token vault, Teams notifier.
//!
//! Owned by prompt 06. Read `AGENTS.md` and `prompts/06*.md` before changing this crate.
//!
//! The crate's heart is [`WriteHub`], the [`ports::WriteService`] implementation, and the [`pipeline`] its actions go through:
//! authorize, claim the idempotency key, check the editing lock, validate, route to approval, execute. Nothing is written
//! blind (rule 11): the agent is sent the hash it must find. Every applied write has exactly one audit event, recorded in the
//! same transaction as its tool-write registration, its outbox event and its stored response (rules 8 and D80).
#![forbid(unsafe_code)]
// Documenting `# Errors` on every internal function adds noise; the error types are the documentation.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod clock;
mod config;
mod db;
mod error;
mod fidelity;
mod idempotency;
mod ops;
mod pipeline;
mod probe;
mod proposals;
mod reconcile;
mod service;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{MAX_EXTRA_DENY_GLOBS, WriteConfig};
pub use error::ConfigError;
pub use idempotency::RequestHash;
pub use probe::QueryProbe;
pub use reconcile::{ReconcileReport, ReconcileWrites};
pub use service::{WriteHub, WriteHubDeps};

/// The statements the write path runs on every request, by name, for the tests that run `EXPLAIN` on them (P12). Each takes
/// numbered parameters, so `EXPLAIN (GENERIC_PLAN)` shows its plan for any value.
#[doc(hidden)]
pub fn hot_queries() -> Vec<(&'static str, &'static str)> {
    let mut all = idempotency::queries();
    all.push(("swimlane_lock", pipeline::lock::SWIMLANE_LOCK));
    all.push(("proposal_insert", proposals::store::INSERT_SQL));
    all
}
