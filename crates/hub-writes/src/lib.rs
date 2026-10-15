//! Edits, uploads, deletes, reverts, notify, restarts, PRs, approvals, token vault, Teams notifier.
//!
//! Owned by prompt 06. Read `AGENTS.md` and `prompts/06*.md` before changing this crate.
//!
//! The crate's heart is [`WriteHub`], the [`ports::WriteService`] implementation, and the [`pipeline`] its actions go through:
//! authorize, claim the idempotency key, check the editing lock, validate, route to approval, execute. Nothing is written
//! blind (rule 11): the agent is sent the hash it must find. Every applied write has exactly one audit event, recorded in the
//! same transaction as its tool-write registration, its outbox event and its stored response (rules 8 and D80).
//!
//! - **Actions** (`ops`): `edit`, `upload`, `delete`, `revert`, `restart` and `notify`. The three that put content into a file (edit, upload
//!   and revert) share one tail (`ops::file`); an upload and a delete also need the token of a preview.
//! - **Approvals** (`proposals`): a change by a user who `requires_approval` waits as a proposal; an admin (never its author)
//!   approves it, which re-checks the file and applies it, or marks it stale; a leased job expires the ones nobody decided. Every
//!   transition publishes `ProposalChanged`. **Drafts** (`drafts`) are the same change prepared with no effect.
//! - **Previews** (`preview`): what an upload or a delete would do, from the engine, and the HMAC token bound to the user, the
//!   swimlane, the path, the action and the hashes that lets it through (D46).
//! - **Pull requests** ([`prs`]): the NFS paths reverse-mapped to the repository, the files put into the form of the repository's
//!   own, and the commit and the pull request made through the Git Data API with the raising user's own token (D41, D43). A request
//!   is accepted as a persisted, resumable **job** (`pending`, `preparing`, `committing`, `opening`, `completed` or `failed`) that
//!   [`WriteHub::pr_jobs`] hands out and a leased scan takes up after a crash; each pull request is **linked** to the swimlane, path
//!   and NFS hash it came from, and a second one for the same change is refused with the first one's link (D77).
//! - **Teams** ([`teams`]): [`teams::TeamsNotifier`], the [`ports::Notifier`] that posts an Adaptive Card to a Workflows webhook
//!   (D49, Q9). `notify` only queues; a worker sends with bounded retries and a breaker, and a failure never reaches the action.
//! - **Reveal** ([`RevealDesk`]): the one audited way to see a value the secret scan flags (D79, S14b). Editor and above, only
//!   through the web UI, rate limited for each user, one audit event committed before the value is returned, and the value is never
//!   stored, cached or logged. What can be revealed is exactly what every view masks. Every door of the write path also refuses a
//!   denied path (the built-in globs and the configured ones, and a file the registry withholds).
//! - **Notify** (`ops::notify`, `autonotify`, T11): tells the config-server's unauthenticated update endpoint (Q37) which files changed,
//!   through the agent: Operator only, exactly the paths of the request, one audit event, a proposal for a user who needs approval.
//!   A notification rewrites the pickup states of the services that read its paths (`ops::pickup`, the engine decides). With
//!   auto-notify on for a swimlane, [`WriteHub::auto_notifier`] batches the paths of an Operator's writes into one call per two
//!   seconds. [`WriteHub::config_server_restart_offer`] names the restart that check C11 asks for and the services it takes away.
//! - **Routes** ([`routes`]): the REST face of the actions above, registered through the platform's guarded router (S4), with
//!   [`routes::wire`] reading each request into typed values.
#![forbid(unsafe_code)]
// Documenting `# Errors` on every internal function adds noise; the error types are the documentation.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

mod assess;
mod autonotify;
mod clock;
mod config;
mod db;
mod drafts;
mod error;
mod fidelity;
pub mod github;
mod hex;
mod idempotency;
mod ops;
mod pipeline;
mod preview;
#[cfg(feature = "bench-probe")]
mod probe;
mod proposals;
pub mod prs;
mod reconcile;
mod reveal;
pub mod routes;
mod service;
pub mod teams;
pub mod vault;

pub use autonotify::AutoNotifier;
pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{MAX_EXTRA_DENY_GLOBS, PREVIEW_KEY_SECRET, WriteConfig};
pub use error::ConfigError;
pub use idempotency::RequestHash;
#[cfg(feature = "bench-probe")]
pub use probe::QueryProbe;
pub use proposals::{
    ExpireProposals, ExpireReport, NO_HASH, ProposalDesk, ProposalQuery, ProposalView, RejectResult,
};

/// Hooks for the fuzz targets (S22). Not part of the API; only with the `fuzz-hooks` feature.
#[cfg(feature = "fuzz-hooks")]
#[doc(hidden)]
pub use preview::token::fuzzing as fuzz_hooks;
pub use reconcile::{ReconcileReport, ReconcileWrites};
pub use reveal::{DEFAULT_REASON, RevealDesk, RevealError, RevealRequest, Revealed};
pub use service::{ConfigServerRestartOffer, WriteHub, WriteHubDeps};

/// The statements the write path runs on every request, by name, for the tests that run `EXPLAIN` on them (P12). Each takes
/// numbered parameters, so `EXPLAIN (GENERIC_PLAN)` shows its plan for any value.
#[doc(hidden)]
pub fn hot_queries() -> Vec<(&'static str, &'static str)> {
    let mut all = idempotency::queries();
    all.push(("swimlane_lock", pipeline::lock::SWIMLANE_LOCK));
    all.push(("proposal_insert", proposals::store::INSERT_SQL));
    all.push(("proposal_get", proposals::store::GET_SQL));
    all.push(("proposal_list", proposals::store::LIST_SQL));
    all.push(("proposal_lock", proposals::store::LOCK_SQL));
    all.push(("proposal_blob_sizes", proposals::list::SIZES_SQL));
    all.push(("proposal_expiry", proposals::store::EXPIRED_SQL));
    all.push(("draft_get", drafts::GET));
    all.push(("draft_open_count", drafts::OPEN_COUNT));
    all.push(("draft_expiry", drafts::EXPIRE));
    all.push(("c11_open", ops::restart::C11_OPEN));
    all.push(("config_server_of", ops::restart::CONFIG_SERVER_OF));
    all.push(("auto_notify_switch", autonotify::SWITCH));
    all.extend(ops::pickup::queries());
    all.extend(assess::queries());
    all.extend(vault::store_queries());
    all.extend(prs::job::queries());
    all.extend(prs::links::queries());
    all
}
