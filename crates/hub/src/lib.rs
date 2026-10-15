//! Hub composition: listeners, security headers, limits, static files, operations.
//!
//! Owned by prompt 03b. Read `AGENTS.md` and `prompts/03b*.md` before changing this crate.
//!
//! `hub-platform` and the other hub crates are libraries of components and routes. This crate is what turns them into one
//! process: it applies the headers and limits that every response and request passes through (S11, S12), serves the web app,
//! answers the operations endpoints, and drains on shutdown.
#![forbid(unsafe_code)]
// Documenting `# Errors` on every small constructor and check adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod blobs;
pub mod client_ip;
pub mod compose;
pub mod config;
pub mod db;
pub mod edge;
pub mod events;
pub mod git;
pub mod headers;
pub mod limits;
pub mod mcp;
pub mod metrics;
pub mod ops;
pub mod random;
pub mod serve;
pub mod shutdown;
pub mod static_files;
pub mod telemetry;

pub use blobs::BlobsModule;
pub use compose::{BuildError, Deps, Hub, Module};
pub use config::{ConfigError, HubSettings};
pub use events::EventsModule;
pub use git::GitModule;
pub use mcp::McpModule;
