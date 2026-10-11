//! T5 (S9): the nightly verification finds tampering, and alerts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::Severity;
use hub_identity::audit::bucket::{CheckpointBucket, object_name};
use hub_identity::audit::checkpoint::CheckpointObject;
use hub_identity::audit::verify::{FailureKind, NightlyOutcome};
use hub_identity::audit::{MemoryBucket, VerifyFailure};
use ports::{Leases, NotificationKind};
use support::audit_rig::AuditRig;
use support::log_capture;

fn failure(seq: i64, kind: FailureKind) -> VerifyFailure {
    VerifyFailure { seq, kind }
}

/// 12 events with checkpoints at 6 and 12.
async fn rig_with_checkpoints(db: support::pg::TestDb) -> AuditRig {
    let rig = AuditRig::new(db).await;
    let job = rig.checkpoint_job();
    rig.record(6).await;
    job.run_once().await.unwrap();
    rig.record(6).await;
    job.run_once().await.unwrap();
    rig
}

#[tokio::test]
async fn nightly_verify_passes_on_clean_chain() {
    let rig = rig_with_checkpoints(db!()).await;
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Clean(report) = outcome else {
        panic!("expected a clean run, got {outcome:?}");
    };
    assert_eq!(report.events_checked, 12);
    assert_eq!(report.checkpoints_checked, 2);
    assert!(report.failures.is_empty());
    assert_eq!(rig.metrics.audit_verify_runs_total(), 1);
    assert_eq!(rig.metrics.audit_verify_failures_total(), 0);
    assert!(rig.notifier.sent().is_empty(), "no alert for a clean chain");
}

#[tokio::test]
async fn an_empty_log_verifies() {
    let rig = AuditRig::new(db!()).await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert!(report.is_clean());
    assert_eq!((report.events_checked, report.checkpoints_checked), (0, 0));
}

#[tokio::test]
async fn tampered_row_is_detected() {
    // (a) The stored event is changed and nothing else: its hash no longer matches. The table's own check is dropped first, as an
    // owner who meant to would.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 3",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::ChainHashMismatch)]);
}

#[tokio::test]
async fn a_tamper_that_keeps_the_row_consistent_breaks_the_next_link() {
    // (b) Event and hash are rewritten together, so the table's check passes. Only the row after it exposes the change.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed'), \
         hash = sha256(prev_hash || convert_to(replace(event_json, 'settings_changed', 'role_changed'), 'UTF8')) \
         WHERE seq = 3",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(4, FailureKind::ChainLinkBroken)]);
}

#[tokio::test]
async fn deleted_middle_row_is_detected() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql("DELETE FROM audit_events WHERE seq = 4").await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(5, FailureKind::ChainLinkBroken)]);
}

#[tokio::test]
async fn truncated_tail_is_detected_by_checkpoint() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    // The newest events and nothing else are removed; the rest of the chain is still consistent.
    rig.admin_sql("DELETE FROM audit_events WHERE seq > 9").await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.events_checked, 9);
    assert_eq!(report.failures, [failure(12, FailureKind::CheckpointHeadMissing)]);
}

#[tokio::test]
async fn a_rewritten_tail_is_detected_by_checkpoint() {
    // The last events are replaced by others with a valid chain: only the checkpoint knows the old head.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed'), \
         hash = sha256(prev_hash || convert_to(replace(event_json, 'settings_changed', 'role_changed'), 'UTF8')) \
         WHERE seq = 12",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    // The chain alone is consistent; the checkpoint of seq 12 signed another head.
    assert_eq!(
        report.failures,
        [failure(12, FailureKind::CheckpointHeadMismatch)]
    );
}

#[tokio::test]
async fn forged_checkpoint_row_is_detected() {
    let rig = AuditRig::new(db!()).await;
    rig.record(5).await;
    let head: Vec<u8> = sqlx::query_scalar("SELECT hash FROM audit_events WHERE seq = 3")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    // The app role may insert checkpoint rows, so a hub with a bug (or a stolen database credential) can write a forged one.
    sqlx::query(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) VALUES (3, $1, $2, $3, $4)",
    )
    .bind(&head)
    .bind(support::audit_rig::VERSION)
    .bind(vec![0x30u8, 0x06, 1, 2, 3, 4, 5, 6])
    .bind(object_name(3))
    .execute(&rig.pool)
    .await
    .unwrap();
    // No object in the bucket: the second copy does not exist.
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::BucketObjectMissing)]);

    // With a matching object, the signature is what fails.
    let object = CheckpointObject {
        seq: 3,
        head: domain::ContentHash::from_bytes(head.try_into().unwrap()),
        signature: vec![0x30, 0x06, 1, 2, 3, 4, 5, 6],
        key_version: support::ecdsa_kms::EcdsaKms::key(),
        created_at: domain::Timestamp::from_unix_millis(1),
    };
    rig.bucket
        .put(&object_name(3), object.encode().unwrap())
        .await
        .unwrap();
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::SignatureInvalid)]);
}

