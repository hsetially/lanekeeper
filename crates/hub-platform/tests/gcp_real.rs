//! T2: the same conformance suites against a real Google Cloud project. Opt in, outside CI.
//!
//! Run it on a machine that has a project to spend on it:
//!
//! ```text
//! LK_KMS_TEST_KEY=projects/P/locations/L/keyRings/R/cryptoKeys/sign-key/cryptoKeyVersions/1 \
//! LK_KMS_TEST_UNKNOWN_KEY=projects/P/locations/L/keyRings/R/cryptoKeys/does-not-exist/cryptoKeyVersions/1 \
//! LK_KMS_TEST_ENVELOPE_KEY=projects/P/locations/L/keyRings/R/cryptoKeys/envelope-key \
//! LK_SECRET_TEST_PROJECT=P LK_SECRET_TEST_NAME=lk-test-secret LK_SECRET_TEST_VALUE=the-value \
//! LK_SECRET_TEST_MISSING=lk-test-secret-that-does-not-exist \
//! LK_TEST_ACCESS_TOKEN="$(gcloud auth print-access-token)" \
//! cargo test -p hub-platform --test gcp_real -- --ignored
//! ```
//!
//! `LK_KMS_TEST_KEY` is an `EC_SIGN_P256_SHA256` key version and `LK_KMS_TEST_ENVELOPE_KEY` a symmetric encryption key.
//! Without `LK_TEST_ACCESS_TOKEN` the token comes from the metadata server, as in production, so the test also runs on a
//! GKE pod with Workload Identity. A missing variable fails the test and says which: an opt-in test that skips is not
//! a test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Secret;
use hub_platform::gcp::http::{GoogleHttp, HttpLimits};
use hub_platform::gcp::secret_manager::{DEFAULT_CACHE_ENTRIES, MAX_CACHE_TTL};
use hub_platform::gcp::token::{AccessTokens, TokenError, WorkloadIdentityTokens};
use hub_platform::gcp::{
    CallLimits, CloudKmsEnvelope, CloudKmsSigner, GoogleApi, GoogleEndpoints, GoogleMetrics, ProjectId,
    SecretManagerSource, Transport,
};
use ports::{KeyRef, conformance};

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("set {name} to run the real-project test (see the file's docs)"))
}

/// A developer's access token from the environment, for a machine that is not a GKE pod.
struct StaticToken(Secret<String>);

impl fmt::Debug for StaticToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticToken").finish_non_exhaustive()
    }
}

#[async_trait]
impl AccessTokens for StaticToken {
    async fn token(&self) -> Result<Secret<String>, TokenError> {
        Ok(Secret::new(self.0.expose().clone()))
    }

    async fn refused(&self, _token: &Secret<String>) {}
}

fn api() -> (GoogleApi, GoogleEndpoints) {
    let endpoints = GoogleEndpoints::production();
    let http = GoogleHttp::production(HttpLimits::default()).unwrap();
    let transport = Transport::new(http, CallLimits::default(), GoogleMetrics::default());
    let tokens: Arc<dyn AccessTokens> = match std::env::var("LK_TEST_ACCESS_TOKEN") {
        Ok(token) => Arc::new(StaticToken(Secret::new(token))),
        Err(_) => Arc::new(WorkloadIdentityTokens::new(
            transport.clone(),
            endpoints.metadata.clone(),
        )),
    };
    (GoogleApi::new(transport, tokens), endpoints)
}

#[tokio::test]
#[ignore = "needs a real Google Cloud project: see the file's docs"]
async fn real_cloud_kms_signer_conforms() {
    let (api, endpoints) = api();
    let signer = CloudKmsSigner::new(api, endpoints.kms);
    let known = KeyRef::parse(&env("LK_KMS_TEST_KEY")).unwrap();
    let unknown = KeyRef::parse(&env("LK_KMS_TEST_UNKNOWN_KEY")).unwrap();
    conformance::kms_signer(&signer, &known, &unknown).await;
    assert!(!signer.public_key(&known).await.unwrap().spki_der().is_empty());
}

#[tokio::test]
#[ignore = "needs a real Google Cloud project: see the file's docs"]
async fn real_cloud_kms_envelope_conforms() {
    let (api, endpoints) = api();
    let key = KeyRef::parse(&env("LK_KMS_TEST_ENVELOPE_KEY")).unwrap();
    let envelope = CloudKmsEnvelope::new(api, endpoints.kms, &key).unwrap();
    conformance::kms_envelope(&envelope).await;
}

#[tokio::test]
#[ignore = "needs a real Google Cloud project: see the file's docs"]
async fn real_secret_manager_source_conforms() {
    let (api, endpoints) = api();
    let project = ProjectId::parse(&env("LK_SECRET_TEST_PROJECT")).unwrap();
    let source = SecretManagerSource::new(
        api,
        endpoints.secret_manager,
        project,
        MAX_CACHE_TTL,
        DEFAULT_CACHE_ENTRIES,
    );
    conformance::secret_source(
        &source,
        &env("LK_SECRET_TEST_NAME"),
        &env("LK_SECRET_TEST_VALUE"),
        &env("LK_SECRET_TEST_MISSING"),
    )
    .await;
}
