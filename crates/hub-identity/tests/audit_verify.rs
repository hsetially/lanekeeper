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
use hub_identity::audit::{Column, ColumnSet};
use hub_identity::audit::{MemoryBucket, VerifyFailure};
use ports::{Leases, NotificationKind};
use support::audit_rig::{AuditRig, bare_event, full_event};
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
    // (b) Event, hash and the column that copies the changed field are rewritten together, so the table's check passes and so does
    // the row's own comparison with its columns. Only the row after it exposes the change.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed'), \
         action = 'role_changed', \
         hash = sha256(prev_hash || convert_to(replace(event_json, 'settings_changed', 'role_changed'), 'UTF8')) \
         WHERE seq = 3",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(4, FailureKind::ChainLinkBroken)]);
}

#[tokio::test]
async fn a_rewritten_event_whose_columns_were_left_alone_is_also_a_column_failure() {
    // (b) again, by someone who forgot the copy: the row's own columns now contradict its event, and the next row's link breaks.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed'), \
         hash = sha256(prev_hash || convert_to(replace(event_json, 'settings_changed', 'role_changed'), 'UTF8')) \
         WHERE seq = 3",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(
        report.failures,
        [
            failure(3, FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::Action]))),
            failure(4, FailureKind::ChainLinkBroken)
        ]
    );
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
         action = 'role_changed', \
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
    // The marker is bound, not part of the SQL: `sqlx` logs statement text, and the capture below would find it there.
    rig.admin_sql_bound(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', $1) WHERE seq = 5",
        "SECRET-EVENT-TEXT",
    )
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
    // The search can fail: the same capture finds the marker when something logs it, so the absence above means something.
    tracing::error!(event_text = "SECRET-EVENT-TEXT", "planted by the test");
    assert!(
        capture.text().contains("SECRET-EVENT-TEXT"),
        "the capture would not have shown a leak"
    );
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

// ---------------------------------------------------------------------------------------------------------------------------
// The denormalised columns (S9): they copy the event for filtering and listing, and the table's CHECK covers `event_json` only.

/// Every optional field present, on each kind of actor.
fn full_events_of_every_actor(n: usize) -> Vec<ports::AuditEvent> {
    use ports::AuditActor;
    let mut out = Vec::new();
    for i in 0..n {
        let mut e = full_event(i64::try_from(i).unwrap());
        e.actor = match i % 4 {
            0 => e.actor,
            1 => AuditActor::Agent {
                swimlane: ports::conformance::sample::swimlane("sit2"),
            },
            2 => AuditActor::Sentinel {
                name: domain::ShortText::parse("nfs-vm-1").unwrap(),
            },
            _ => AuditActor::System {
                component: domain::ShortText::parse("sync-job").unwrap(),
            },
        };
        out.push(e);
    }
    out
}

#[tokio::test]
async fn events_with_every_optional_field_present_verify_clean() {
    let rig = AuditRig::new(db!()).await;
    for e in full_events_of_every_actor(8) {
        rig.record_event(e).await;
    }
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, []);
    assert_eq!(report.events_checked, 8);
}

#[tokio::test]
async fn events_with_every_optional_field_absent_verify_clean() {
    let rig = AuditRig::new(db!()).await;
    for n in 0..5 {
        rig.record_event(bare_event(n)).await;
    }
    // An attribution with nothing in it is not the same as none, and must verify too.
    let mut e = bare_event(9);
    e.attribution = Some(domain::Attribution::unknown());
    rig.record_event(e).await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, []);
    assert_eq!(report.events_checked, 6);
}

#[tokio::test]
async fn a_mixed_chain_with_checkpoints_and_odd_timestamps_verifies_clean() {
    let rig = AuditRig::new(db!()).await;
    let job = rig.checkpoint_job();
    for (i, e) in full_events_of_every_actor(4).into_iter().enumerate() {
        rig.record_event(e).await;
        rig.record_event(bare_event(i64::try_from(i).unwrap())).await;
    }
    // The extremes of a millisecond timestamp the table can hold: a long way before the epoch, and a long way after it.
    for ms in [
        -210_866_803_200_000, // 4713 BC, the earliest `timestamptz`
        -62_135_596_800_000 + 86_400_000,
        -1_i64,
        0,
        1,
        253_402_300_799_999, // 9999-12-31, which a floating-point interval product is already off by microseconds for
        8_900_000_000_000_123,
    ] {
        let mut e = bare_event(0);
        e.at = domain::Timestamp::from_unix_millis(ms);
        rig.record_event(e).await;
    }
    job.run_once().await.unwrap();
    let outcome = rig.nightly().run_once().await.unwrap();
    assert!(matches!(outcome, NightlyOutcome::Clean(_)), "{outcome:?}");
}

