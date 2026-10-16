//! Shared test support: a real Postgres (migrated as the owner, used as the app role), test doubles of the ports over the real
//! tables (audit chain, outbox, blobs, tool-write registrations), a fake agent with an in-memory NFS, and a log capture.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::large_futures
)]

pub mod fake_agent;
pub mod fake_git;
pub mod fake_github;
pub mod fake_teams;
pub mod live_registry;
pub mod log_capture;
pub mod pg;
pub mod pg_audit;
pub mod pg_blobs;
pub mod pg_bus;
pub mod pg_tool_writes;
pub mod recording_leases;
pub mod rig;
pub mod withholding_registry;
