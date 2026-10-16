//! MCP server for Cursor (rmcp, Streamable HTTP), with elicitation-confirmed writes and redaction.
//!
//! Owned by prompt 07. Read `AGENTS.md` and `prompts/07*.md` before changing this crate.
//!
//! - [`config`]: the deployment switches (authentication mode, confirmation mode, content mode) and the public origin (T0, T1).
//! - [`auth`]: who is calling (S14): Entra access tokens or personal tokens, never both, and the REST routes for personal tokens (T1).
//! - [`server`]: rmcp's Streamable HTTP at `/mcp` behind a gate, and [`server::mount`], the one call the hub makes (T1).
//! - [`tools`], [`output`], [`limits`]: the 27 tools of the contract, their results, and what one caller may ask of the server (T2);
//!   the secret scan of every text, the final sweep of every result and the denied-file floor (T6).
//! - [`confirm`]: the confirmation of every write that acts on the world (D52): the question to the user, what the answer means, the
//!   draft link for a client that cannot be asked, and the keys that make a retry safe (T3).
//! - [`error`], [`clock`]: the errors this crate answers with, and the time source.
//!
//! The compatibility stub of T0 is `examples/t0_stub.rs`. It is not part of this library, so nothing that reads a token without
//! verifying it can end up in the hub.
#![forbid(unsafe_code)]
// Documenting `# Errors` on every small constructor and check adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod auth;
pub mod clock;
pub mod config;
pub mod confirm;
pub mod error;
pub mod limits;
pub mod metrics;
pub mod output;
pub mod schemas;
pub mod server;
pub mod tools;

pub use error::McpError;
pub use server::{Credentials, McpDeps, McpSettings, McpState, StartError, mount};
