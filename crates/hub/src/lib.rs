//! Hub composition: listeners, security headers, limits, static files, operations.
//!
//! Owned by prompt 18 (hub assembly), which built on the headers, limits and operations listener that prompt 03b started this crate
//! with. Read `AGENTS.md` and `prompts/18-*.md` before changing this crate.
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

pub mod allowances;
pub mod blobs;
pub mod ca_init;
pub mod cli;
pub mod client_ip;
pub mod compose;
pub mod config;
pub mod db;
pub mod docs;
pub mod edge;
pub mod edges;
pub mod events;
pub mod git;
pub mod headers;
pub mod identity;
pub mod job_wrappers;
pub mod limits;
pub mod mcp;
pub mod metrics;
pub mod metrics_bridge;
pub mod migrate;
pub mod notify;
pub mod ops;
pub mod process;
pub mod random;
pub mod ready;
pub mod registry;
pub mod schema;
pub mod seed;
pub mod serve;
pub mod session_auth;
pub mod shutdown;
pub mod static_files;
pub mod telemetry;
#[cfg(feature = "testing")]
pub mod testing;
pub mod user_cache;
pub mod web_tls;
pub mod wiring;
pub mod writes;

pub use blobs::BlobsModule;
pub use compose::{BuildError, Deps, Hub, Module, MountedRouter, SessionEdge};
pub use config::{ConfigError, HubSettings};
pub use events::EventsModule;
pub use git::GitModule;
pub use mcp::McpModule;