/// One tamper of a denormalised column: `full` is the SQL value written into row 1 (every optional field present), `bare` the one
/// written into row 2 (no optional field), or empty to leave row 2 alone because the value would not change it.
struct Case {
    name: &'static str,
    column: Column,
    full: &'static str,
    bare: &'static str,
}

const fn case(name: &'static str, column: Column, full: &'static str, bare: &'static str) -> Case {
    Case {
        name,
        column,
        full,
        bare,
    }
}

const EVIL_ACTOR: &str = "'{\"kind\":\"os_login\",\"login\":\"someone-else\"}'::jsonb";

/// Every compared column, changed, and for the nullable ones also removed.
const CASES: &[Case] = &[
    case(
        "at",
        Column::At,
        "at + interval '1 second'",
        "at + interval '1 second'",
    ),
    case(
        "at by one microsecond",
        Column::At,
        "at + interval '1 microsecond'",
        "at - interval '1 microsecond'",
    ),
    case("actor_kind", Column::ActorKind, "'agent'", "'agent'"),
    case("actor_ref", Column::ActorRef, "'someone-else'", "'someone-else'"),
    case("actor_ref removed", Column::ActorRef, "NULL", "NULL"),
    case("action", Column::Action, "'role_changed'", "'role_changed'"),
    case("via", Column::Via, "'mcp'", "'mcp'"),
    case("swimlane_id", Column::SwimlaneId, "'sit9'", "'sit9'"),
    case("swimlane_id removed", Column::SwimlaneId, "NULL", ""),
    case("path", Column::Path, "'other/file.yml'", "'other/file.yml'"),
    case("path removed", Column::Path, "NULL", ""),
    case("hash_before", Column::HashBefore, "sha256('x')", "sha256('x')"),
    case("hash_before removed", Column::HashBefore, "NULL", ""),
    case("hash_after", Column::HashAfter, "sha256('x')", "sha256('x')"),
    case("hash_after removed", Column::HashAfter, "NULL", ""),
    case("github_login", Column::GithubLogin, "'mallory'", "'mallory'"),
    case("github_login removed", Column::GithubLogin, "NULL", ""),
    case("approval_id", Column::ApprovalId, "78", "78"),
    case("approval_id removed", Column::ApprovalId, "NULL", ""),
    case("request_id", Column::RequestId, "'req-forged'", "'req-forged'"),
    case("request_id removed", Column::RequestId, "NULL", ""),
    case(
        "attribution_source",
        Column::AttributionSource,
        "'tool_write'",
        "'tool_write'",
    ),
    case(
        "attribution_source removed",
        Column::AttributionSource,
        "NULL",
        "",
    ),
    case(
        "attribution_confidence",
        Column::AttributionConfidence,
        "'low'",
        "'low'",
    ),
    case(
        "attribution_confidence removed",
        Column::AttributionConfidence,
        "NULL",
        "",
    ),
    case(
        "attribution_actor",
        Column::AttributionActor,
        EVIL_ACTOR,
        EVIL_ACTOR,
    ),
    case("attribution_actor removed", Column::AttributionActor, "NULL", ""),
    case(
        "attribution_evidence",
        Column::AttributionEvidence,
        "'{}'::jsonb",
        "'{\"executable\":\"/bin/sh\"}'::jsonb",
    ),
    case(
        "attribution_evidence removed",
        Column::AttributionEvidence,
        "NULL",
        "",
    ),
];

#[test]
fn the_tamper_cases_cover_every_compared_column_changed_and_removed() {
    for column in Column::ALL {
        assert!(
            CASES.iter().any(|c| c.column == column),
            "no tamper case for {}",
            column.name()
        );
    }
    // The nullable columns are also tested with the value taken away.
    for column in Column::ALL {
        let nullable = !matches!(
            column,
            Column::At | Column::ActorKind | Column::Action | Column::Via
        );
        let removed = CASES
            .iter()
            .filter(|c| c.column == column && c.full == "NULL")
            .count();
        assert_eq!(removed > 0, nullable, "{}", column.name());
    }
}

/// Each case on a database of its own, with a checkpoint: the rows are clean before the tamper, and after it the only things that
/// changed are the columns, so they are the only things reported (and nothing else, the event, hash, links and checkpoints all
/// stay).
#[tokio::test]
async fn a_denormalised_column_changed_alone_is_detected_for_every_column() {
    for case in CASES {
        let rig = AuditRig::new(db!()).await;
        rig.record_event(full_event(0)).await;
        rig.record_event(bare_event(1)).await;
        rig.checkpoint_job().run_once().await.unwrap();
        let clean = rig.chain_verifier().verify().await.unwrap();
        assert_eq!(
            clean.failures,
            [],
            "{}: the rows are clean before the tamper",
            case.name
        );

        let column = case.column.name();
        rig.tamper_open(false).await;
        rig.admin_sql(&format!(
            "UPDATE audit_events SET {column} = {} WHERE seq = 1",
            case.full
        ))
        .await;
        let mut expected = vec![(1_i64, column)];
        if !case.bare.is_empty() {
            rig.admin_sql(&format!(
                "UPDATE audit_events SET {column} = {} WHERE seq = 2",
                case.bare
            ))
            .await;
            expected.push((2, column));
        }
        let report = rig.chain_verifier().verify().await.unwrap();
        assert_columns_differ(case.name, &report, &expected);
    }
}

