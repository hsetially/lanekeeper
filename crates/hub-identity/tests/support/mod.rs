//! Shared test support: a fake outbound HTTPS client, a mock Entra, a log capture, and (from T2) a real Postgres with a
//! test-only audit log and event bus.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::print_stderr
)]

pub mod fake_https;
pub mod log_capture;
pub mod login_rig;
pub mod mock_oidc;
pub mod pg;
pub mod pg_bus;
pub mod sessions_rig;
pub mod simple_audit;
pub mod users_rig;
pub mod web_rig;