#[tokio::test]
async fn a_checkpoint_row_with_a_head_the_chain_does_not_have_is_detected() {
    let rig = AuditRig::new(db!()).await;
    rig.record(5).await;
    // The foreign key forbids this; an owner switches it off.
    rig.admin_sql(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         VALUES (3, sha256('forged'), 'v', '\\x3006010203040506', 'audit/checkpoints/3.json')",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::CheckpointHeadMismatch)]);
}

#[tokio::test]
async fn bucket_object_mismatch_is_detected() {
    let rig = rig_with_checkpoints(db!()).await;
    let name = object_name(6);
    let original = CheckpointObject::parse(&rig.bucket.get(&name).await.unwrap().unwrap()).unwrap();

    // A bucket whose object for 6 has another signature; the one for 12 is the real one.
    let other = Arc::new(MemoryBucket::new());
    let mut changed = original.clone();
    changed.signature[4] ^= 1;
    other.put(&name, changed.encode().unwrap()).await.unwrap();
    other
        .put(
            &object_name(12),
            rig.bucket.get(&object_name(12)).await.unwrap().unwrap(),
        )
        .await
        .unwrap();
    let report = rig.chain_verifier_with(other).verify().await.unwrap();
    assert_eq!(report.failures, [failure(6, FailureKind::BucketObjectMismatch)]);

    // A bucket that lacks the object for 12.
    let missing = Arc::new(MemoryBucket::new());
    missing
        .put(&name, rig.bucket.get(&name).await.unwrap().unwrap())
        .await
        .unwrap();
    let report = rig.chain_verifier_with(missing).verify().await.unwrap();
    assert_eq!(report.failures, [failure(12, FailureKind::BucketObjectMissing)]);

    // An object that is not a checkpoint at all.
    let junk = Arc::new(MemoryBucket::new());
    junk.put(&name, Bytes::from_static(b"not json")).await.unwrap();
    junk.put(
        &object_name(12),
        rig.bucket.get(&object_name(12)).await.unwrap().unwrap(),
    )
    .await
    .unwrap();
    let report = rig.chain_verifier_with(junk).verify().await.unwrap();
    assert_eq!(report.failures, [failure(6, FailureKind::BucketObjectMismatch)]);
}

#[tokio::test]
async fn a_bucket_that_cannot_be_read_is_a_failure_to_verify() {
    struct Down;
    #[async_trait::async_trait]
    impl CheckpointBucket for Down {
        async fn put(
            &self,
            _: &str,
            _: Bytes,
        ) -> Result<hub_identity::audit::PutOutcome, hub_identity::audit::BucketError> {
            Err(hub_identity::audit::BucketError::Unavailable)
        }
        async fn get(&self, _: &str) -> Result<Option<Bytes>, hub_identity::audit::BucketError> {
            Err(hub_identity::audit::BucketError::Unavailable)
        }
    }
    let rig = rig_with_checkpoints(db!()).await;
    let report = rig.chain_verifier_with(Arc::new(Down)).verify().await.unwrap();
    assert_eq!(
        report.failures,
        [
            failure(6, FailureKind::BucketUnavailable),
            failure(12, FailureKind::BucketUnavailable)
        ]
    );
}

#[tokio::test]
async fn a_signature_by_another_key_is_detected() {
    let rig = rig_with_checkpoints(db!()).await;
    // The verifier trusts a different key than the one that signed.
    let other = support::ecdsa_kms::EcdsaKms::new(rig.pool.clone());
    let wrong = Arc::new(
        hub_identity::audit::EcdsaP256Verifier::new()
            .with_spki_der(support::audit_rig::VERSION, &other.spki_der())
            .unwrap(),
    );
    let report = hub_identity::audit::ChainVerifier::new(rig.pool.clone(), rig.bucket.clone(), wrong)
        .verify()
        .await
        .unwrap();
    assert_eq!(
        report.failures,
        [
            failure(6, FailureKind::SignatureInvalid),
            failure(12, FailureKind::SignatureInvalid)
        ]
    );
}

#[tokio::test]
async fn verify_report_lists_first_bad_seq_only() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    // Two separate changes; every row after the first would also fail, and the report does not list them all.
    rig.admin_sql("UPDATE audit_events SET event_json = event_json || ' ' WHERE seq IN (3, 8)")
        .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::ChainHashMismatch)]);
    assert_eq!(report.events_checked, 3, "the walk stops at the first bad row");
}

