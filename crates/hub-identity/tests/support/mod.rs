//! Shared test support: a fake outbound HTTPS client, a mock Entra, a log capture.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc
)]

pub mod fake_https;
pub mod log_capture;
pub mod login_rig;
pub mod mock_oidc;
