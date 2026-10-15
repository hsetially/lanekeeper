//! Plan 07, T1: authentication of the MCP server (S1, S3, S4, S7, S9, S10, S11, S14, S21), in one test binary.
//!
//! One binary and not a file per topic: each integration test binary links the whole of rmcp, axum and sqlx, and the disk of the
//! machines that build this is not unlimited. The modules keep the names of the plan's traceability table
//! (`auth_oauth::pending_user_is_403`, `auth_pat::at_most_five_live_tokens_per_user`, ...).
//!
//! The tests that need a real Postgres 16 (`auth_pat_pg`) start one with testcontainers, or use the throwaway server named by
//! `LK_TEST_PG_ADMIN_URL`. Without either they print `SKIPPED` and pass, unless `LK_REQUIRE_DOCKER=1` (set by `just verify-07`),
//! which makes a missing database a failure. A `SKIPPED` line means those tests did not run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

#[path = "../support/mod.rs"]
mod support;

mod rig;

mod auth_header;
mod auth_oauth;
mod auth_pat;
mod auth_pat_pg;
mod corpus_replay;
mod errors;
mod logging;
mod pg;
mod route_table;
mod server;
mod source_rules;
mod token_routes;
mod token_syntax;