#[tokio::test]
async fn nightly_verify_streams_in_bounded_batches() {
    let rig = AuditRig::new(db!()).await;
    rig.record(25).await;
    let report = rig.chain_verifier().with_batch_rows(10).verify().await.unwrap();
    assert!(report.is_clean());
    assert_eq!(report.events_checked, 25);
    assert_eq!(report.batches, 3, "10 + 10 + 5");
    // A log that fills the last batch exactly ends on an empty read, not on a guess.
    let report = rig.chain_verifier().with_batch_rows(25).verify().await.unwrap();
    assert_eq!(report.events_checked, 25);
    assert_eq!(report.batches, 1);
}

#[tokio::test]
async fn nightly_verify_mismatch_alerts_metric_and_notifier() {
    let (capture, _guard) = log_capture::install();
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    rig.admin_sql("UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'SECRET-EVENT-TEXT') WHERE seq = 5")
        .await;
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(report.failures, [failure(5, FailureKind::ChainHashMismatch)]);

    // The metric.
    assert_eq!(rig.metrics.audit_verify_failures_total(), 1);
    // The notifier: one critical finding that names the position and says nothing of the content.
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert_eq!(sent[0].severity, Severity::Critical);
    assert!(
        sent[0].summary.as_str().contains("position 5"),
        "{:?}",
        sent[0].summary
    );
    // The log: the position and the reason, never the event.
    let log = capture.text();
    assert!(
        log.contains("audit chain verification failed") && log.contains("seq=5"),
        "{log}"
    );
    assert!(
        !log.contains("SECRET-EVENT-TEXT"),
        "event content reached the log"
    );
    assert!(!format!("{sent:?}").contains("SECRET-EVENT-TEXT"));
}

#[tokio::test]
async fn a_failing_notifier_does_not_hide_the_failure() {
    use async_trait::async_trait;
    use ports::{Notification, Notifier, NotifyError};
    struct Broken;
    #[async_trait]
    impl Notifier for Broken {
        async fn notify(&self, _: Notification) -> Result<(), NotifyError> {
            Err(NotifyError::Unavailable)
        }
    }
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql("DELETE FROM audit_events WHERE seq = 2").await;
    let job = hub_identity::audit::NightlyVerifyJob::new(
        rig.chain_verifier(),
        rig.leases.clone(),
        Arc::new(Broken),
        rig.metrics.clone(),
    );
    assert!(matches!(job.run_once().await.unwrap(), NightlyOutcome::Failed(_)));
    assert_eq!(
        rig.metrics.audit_verify_failures_total(),
        1,
        "the metric still rises"
    );
}

#[tokio::test]
async fn nightly_verify_requires_the_lease_and_keeps_it() {
    let rig = rig_with_checkpoints(db!()).await;
    let other = rig
        .leases
        .try_acquire("audit.verify", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rig.nightly().run_once().await.unwrap(), NightlyOutcome::NotLeader);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 0);
    other.release().await.unwrap();
    assert!(matches!(
        rig.nightly().run_once().await.unwrap(),
        NightlyOutcome::Clean(_)
    ));
    // Replicas that wake at the same moment find the lease still held, so one night gives one run and one alert.
    assert_eq!(rig.nightly().run_once().await.unwrap(), NightlyOutcome::NotLeader);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 1);
}

#[tokio::test]
async fn a_database_that_cannot_be_read_is_an_error_not_a_pass() {
    let rig = rig_with_checkpoints(db!()).await;
    // Cut the verifier's pool off.
    let verifier = rig.chain_verifier();
    rig.pool.close().await;
    assert!(verifier.verify().await.is_err());
    let job = rig.nightly();
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 0);
}
