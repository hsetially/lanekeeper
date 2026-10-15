//! MCP server for Cursor (rmcp, Streamable HTTP), with elicitation-confirmed writes and redaction.
//!
//! Owned by prompt 07. Read `AGENTS.md` and `prompts/07*.md` before changing this crate.
//!
//! T0 (plan 07) adds only [`config`]: the three deployment switches whose defaults stay in place until a human records the
//! Cursor 3.22.12 results. The compatibility stub that produces those results is `examples/t0_stub.rs`; it is not part of this
//! library, so nothing that reads a token without verifying it can end up in the hub.
#![forbid(unsafe_code)]

pub mod config;
