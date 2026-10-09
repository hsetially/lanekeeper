//! Contract tests for every port: `ports::conformance::<port>(impl)` (feature `conformance`).
//!
//! Each function takes an implementation and asserts the behaviour that `docs/interfaces.md` and the port's
//! rustdoc promise, panicking with a message on the first violation. Run them from `#[tokio::test]` in the
//! crate that owns the implementation; the fakes in [`crate::fakes`] run them in this crate's own tests, and
//! every real implementation must run the same function.
//!
//! - Suites for ports whose behaviour needs set-up the port itself cannot do (making an agent connect,
//!   seeding files, reading back the audit chain) take a small *scenario* or *probe* trait. The real
//!   implementation's test crate implements it over its own test harness.
//! - Timing assertions use the tokio clock. Run suites for in-process implementations in
//!   `#[tokio::test(start_paused = true)]` so that they take no wall-clock time.
//! - [`route_guard`] is the S4 route-table harness.
//!
//! This module is test support: it asserts with `unwrap` and `panic!` on purpose.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    // A suite is one long scenario: splitting it would hide the order that its steps depend on.
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation
)]

mod audit;
mod auth;
mod blob;
mod docs;
mod events;
mod gateway;
mod git;
mod kms;
mod lease;
mod notify;
mod read;
mod sink;
mod write;

pub mod route_guard;
pub mod sample;

pub use audit::{AuditProbe, audit_log};
pub use auth::{TokenFixtures, UsersScenario, token_verifier, users};
pub use blob::blob_store;
pub use docs::{DocScenario, doc_search};
pub use events::event_bus;
pub use gateway::{AgentGatewayScenario, agent_gateway};
pub use git::{GitExpectation, git_reader};
pub use kms::{kms_envelope, kms_signer, secret_source};
pub use lease::leases;
pub use notify::{NotifierProbe, notifier};
pub use read::{RegistryScenario, registry_read};
pub use sink::{SentinelProbe, report_sink, sentinel_sink};
pub use write::{WriteScenario, write_service};
