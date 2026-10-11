//! The Git mirror, the webhook and everything that keeps them current (T7, D64, P2, S18).
//!
//! - [`credentials`]: the read-only service credential behind `GitCredentials` (Q17, A12).
//! - [`mirror`]: the bare mirrors, fetched by `gix` with one fetch per repository at a time.
//! - [`metrics`]: counters and histograms with closed label sets.
//! - [`sync`], [`poll`]: announcing `GitHeadMoved`, the worker that fetches after a webhook, the replicas that fetch after an announcement, and the leased poll.
//! - [`reader`]: `GixReader`, the `GitReader` port over the mirrors, with the read-through fetch for a missing commit.
//! - [`tree_index`]: the per-commit index of every file, cached by weight and persisted to `git_tree_index`.
//! - [`webhook`], [`deliveries`]: `POST /hooks/github` and its replay guard on Postgres, with the prune job.
//! - [`pr_webhook`]: the `pull_request` event: the state machine, the `pr_links` update, its audit event and `PrStateChanged` in one transaction.
//! - [`fetch_queue`]: the bounded, coalescing queue between the webhook and the fetch worker.
//! - [`payload`]: the typed, bounded parse of GitHub's `push` and `pull_request` payloads.
pub mod credentials;
pub mod deliveries;
pub mod fetch_queue;
pub mod metrics;
pub mod mirror;
pub mod payload;
pub mod poll;
pub mod pr_webhook;
pub mod reader;
pub mod sync;
pub mod tree_index;
pub mod webhook;
