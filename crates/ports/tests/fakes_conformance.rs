//! Every fake must pass the conformance suite that every real implementation will also run (AC2).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ports::conformance::{self, GitExpectation, TokenFixtures};
use ports::fakes::{
    FakeAgentGateway, FakeAuditLog, FakeBlobStore, FakeDocSearch, FakeEventBus, FakeGit, FakeKmsEnvelope,
    FakeKmsSigner, FakeLeases, FakeNotifier, FakeRegistry, FakeReportSink, FakeSecretSource,
    FakeSentinelSink, FakeTokenVerifier, FakeTxFactory, FakeUsers, FakeWriteService,
};
use ports::{KeyRef, TxFactory};

#[tokio::test(start_paused = true)]
async fn fake_agent_gateway_conforms() {
    conformance::agent_gateway(&FakeAgentGateway::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_report_sink_conforms() {
    conformance::report_sink(&FakeReportSink::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_blob_store_conforms() {
    conformance::blob_store(&FakeBlobStore::new()).await;
}

#[test]
fn fake_git_reader_conforms() {
    let (git, expect): (FakeGit, GitExpectation) = FakeGit::with_conformance_repo().unwrap();
    conformance::git_reader(&git, &expect);
}

#[tokio::test(start_paused = true)]
async fn fake_event_bus_conforms() {
    let bus = FakeEventBus::new(8);
    let txf = FakeTxFactory::new();
    conformance::event_bus(&bus, &txf, 8).await;
}

#[tokio::test(start_paused = true)]
async fn fake_leases_conforms() {
    conformance::leases(&FakeLeases::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_kms_signer_conforms() {
    let known = KeyRef::parse("projects/p/keys/audit").unwrap();
    let unknown = KeyRef::parse("projects/p/keys/other").unwrap();
    let signer = FakeKmsSigner::new(&[known.clone()]);
    conformance::kms_signer(&signer, &known, &unknown).await;
}

#[tokio::test(start_paused = true)]
async fn fake_kms_envelope_conforms() {
    conformance::kms_envelope(&FakeKmsEnvelope::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_secret_source_conforms() {
    let src = FakeSecretSource::new().with("db-password", "s3cr3t-value");
    conformance::secret_source(&src, "db-password", "s3cr3t-value", "absent-secret").await;
}

#[tokio::test(start_paused = true)]
async fn fake_notifier_conforms() {
    conformance::notifier(&FakeNotifier::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_token_verifier_conforms() {
    let fixtures: TokenFixtures = conformance::sample::token_fixtures();
    let verifier = FakeTokenVerifier::from_fixtures(&fixtures);
    conformance::token_verifier(&verifier, &fixtures).await;
}

#[tokio::test(start_paused = true)]
async fn fake_users_conforms() {
    conformance::users(&FakeUsers::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_audit_log_conforms() {
    let log = FakeAuditLog::new();
    let txf: &dyn TxFactory = &FakeTxFactory::new();
    conformance::audit_log(&log, txf).await;
}

#[tokio::test(start_paused = true)]
async fn fake_registry_read_conforms() {
    conformance::registry_read(&FakeRegistry::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_write_service_conforms() {
    conformance::write_service(&FakeWriteService::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_doc_search_conforms() {
    conformance::doc_search(&FakeDocSearch::new()).await;
}

#[tokio::test(start_paused = true)]
async fn fake_sentinel_sink_conforms() {
    conformance::sentinel_sink(&FakeSentinelSink::new()).await;
}
