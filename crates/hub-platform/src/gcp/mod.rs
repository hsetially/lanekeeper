//! Google Cloud without an SDK (T2, S5, S7, S18): the access token, Secret Manager and Cloud KMS.
//!
//! The hub calls a handful of REST methods, so it speaks them itself over one small client ([`http`]) instead of carrying
//! a Google SDK with its own TLS and credential stacks. What is here:
//!
//! - [`http`]: the client. TLS 1.3, a host allowlist, a timeout per attempt, capped bodies, and errors that carry codes only.
//! - [`api`]: retries with full jitter, a deadline, the bearer token (renewed once on a `401`), and metrics.
//! - [`token`]: the workload's access token from the metadata server (Workload Identity).
//! - [`secret_manager`]: [`SecretManagerSource`], a `ports::SecretSource` with a five-minute bounded cache.
//! - [`kms`]: [`CloudKmsSigner`] and [`CloudKmsEnvelope`], the `ports::KmsSigner` and `ports::KmsEnvelope` of the CA and of
//!   the token vault.
//!
//! Every call carries and checks CRC-32C checksums where Google offers them, so a request or an answer that was damaged
//! on the way is refused instead of signed or stored.

pub mod api;
mod crc32c;
pub mod http;
pub mod kms;
pub mod metrics;
pub mod retry;
pub mod secret_manager;
pub mod token;
mod ttl_cache;
pub mod wire;

pub use api::{ApiError, CallLimits, GoogleApi, Transport};
pub use http::{GoogleHttp, HostPolicy, HttpError, HttpLimits, Origin};
pub use kms::{CloudKmsEnvelope, CloudKmsSigner, PublicKey};
pub use metrics::GoogleMetrics;
pub use retry::RetryPolicy;
pub use secret_manager::SecretManagerSource;
pub use token::{AccessTokens, TokenError, WorkloadIdentityTokens};
pub use wire::ProjectId;

/// Where the three Google services live. [`GoogleEndpoints::production`] is the only value outside tests; every origin is
/// checked against [`HostPolicy`] when a request is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleEndpoints {
    pub metadata: Origin,
    pub secret_manager: Origin,
    pub kms: Origin,
}

impl GoogleEndpoints {
    /// The real services.
    #[must_use]
    pub fn production() -> Self {
        Self {
            metadata: Origin::http("metadata.google.internal"),
            secret_manager: Origin::https("secretmanager.googleapis.com"),
            kms: Origin::https("cloudkms.googleapis.com"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_endpoints_are_allowed_by_the_google_policy() {
        let endpoints = GoogleEndpoints::production();
        let policy = HostPolicy::google();
        for origin in [&endpoints.metadata, &endpoints.secret_manager, &endpoints.kms] {
            assert_eq!(policy.check(origin), Ok(()));
        }
        assert_eq!(endpoints.kms.host(), "cloudkms.googleapis.com");
        assert_eq!(endpoints.secret_manager.host(), "secretmanager.googleapis.com");
        assert_eq!(endpoints.metadata.host(), "metadata.google.internal");
    }
}
