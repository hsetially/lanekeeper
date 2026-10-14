//! Shared test support for the hub-registry integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod counting_git;
pub mod fixtures;
pub mod gitload;
pub mod lane;
pub mod log_capture;
pub mod perf;
pub mod pg;
pub mod pg_audit;
pub mod pg_blobs;
pub mod pg_bus;
pub mod rig;
pub mod world;