/// The report lists exactly these rows, each with exactly these column names, and nothing else.
fn assert_columns_differ(what: &str, report: &hub_identity::audit::VerifyReport, expected: &[(i64, &str)]) {
    assert!(!report.is_clean(), "{what}: a changed column went unnoticed");
    // A failure of another kind has no column names, and is reported as `None`.
    let found: Vec<(i64, Option<Vec<&'static str>>)> = report
        .failures
        .iter()
        .map(|f| (f.seq, f.kind.columns().map(ColumnSet::names)))
        .collect();
    let expected: Vec<(i64, Option<Vec<&str>>)> =
        expected.iter().map(|(seq, c)| (*seq, Some(vec![*c]))).collect();
    assert_eq!(found, expected, "{what}");
    assert!(!report.truncated, "{what}");
}

#[tokio::test]
async fn a_column_difference_alerts_through_the_metric_and_notifier_and_names_columns_only() {
    let (capture, _guard) = log_capture::install();
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    // The values are bound, not part of the SQL, for the reason `admin_sql_bound` gives.
    rig.admin_sql_bound(
        "UPDATE audit_events SET actor_ref = $1, github_login = $1 WHERE seq = 5",
        "SECRET-FORGED-VALUE",
    )
    .await;
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(
        report.failures,
        [failure(
            5,
            FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::ActorRef, Column::GithubLogin]))
        )]
    );

    // The same metric and the same notifier as the other mismatches.
    assert_eq!(rig.metrics.audit_verify_failures_total(), 1);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 1);
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert_eq!(sent[0].severity, Severity::Critical);
    let summary = sent[0].summary.as_str();
    assert!(summary.contains("position 5"), "{summary}");
    assert!(
        summary.contains("actor_ref") && summary.contains("github_login"),
        "{summary}"
    );

    // Names, never values (S10): not in the alert, not in the log, not in the report.
    let log = capture.text();
    assert!(
        log.contains("audit chain verification failed") && log.contains("seq=5") && log.contains("actor_ref"),
        "{log}"
    );
    assert!(
        !log.contains("SECRET-FORGED-VALUE"),
        "a column value reached the log"
    );
    assert!(!format!("{sent:?}{report:?}").contains("SECRET-FORGED-VALUE"));
    tracing::error!(value = "SECRET-FORGED-VALUE", "planted by the test");
    assert!(
        capture.text().contains("SECRET-FORGED-VALUE"),
        "the capture would not have shown a leak"
    );
}

#[tokio::test]
async fn every_row_with_different_columns_is_listed_and_the_walk_goes_on_across_batches() {
    let rig = AuditRig::new(db!()).await;
    rig.record(25).await;
    rig.tamper_open(false).await;
    rig.admin_sql("UPDATE audit_events SET via = 'mcp' WHERE seq IN (3, 14, 25)")
        .await;
    let report = rig.chain_verifier().with_batch_rows(10).verify().await.unwrap();
    let via = ColumnSet::of(&[Column::Via]);
    assert_eq!(
        report.failures,
        [
            failure(3, FailureKind::ColumnsDiffer(via)),
            failure(14, FailureKind::ColumnsDiffer(via)),
            failure(25, FailureKind::ColumnsDiffer(via)),
        ]
    );
    assert_eq!(
        report.events_checked, 25,
        "the chain itself is intact, so the whole of it is read"
    );
    assert_eq!(report.batches, 3);
}

#[tokio::test]
async fn columns_after_the_first_broken_link_are_not_looked_at() {
    // The walk stops at the first bad link, so a later row's columns are not looked at (they cannot be trusted to line up).
    let rig = AuditRig::new(db!()).await;
    rig.record(8).await;
    rig.tamper_open(false).await;
    rig.admin_sql("DELETE FROM audit_events WHERE seq = 4").await;
    rig.admin_sql("UPDATE audit_events SET via = 'mcp' WHERE seq IN (2, 7)")
        .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(
        report.failures,
        [
            failure(2, FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::Via]))),
            failure(5, FailureKind::ChainLinkBroken)
        ]
    );
}
