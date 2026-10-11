//! Agent gateway and KMS-backed CA, replica routing, outbox event bus and SSE, leases, Git mirror and webhooks, blob store.
//!
//! Owned by prompt 03b. Read `AGENTS.md` and `prompts/03b*.md` before changing this crate.
//!
//! This crate is a library, so that a test can start several hubs in one process. `crates/hub` composes it and owns
//! everything that is about the process: listeners, headers, limits, static files, shutdown and telemetry.
//!
//! - [`error`]: `application/problem+json` and the closed set of API errors (S21).
//! - [`session_auth`]: how a request becomes an active user, until the identity crate (03a) plugs its own in.
//! - [`gcp`]: the access token, Secret Manager and Cloud KMS without an SDK, behind the `SecretSource`, `KmsSigner` and
//!   `KmsEnvelope` ports (S5, S7, S18).
//! - [`ca`]: the agent CA over `KmsSigner` (S5): profiles, CSR parsing, issuance. The CA's private key is never held.
//! - [`clock`]: the instants that go into certificates and audit events, behind a trait.
//! - [`tls`], [`listener`], [`server_identity`]: TLS 1.3 only with client certificates, the gRPC accept loop that puts the verified
//!   peer identity into each request, and the hub's own certificate, renewed at half its life (S5, S6).
//! - [`join`], [`agent_service`], [`stream`], [`agent_metrics`]: `Join` and the `Connect` stream of agents (S5, S9, S11).
//! - [`routes`]: the guarded router every route of the hub is registered through (S4).
#![forbid(unsafe_code)]
// Documenting `# Errors` on every small constructor and check adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod agent_metrics;
pub mod agent_service;
pub mod ca;
pub mod clock;
pub mod error;
pub mod gcp;
pub mod join;
pub mod listener;
pub mod routes;
pub mod server_identity;
pub mod session_auth;
pub mod stream;
pub mod tls;

pub use error::{ApiError, PROBLEM_CONTENT_TYPE, ProblemCode};
pub use routes::{GuardedRouter, RouteError, RouteGuard, RouteSpec, ops_routes, platform_routes};
pub use session_auth::{AuthenticatedUser, SessionAuth, UserThrottle};
#[cfg(any(test, feature = "fakes"))]
pub use session_auth::{FakeSessionAuth, TEST_USER_HEADER};
