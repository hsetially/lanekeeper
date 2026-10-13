//! T2: the same conformance suites that the `ports` fakes pass, run against the real implementations on the fake Google
//! server (S5, S7). A real implementation that passes here and against a real project honours the port.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use ports::KeyRef;
use ports::conformance;
use support::fake_google::{Harness, SIGN_KEY, UNKNOWN_SIGN_KEY};
use support::soft_kms::SoftKmsSigner;

#[tokio::test(start_paused = true)]
async fn secret_manager_source_conformance() {
    let h = Harness::new();
    h.fake.set_secret("db-password", "s3cr3t-value");
    conformance::secret_source(&h.secret_source(), "db-password", "s3cr3t-value", "absent-secret").await;
}

#[tokio::test(start_paused = true)]
async fn kms_signer_conformance() {
    let h = Harness::new();
    let known = KeyRef::parse(SIGN_KEY).unwrap();
    let unknown = KeyRef::parse(UNKNOWN_SIGN_KEY).unwrap();
    conformance::kms_signer(&h.signer(), &known, &unknown).await;
}

#[tokio::test(start_paused = true)]
async fn kms_envelope_conformance() {
    let h = Harness::new();
    conformance::kms_envelope(&h.envelope()).await;
}

/// The signer that the CA tests use is held to the same contract as the real one.
#[tokio::test]
async fn soft_kms_signer_conforms() {
    let known = KeyRef::parse(SIGN_KEY).unwrap();
    let unknown = KeyRef::parse(UNKNOWN_SIGN_KEY).unwrap();
    conformance::kms_signer(&SoftKmsSigner::new(&[&known]), &known, &unknown).await;
}
