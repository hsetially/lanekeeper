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
//! - [`routes`]: the guarded router every route of the hub is registered through (S4).
#![forbid(unsafe_code)]
// Documenting `# Errors` on every small constructor and check adds noise; the error enums are the docs.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod error;
pub mod gcp;
pub mod routes;
pub mod session_auth;

pub use error::{ApiError, PROBLEM_CONTENT_TYPE, ProblemCode};
pub use routes::{GuardedRouter, RouteError, RouteGuard, RouteSpec, ops_routes, platform_routes};
pub use session_auth::{AuthenticatedUser, SessionAuth, UserThrottle};
#[cfg(any(test, feature = "fakes"))]
pub use session_auth::{FakeSessionAuth, TEST_USER_HEADER};
