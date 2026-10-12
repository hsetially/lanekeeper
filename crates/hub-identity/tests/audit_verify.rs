//! T5 (S9): the nightly verification finds tampering, and alerts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::Severity;
use hub_identity::audit::bucket::{CheckpointBucket, object_name};
use hub_identity::audit::checkpoint::CheckpointObject;
use hub_identity::audit::verify::{
    CheckpointColumn, CheckpointColumnSet, FailureKind, NightlyOutcome, VerifyError, chain_batch_sql,
    checkpoint_batch_sql,
};
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
    /// Statements (separated by `;`) run first, as the owner, when the value needs a constraint or an index out of the way.
    prep: &'static str,
    /// A value that something reading the column could choke on (a cast, a decode, a length), and not just one that differs.
    hostile: bool,
}

const fn case(name: &'static str, column: Column, full: &'static str, bare: &'static str) -> Case {
    Case {
        name,
        column,
        full,
        bare,
        prep: "",
        hostile: false,
    }
}

/// A value the reader of the column could fail on: it must still be a finding about that column, and never an error that ends
/// the walk.
const fn hostile(
    name: &'static str,
    column: Column,
    prep: &'static str,
    full: &'static str,
    bare: &'static str,
) -> Case {
    Case {
        name,
        column,
        full,
        bare,
        prep,
        hostile: true,
    }
}

const NO_CHECK_ACTOR_KIND: &str = "ALTER TABLE audit_events DROP CONSTRAINT audit_events_actor_kind_check";
const NO_CHECK_ACTION: &str = "ALTER TABLE audit_events DROP CONSTRAINT audit_events_action_check";
const NO_CHECK_VIA: &str = "ALTER TABLE audit_events DROP CONSTRAINT audit_events_via_check";
const NO_CHECK_SOURCE: &str =
    "ALTER TABLE audit_events DROP CONSTRAINT audit_events_attribution_source_check";
const NO_CHECK_CONFIDENCE: &str =
    "ALTER TABLE audit_events DROP CONSTRAINT audit_events_attribution_confidence_check";
const NO_HASH_LENGTH: &str = "ALTER DOMAIN sha256_hash DROP CONSTRAINT sha256_hash_length";
/// A megabyte of text, for the columns that no index covers.
const MEGABYTE: &str = "repeat('x', 1000000)";
/// A hundred thousand characters: it compresses to a size the indexes on the other text columns still take.
const LONG: &str = "repeat('x', 100000)";
/// A number `serde_json` cannot read (a `jsonb` holds it, being a `numeric`).
const HUGE_NUMBER: &str = "'{\"n\":1e400}'::jsonb";
/// An array nested deeper than `serde_json` reads (its limit is 128).
const DEEP_ARRAY: &str = "(repeat('[', 200) || repeat(']', 200))::jsonb";

/// The indexes that a value past what a btree entry holds cannot be written under.
const DROP_ACTOR_IDX: &str = "DROP INDEX audit_events_actor_idx";
const DROP_SWIMLANE_IDXS: &str = "DROP INDEX audit_events_swimlane_idx;DROP INDEX audit_events_path_idx";
const DROP_PATH_IDX: &str = "DROP INDEX audit_events_path_idx";
/// Two megabytes of text: more than the megabyte an event can hold, and so more than any copy of a part of it.
const OVERSIZE_TEXT: &str = "repeat('x', 2097152)";
/// A `jsonb` whose text form is over twice the megabyte an event can hold.
const OVERSIZE_JSON: &str = "('\"' || repeat('x', 2097152) || '\"')::jsonb";
/// 33 bytes: one more than a hash.
const OVERSIZE_HASH: &str = "decode(repeat('ab', 33), 'hex')";

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
    // Values a reader of the column could fail on. Each must be reported as that column, alone.
    hostile("at infinity", Column::At, "", "'infinity'", "'infinity'"),
    hostile("at -infinity", Column::At, "", "'-infinity'", "'-infinity'"),
    hostile(
        "at beyond the microseconds a bigint holds (294276-01-01)",
        Column::At,
        "",
        "'294276-01-01 00:00:00+00'::timestamptz",
        "'294276-01-01 00:00:00+00'::timestamptz",
    ),
    hostile(
        "at before the epoch by the most a timestamptz holds",
        Column::At,
        "",
        "'4713-01-01 00:00:00+00 BC'::timestamptz",
        "'4713-01-01 00:00:00+00 BC'::timestamptz",
    ),
    hostile(
        "actor_kind of a kind there is none of",
        Column::ActorKind,
        NO_CHECK_ACTOR_KIND,
        "'robot'",
        "'robot'",
    ),
    hostile(
        "actor_ref of a hundred thousand characters",
        Column::ActorRef,
        "",
        LONG,
        LONG,
    ),
    hostile(
        "action of a hundred thousand characters",
        Column::Action,
        NO_CHECK_ACTION,
        "repeat('a', 100000)",
        "repeat('a', 100000)",
    ),
    hostile(
        "via of a route there is none of",
        Column::Via,
        NO_CHECK_VIA,
        "'teleport'",
        "'teleport'",
    ),
    hostile(
        "swimlane_id of a hundred thousand characters",
        Column::SwimlaneId,
        "",
        LONG,
        LONG,
    ),
    hostile(
        "path of a hundred thousand characters",
        Column::Path,
        "",
        LONG,
        LONG,
    ),
    hostile(
        "hash_before of one byte",
        Column::HashBefore,
        NO_HASH_LENGTH,
        "'\\x00'::bytea",
        "'\\x00'::bytea",
    ),
    hostile(
        "hash_before of a megabyte",
        Column::HashBefore,
        NO_HASH_LENGTH,
        "decode(repeat('ab', 500000), 'hex')",
        "decode(repeat('ab', 500000), 'hex')",
    ),
    hostile(
        "hash_after of one byte",
        Column::HashAfter,
        NO_HASH_LENGTH,
        "'\\x00'::bytea",
        "'\\x00'::bytea",
    ),
    hostile(
        "github_login of a megabyte",
        Column::GithubLogin,
        "",
        MEGABYTE,
        MEGABYTE,
    ),
    hostile(
        "approval_id of the largest bigint",
        Column::ApprovalId,
        "",
        "9223372036854775807",
        "9223372036854775807",
    ),
    hostile(
        "approval_id of the smallest bigint",
        Column::ApprovalId,
        "",
        "-9223372036854775808",
        "-9223372036854775808",
    ),
    hostile(
        "request_id of a megabyte",
        Column::RequestId,
        "",
        MEGABYTE,
        MEGABYTE,
    ),
    hostile(
        "attribution_source of a source there is none of",
        Column::AttributionSource,
        NO_CHECK_SOURCE,
        "'wizard'",
        "'wizard'",
    ),
    hostile(
        "attribution_confidence of a level there is none of",
        Column::AttributionConfidence,
        NO_CHECK_CONFIDENCE,
        "'omniscient'",
        "'omniscient'",
    ),
    hostile(
        "attribution_actor holding a number serde_json cannot read",
        Column::AttributionActor,
        "",
        HUGE_NUMBER,
        HUGE_NUMBER,
    ),
    hostile(
        "attribution_actor nested deeper than serde_json reads",
        Column::AttributionActor,
        "",
        DEEP_ARRAY,
        DEEP_ARRAY,
    ),
    hostile(
        "attribution_evidence holding a number serde_json cannot read",
        Column::AttributionEvidence,
        "",
        HUGE_NUMBER,
        HUGE_NUMBER,
    ),
    hostile(
        "attribution_evidence nested deeper than serde_json reads",
        Column::AttributionEvidence,
        "",
        DEEP_ARRAY,
        DEEP_ARRAY,
    ),
    // Values past what any event can have put there (round 3): the select must not send them, and the column is reported.
    hostile(
        "actor_kind oversize",
        Column::ActorKind,
        NO_CHECK_ACTOR_KIND,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "actor_ref oversize",
        Column::ActorRef,
        DROP_ACTOR_IDX,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "action oversize",
        Column::Action,
        NO_CHECK_ACTION,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "via oversize",
        Column::Via,
        NO_CHECK_VIA,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "swimlane_id oversize",
        Column::SwimlaneId,
        DROP_SWIMLANE_IDXS,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "path oversize",
        Column::Path,
        DROP_PATH_IDX,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "hash_before oversize",
        Column::HashBefore,
        NO_HASH_LENGTH,
        OVERSIZE_HASH,
        OVERSIZE_HASH,
    ),
    hostile(
        "hash_after oversize",
        Column::HashAfter,
        NO_HASH_LENGTH,
        OVERSIZE_HASH,
        OVERSIZE_HASH,
    ),
    hostile(
        "github_login oversize",
        Column::GithubLogin,
        "",
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "request_id oversize",
        Column::RequestId,
        "",
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "attribution_source oversize",
        Column::AttributionSource,
        NO_CHECK_SOURCE,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "attribution_confidence oversize",
        Column::AttributionConfidence,
        NO_CHECK_CONFIDENCE,
        OVERSIZE_TEXT,
        OVERSIZE_TEXT,
    ),
    hostile(
        "attribution_actor oversize",
        Column::AttributionActor,
        "",
        OVERSIZE_JSON,
        OVERSIZE_JSON,
    ),
    hostile(
        "attribution_evidence oversize",
        Column::AttributionEvidence,
        "",
        OVERSIZE_JSON,
        OVERSIZE_JSON,
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
    // Every column also has a value that something reading it could fail on, so that no column is judged only on tidy values.
    for column in Column::ALL {
        assert!(
            CASES.iter().any(|c| c.column == column && c.hostile),
            "no hostile-value case for {}",
            column.name()
        );
    }
    // Every column that can hold more than a part of an event can hold has a value past that bound. Only the two of a fixed size
    // (`at`, `approval_id`) cannot.
    for column in Column::ALL {
        let fixed = matches!(column, Column::At | Column::ApprovalId);
        let oversize = CASES
            .iter()
            .any(|c| c.column == column && c.hostile && c.name.contains("oversize"));
        assert_eq!(oversize, !fixed, "{}", column.name());
    }
    // The three times a `timestamptz` holds and a bigint of microseconds does not.
    for needle in ["at infinity", "at -infinity", "294276-01-01"] {
        assert!(
            CASES.iter().any(|c| c.name.contains(needle)),
            "no case for {needle}"
        );
    }
    // The jsonb columns: the two ways a stored value cannot be read.
    for column in [Column::AttributionActor, Column::AttributionEvidence] {
        for what in ["cannot read", "nested deeper"] {
            assert!(
                CASES
                    .iter()
                    .any(|c| c.column == column && c.hostile && c.name.contains(what)),
                "no {what} case for {}",
                column.name()
            );
        }
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
        for statement in case.prep.split(';').filter(|s| !s.is_empty()) {
            rig.admin_sql(statement).await;
        }
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

// ---------------------------------------------------------------------------------------------------------------------------
// A value in a copied column that cannot be read must not end the walk (round 2).

/// 12 events with checkpoints at 6 and 12. The copied column of seq 2 is set by `column_sql` (an `UPDATE ... SET` list), the stored
/// event of seq 5 is rewritten without its hash being kept, and seq 12 is rewritten consistently, so that the checkpoint at 12 signed
/// another head. Run through the nightly job, as in production.
async fn a_later_tamper_is_found_after(column_sql: &str, differ: Column) {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    rig.admin_sql(&format!("UPDATE audit_events SET {column_sql} WHERE seq = 2"))
        .await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 5",
    )
    .await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed'), \
         action = 'role_changed', \
         hash = sha256(prev_hash || convert_to(replace(event_json, 'settings_changed', 'role_changed'), 'UTF8')) \
         WHERE seq = 12",
    )
    .await;
    let outcome = rig.nightly().run_once().await.unwrap();
    assert!(
        matches!(outcome, NightlyOutcome::Failed(_)),
        "{column_sql}: expected a failed run, got {outcome:?}"
    );
    let NightlyOutcome::Failed(report) = outcome else {
        return;
    };
    assert_eq!(
        report.failures,
        [
            failure(2, FailureKind::ColumnsDiffer(ColumnSet::of(&[differ]))),
            failure(5, FailureKind::ChainHashMismatch),
            failure(12, FailureKind::CheckpointHeadMismatch),
        ],
        "{column_sql}"
    );
    assert_eq!(rig.metrics.audit_verify_failures_total(), 3);
    assert_eq!(
        rig.metrics.audit_verify_errors_total(),
        0,
        "this is a finding, not an error"
    );
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1, "{column_sql}: the admins are told once");
    assert_eq!(sent[0].severity, Severity::Critical);
    let summary = sent[0].summary.as_str();
    assert!(
        summary.contains("3 problem(s)") && summary.contains("event hash mismatch at position 5"),
        "{summary}"
    );
}

#[tokio::test]
async fn an_infinite_time_is_a_finding_and_a_later_tamper_is_still_found() {
    // The reviewer's scenario: a verifier that fails on `at = 'infinity'` ends the walk at seq 2 and never looks at seq 5 or at
    // the checkpoints.
    a_later_tamper_is_found_after("at = 'infinity'", Column::At).await;
}

#[tokio::test]
async fn a_negative_infinite_time_is_a_finding_and_a_later_tamper_is_still_found() {
    a_later_tamper_is_found_after("at = '-infinity'", Column::At).await;
}

#[tokio::test]
async fn a_time_beyond_the_microseconds_of_a_bigint_is_a_finding_and_a_later_tamper_is_still_found() {
    a_later_tamper_is_found_after("at = '294276-01-01 00:00:00+00'", Column::At).await;
}

#[tokio::test]
async fn a_jsonb_column_that_cannot_be_read_is_a_finding_and_a_later_tamper_is_still_found() {
    a_later_tamper_is_found_after(
        "attribution_evidence = '{\"n\":1e400}'::jsonb",
        Column::AttributionEvidence,
    )
    .await;
    a_later_tamper_is_found_after(
        "attribution_actor = (repeat('[', 200) || repeat(']', 200))::jsonb",
        Column::AttributionActor,
    )
    .await;
}

#[tokio::test]
async fn a_row_with_a_null_in_a_chain_column_is_a_hash_failure_and_not_an_error() {
    // The columns are NOT NULL; an owner can drop that. The table's own check passes on NULL, so nothing else refuses it.
    for column in ["event_json", "hash", "prev_hash"] {
        let rig = rig_with_checkpoints(db!()).await;
        rig.tamper_open(false).await;
        rig.admin_sql(&format!(
            "ALTER TABLE audit_events ALTER COLUMN {column} DROP NOT NULL"
        ))
        .await;
        rig.admin_sql(&format!("UPDATE audit_events SET {column} = NULL WHERE seq = 3"))
            .await;
        let outcome = rig.nightly().run_once().await.unwrap();
        let NightlyOutcome::Failed(report) = outcome else {
            panic!("{column}: expected a failed run, got {outcome:?}");
        };
        assert_eq!(
            report.failures.first(),
            Some(&failure(3, FailureKind::ChainHashMismatch)),
            "{column}"
        );
        assert_eq!(rig.metrics.audit_verify_errors_total(), 0, "{column}");
        assert_eq!(rig.notifier.sent().len(), 1, "{column}");
    }
}

#[tokio::test]
async fn a_database_error_is_still_an_error_and_not_a_finding() {
    // The line between "the stored value cannot be read" (a finding) and "the database cannot be read" (an error): a query the
    // server refuses is the second. Here a column the select list names is gone.
    let rig = rig_with_checkpoints(db!()).await;
    rig.admin_sql("ALTER TABLE audit_events RENAME COLUMN via TO via_renamed")
        .await;
    assert!(rig.chain_verifier().verify().await.is_err());
    assert_eq!(rig.nightly().run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 0);
}

#[tokio::test]
async fn a_hundred_column_failures_do_not_crowd_out_a_broken_chain_or_a_checkpoint() {
    let rig = AuditRig::new(db!()).await;
    rig.record(110).await;
    rig.checkpoint_job().run_once().await.unwrap();
    rig.tamper_open(true).await;
    // 104 rows whose copies differ, then the stored event of seq 105 changed: the walk ends there.
    rig.admin_sql("UPDATE audit_events SET via = 'mcp' WHERE seq <= 104")
        .await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 105",
    )
    .await;
    // A verifier that trusts another key: the checkpoint at 110 fails its signature.
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
    let columns = report
        .failures
        .iter()
        .filter(|f| f.kind.columns().is_some())
        .count();
    assert_eq!(columns, 100, "the column failures are bounded");
    assert!(report.truncated, "and the rest are counted as left out");
    assert_eq!(
        report.failures.iter().rev().take(2).copied().collect::<Vec<_>>(),
        [
            failure(110, FailureKind::SignatureInvalid),
            failure(105, FailureKind::ChainHashMismatch)
        ],
        "the broken chain and the checkpoint are listed, though a hundred column failures came first"
    );
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3, B1: a checkpoint column that cannot be used is a finding about that checkpoint, never an error that discards the rest.

const TAMPER_5: &str = "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 5";

/// 12 events with checkpoints at 6 and 12, and the stored event of seq 5 rewritten without its hash. The triggers of both audit
/// tables, and the table's own hash check, are off, as they are for an owner who means to.
async fn rig_with_a_tamper_at_5(db: support::pg::TestDb) -> AuditRig {
    let rig = rig_with_checkpoints(db).await;
    rig.tamper_open(true).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql(TAMPER_5).await;
    rig
}

/// The nightly run is `Failed` with exactly these failures in this order, is not an error, and the admins are told once.
async fn assert_failed_with(
    rig: &AuditRig,
    what: &str,
    expected: &[VerifyFailure],
) -> hub_identity::audit::VerifyReport {
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("{what}: expected a failed run, got {outcome:?}");
    };
    assert_eq!(report.failures, expected, "{what}");
    assert_eq!(
        rig.metrics.audit_verify_errors_total(),
        0,
        "{what}: a finding, not an error"
    );
    assert_eq!(
        rig.metrics.audit_verify_failures_total(),
        u64::try_from(expected.len()).unwrap(),
        "{what}"
    );
    assert_eq!(rig.notifier.sent().len(), 1, "{what}: the admins are told once");
    report
}

struct CpCase {
    name: &'static str,
    column: CheckpointColumn,
    /// Run first, as the owner.
    prep: &'static str,
    /// The SQL value written into the checkpoint at 12.
    value: &'static str,
    /// What the checkpoint is reported as; `None` is `CheckpointColumnsInvalid` naming the column.
    kind: Option<FailureKind>,
}

const DROP_NOT_NULL_HEAD: &str = "ALTER TABLE audit_checkpoints ALTER COLUMN head_hash DROP NOT NULL";
const DROP_NOT_NULL_VERSION: &str =
    "ALTER TABLE audit_checkpoints ALTER COLUMN kms_key_version DROP NOT NULL";
const DROP_NOT_NULL_SIGNATURE: &str = "ALTER TABLE audit_checkpoints ALTER COLUMN signature DROP NOT NULL";
const DROP_NOT_NULL_OBJECT: &str = "ALTER TABLE audit_checkpoints ALTER COLUMN gcs_object DROP NOT NULL";

const CP_CASES: &[CpCase] = &[
    CpCase {
        name: "head_hash NULL",
        column: CheckpointColumn::HeadHash,
        prep: DROP_NOT_NULL_HEAD,
        value: "NULL",
        kind: None,
    },
    CpCase {
        name: "head_hash oversize (33 bytes)",
        column: CheckpointColumn::HeadHash,
        prep: NO_HASH_LENGTH,
        value: "decode(repeat('ab', 33), 'hex')",
        kind: None,
    },
    CpCase {
        name: "head_hash of two megabytes",
        column: CheckpointColumn::HeadHash,
        prep: NO_HASH_LENGTH,
        value: "decode(repeat('ab', 2000000), 'hex')",
        kind: None,
    },
    CpCase {
        name: "head_hash of one byte",
        column: CheckpointColumn::HeadHash,
        prep: NO_HASH_LENGTH,
        value: "'\\x00'::bytea",
        kind: Some(FailureKind::CheckpointHeadMismatch),
    },
    CpCase {
        name: "kms_key_version NULL",
        column: CheckpointColumn::KmsKeyVersion,
        prep: DROP_NOT_NULL_VERSION,
        value: "NULL",
        kind: None,
    },
    CpCase {
        name: "kms_key_version oversize (513 characters)",
        column: CheckpointColumn::KmsKeyVersion,
        prep: "",
        value: "repeat('k', 513)",
        kind: None,
    },
    CpCase {
        name: "kms_key_version of four megabytes",
        column: CheckpointColumn::KmsKeyVersion,
        prep: "",
        value: "repeat('k', 4000000)",
        kind: None,
    },
    CpCase {
        name: "kms_key_version empty",
        column: CheckpointColumn::KmsKeyVersion,
        prep: "",
        value: "''",
        kind: Some(FailureKind::BucketObjectMismatch),
    },
    CpCase {
        name: "signature NULL",
        column: CheckpointColumn::Signature,
        prep: DROP_NOT_NULL_SIGNATURE,
        value: "NULL",
        kind: None,
    },
    CpCase {
        name: "signature oversize (129 bytes)",
        column: CheckpointColumn::Signature,
        prep: "",
        value: "decode(repeat('ab', 129), 'hex')",
        kind: None,
    },
    CpCase {
        name: "signature of two megabytes",
        column: CheckpointColumn::Signature,
        prep: "",
        value: "decode(repeat('ab', 2000000), 'hex')",
        kind: None,
    },
    CpCase {
        name: "signature empty",
        column: CheckpointColumn::Signature,
        prep: "",
        value: "'\\x'::bytea",
        kind: Some(FailureKind::BucketObjectMismatch),
    },
    CpCase {
        name: "gcs_object NULL",
        column: CheckpointColumn::GcsObject,
        prep: DROP_NOT_NULL_OBJECT,
        value: "NULL",
        kind: None,
    },
    CpCase {
        name: "gcs_object oversize (129 characters)",
        column: CheckpointColumn::GcsObject,
        prep: "",
        value: "repeat('o', 129)",
        kind: None,
    },
    CpCase {
        name: "gcs_object of four megabytes",
        column: CheckpointColumn::GcsObject,
        prep: "",
        value: "repeat('o', 4000000)",
        kind: None,
    },
    CpCase {
        name: "gcs_object of another position",
        column: CheckpointColumn::GcsObject,
        prep: "",
        value: "'audit/checkpoints/13.json'",
        kind: Some(FailureKind::CheckpointNameInvalid),
    },
];

#[test]
fn the_checkpoint_cases_cover_every_column_removed_and_oversize() {
    for column in CheckpointColumn::ALL {
        for what in ["NULL", "oversize"] {
            assert!(
                CP_CASES
                    .iter()
                    .any(|c| c.column == column && c.kind.is_none() && c.name.contains(what)),
                "no {what} case for {}",
                column.name()
            );
        }
    }
}

#[tokio::test]
async fn a_checkpoint_column_that_cannot_be_used_is_a_finding_and_a_tamper_elsewhere_is_still_found() {
    // The reviewer's scenario for every column: the checkpoint at 12 is unusable and the event at 5 was rewritten. A verifier that
    // lets the first end the run never reports the second, night after night.
    for case in CP_CASES {
        let rig = rig_with_a_tamper_at_5(db!()).await;
        for statement in case.prep.split(';').filter(|s| !s.is_empty()) {
            rig.admin_sql(statement).await;
        }
        rig.admin_sql(&format!(
            "UPDATE audit_checkpoints SET {} = {} WHERE seq = 12",
            case.column.name(),
            case.value
        ))
        .await;
        let kind = case
            .kind
            .unwrap_or(FailureKind::CheckpointColumnsInvalid(CheckpointColumnSet::of(&[
                case.column,
            ])));
        assert_failed_with(
            &rig,
            case.name,
            &[failure(5, FailureKind::ChainHashMismatch), failure(12, kind)],
        )
        .await;
    }
}

#[tokio::test]
async fn a_bad_checkpoint_column_alerts_with_the_column_name_and_never_the_value() {
    let (capture, _guard) = log_capture::install();
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql_bound(
        "UPDATE audit_checkpoints SET gcs_object = $1 || repeat('o', 200) WHERE seq = 12",
        "SECRET-OBJECT-NAME",
    )
    .await;
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(
        report.failures,
        [failure(
            12,
            FailureKind::CheckpointColumnsInvalid(CheckpointColumnSet::of(&[CheckpointColumn::GcsObject]))
        )]
    );
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1);
    let summary = sent[0].summary.as_str();
    assert!(
        summary.contains("position 12") && summary.contains("gcs_object"),
        "{summary}"
    );
    let log = capture.text();
    assert!(log.contains("seq=12") && log.contains("gcs_object"), "{log}");
    assert!(!log.contains("SECRET-OBJECT-NAME"), "a value reached the log");
    assert!(!format!("{sent:?}{report:?}").contains("SECRET-OBJECT-NAME"));
    tracing::error!(value = "SECRET-OBJECT-NAME", "planted by the test");
    assert!(capture.text().contains("SECRET-OBJECT-NAME"));
}

#[tokio::test]
async fn a_checkpoint_at_a_position_no_event_has_is_a_finding_for_any_seq() {
    // The INSERT-only app role may write a checkpoint row with any seq. None of these is the checkpoint of a head.
    for seq in [-5_i64, 0, i64::MIN, i64::MAX] {
        let rig = rig_with_a_tamper_at_5(db!()).await;
        rig.admin_sql(&format!(
            "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
             VALUES ({seq}, sha256('forged'), 'v', '\\x3006010203040506', 'audit/checkpoints/9.json')"
        ))
        .await;
        assert_failed_with(
            &rig,
            &format!("checkpoint at {seq}"),
            &[
                failure(5, FailureKind::ChainHashMismatch),
                failure(seq, FailureKind::CheckpointHeadMissing),
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn a_checkpoint_of_a_forged_event_at_position_0_is_still_a_finding() {
    // The forged event at 0 and a checkpoint row that names it, with the right hash and the right object name for position 0.
    // Nothing is wrong with the checkpoint but its position: no head is ever signed there.
    let rig = rig_with_checkpoints(db!()).await;
    insert_forged_event(&rig, 0).await;
    rig.admin_sql(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         SELECT 0, hash, 'v', '\\x3006010203040506', 'audit/checkpoints/0.json' FROM audit_events WHERE seq = 0",
    )
    .await;
    assert_failed_with(
        &rig,
        "checkpoint at 0",
        &[
            failure(0, FailureKind::SeqNotPositive),
            failure(0, FailureKind::CheckpointHeadMissing),
        ],
    )
    .await;
}

#[tokio::test]
async fn a_checkpoint_created_at_that_is_null_or_infinite_changes_nothing() {
    // `created_at` is not read by the verifier (the bucket's object holds its own copy and it is not compared), so no value in it
    // can end the run or hide the tamper at 5. A characterisation, true before the round and after it.
    for (name, prep, value) in [
        (
            "NULL",
            "ALTER TABLE audit_checkpoints ALTER COLUMN created_at DROP NOT NULL",
            "NULL",
        ),
        ("infinity", "", "'infinity'"),
        ("-infinity", "", "'-infinity'"),
    ] {
        let rig = rig_with_a_tamper_at_5(db!()).await;
        if !prep.is_empty() {
            rig.admin_sql(prep).await;
        }
        rig.admin_sql(&format!(
            "UPDATE audit_checkpoints SET created_at = {value} WHERE seq = 12"
        ))
        .await;
        assert_failed_with(
            &rig,
            &format!("created_at {name}"),
            &[failure(5, FailureKind::ChainHashMismatch)],
        )
        .await;
    }
}

#[tokio::test]
async fn a_checkpoint_query_that_fails_still_reports_what_the_chain_walk_found() {
    // A real query-level fault in the second half (a column of `audit_checkpoints` the select names is gone). The findings of the
    // first half are not thrown away with it: they come back with the error, and the admins are told.
    let rig = rig_with_a_tamper_at_5(db!()).await;
    rig.admin_sql("ALTER TABLE audit_checkpoints RENAME COLUMN gcs_object TO gcs_object_renamed")
        .await;
    let aborted = rig.chain_verifier().verify().await.unwrap_err();
    assert_eq!(aborted.error, VerifyError::Database);
    assert_eq!(
        aborted.partial.failures,
        [failure(5, FailureKind::ChainHashMismatch)]
    );
    assert_eq!(aborted.partial.events_checked, 5);

    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Incomplete(report) = outcome else {
        panic!("expected an incomplete run, got {outcome:?}");
    };
    assert_eq!(report.failures, [failure(5, FailureKind::ChainHashMismatch)]);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1, "it is also an error");
    assert_eq!(
        rig.metrics.audit_verify_failures_total(),
        1,
        "and the finding is counted"
    );
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1, "the admins are told of the finding");
    let summary = sent[0].summary.as_str();
    assert!(
        summary.contains("position 5") && summary.contains("did not finish"),
        "{summary}"
    );
}

#[tokio::test]
async fn a_chain_query_that_fails_does_not_stop_the_checkpoints_being_checked() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    // The checkpoint at 12 now signs a head the chain does not have ...
    rig.admin_sql("UPDATE audit_checkpoints SET head_hash = sha256('other') WHERE seq = 12")
        .await;
    // ... and the chain walk cannot run at all.
    rig.admin_sql("ALTER TABLE audit_events RENAME COLUMN via TO via_renamed")
        .await;
    let aborted = rig.chain_verifier().verify().await.unwrap_err();
    assert_eq!(aborted.error, VerifyError::Database);
    assert_eq!(
        aborted.partial.failures,
        [failure(12, FailureKind::CheckpointHeadMismatch)]
    );
    assert_eq!(aborted.partial.checkpoints_checked, 2);
}

#[tokio::test]
async fn a_run_that_found_nothing_before_it_failed_is_plainly_errored() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.admin_sql("ALTER TABLE audit_checkpoints RENAME COLUMN gcs_object TO gcs_object_renamed")
        .await;
    assert_eq!(rig.nightly().run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert_eq!(rig.metrics.audit_verify_failures_total(), 0);
    assert!(rig.notifier.sent().is_empty());
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3, N1: a position that is not positive, or missing, cannot hide a row.

/// A row the INSERT-only app role can write with an explicit `seq`: a valid hash, a `prev_hash` that links to nothing, and an
/// attribution that is a forgery.
async fn insert_forged_event(rig: &AuditRig, seq: i64) {
    sqlx::query(
        "WITH p AS (SELECT sha256(convert_to('forged-prev-' || $1::bigint::text, 'UTF8')) AS prev) \
         INSERT INTO audit_events (seq, prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via) \
         SELECT $1::bigint, prev, sha256(prev || convert_to('{\"forged\":1}', 'UTF8')), '{\"forged\":1}', now(), \
                'system', 'forged', 'settings_changed', 'system' FROM p",
    )
    .bind(seq)
    .execute(&rig.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn an_event_at_a_position_that_is_not_positive_is_a_finding_and_the_chain_is_still_whole() {
    for seq in [-5_i64, 0, i64::MIN] {
        let rig = rig_with_checkpoints(db!()).await;
        insert_forged_event(&rig, seq).await;
        let report = assert_failed_with(
            &rig,
            &format!("event at {seq}"),
            &[failure(seq, FailureKind::SeqNotPositive)],
        )
        .await;
        // The real chain, from the genesis hash, is read and found good: the forged row is skipped and does not take the place of
        // the first.
        assert_eq!(report.events_checked, 12, "event at {seq}");
        assert_eq!(report.checkpoints_checked, 2, "event at {seq}");
    }
}

#[tokio::test]
async fn an_event_at_a_position_that_is_not_positive_does_not_hide_a_later_tamper() {
    for seq in [-5_i64, 0, i64::MIN] {
        let rig = rig_with_checkpoints(db!()).await;
        insert_forged_event(&rig, seq).await;
        rig.tamper_open(true).await;
        rig.admin_sql(TAMPER_5).await;
        assert_failed_with(
            &rig,
            &format!("event at {seq}"),
            &[
                failure(seq, FailureKind::SeqNotPositive),
                failure(5, FailureKind::ChainHashMismatch),
            ],
        )
        .await;
    }
}

const DROP_EVENTS_KEY: [&str; 2] = [
    "ALTER TABLE audit_events DROP CONSTRAINT audit_events_pkey",
    "ALTER TABLE audit_events ALTER COLUMN seq DROP NOT NULL",
];
const DROP_CHECKPOINTS_KEY: [&str; 2] = [
    "ALTER TABLE audit_checkpoints DROP CONSTRAINT audit_checkpoints_pkey",
    "ALTER TABLE audit_checkpoints ALTER COLUMN seq DROP NOT NULL",
];

#[tokio::test]
async fn an_event_whose_seq_was_removed_is_a_finding_and_breaks_the_next_link() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    for statement in DROP_EVENTS_KEY {
        rig.admin_sql(statement).await;
    }
    rig.admin_sql("UPDATE audit_events SET seq = NULL WHERE seq = 3")
        .await;
    assert_failed_with(
        &rig,
        "seq NULL",
        &[
            failure(0, FailureKind::EventPositionMissing),
            failure(4, FailureKind::ChainLinkBroken),
        ],
    )
    .await;
}

#[tokio::test]
async fn a_forged_event_with_no_seq_at_the_tail_is_a_finding() {
    // Nothing after it breaks: it is chained to the head, valid, and has no position. Only a look for such rows finds it.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    for statement in DROP_EVENTS_KEY {
        rig.admin_sql(statement).await;
    }
    rig.admin_sql(
        "INSERT INTO audit_events (seq, prev_hash, hash, event_json, at, actor_kind, actor_ref, action, via) \
         SELECT NULL, hash, sha256(hash || convert_to('{\"forged\":1}', 'UTF8')), '{\"forged\":1}', now(), \
                'system', 'forged', 'settings_changed', 'system' FROM audit_events WHERE seq = 12",
    )
    .await;
    let report = assert_failed_with(
        &rig,
        "forged tail",
        &[failure(0, FailureKind::EventPositionMissing)],
    )
    .await;
    assert_eq!(report.events_checked, 12, "the real chain is whole");
}

#[tokio::test]
async fn a_full_batch_of_rows_with_no_seq_ends_the_walk_instead_of_repeating_it() {
    // The first batch has no `WHERE`, so it returns rows with no `seq` (last). If a whole batch is such rows, no `seq` was read to
    // continue from; the walk must end there and not ask for the same batch for ever.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(false).await;
    rig.tamper_open_checkpoints().await;
    for statement in DROP_EVENTS_KEY {
        rig.admin_sql(statement).await;
    }
    for statement in DROP_CHECKPOINTS_KEY {
        rig.admin_sql(statement).await;
    }
    rig.admin_sql("UPDATE audit_events SET seq = NULL").await;
    // More checkpoint rows with no `seq` than a checkpoint batch holds (500).
    rig.admin_sql(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         SELECT NULL, sha256('x'), 'v', '\\x3006010203040506', 'audit/checkpoints/1.json' FROM generate_series(1, 600)",
    )
    .await;
    rig.admin_sql("UPDATE audit_checkpoints SET seq = NULL").await;
    let verifier = rig.chain_verifier().with_batch_rows(5);
    let report = tokio::time::timeout(Duration::from_secs(30), verifier.verify())
        .await
        .expect("the walk ended")
        .unwrap();
    assert_eq!(
        report.failures,
        [
            failure(0, FailureKind::EventPositionMissing),
            failure(0, FailureKind::CheckpointPositionMissing)
        ]
    );
    assert_eq!((report.events_checked, report.checkpoints_checked), (0, 0));
}

#[tokio::test]
async fn a_checkpoint_with_no_seq_is_a_finding() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    for statement in DROP_CHECKPOINTS_KEY {
        rig.admin_sql(statement).await;
    }
    rig.admin_sql(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         VALUES (NULL, sha256('forged'), 'v', '\\x3006010203040506', 'audit/checkpoints/9.json')",
    )
    .await;
    assert_failed_with(
        &rig,
        "checkpoint without seq",
        &[failure(0, FailureKind::CheckpointPositionMissing)],
    )
    .await;
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3, B2: every value the verifier reads is bounded in the select; an oversize one is reported and never sent.

/// The raw bytes of every column of a fetched row, as the server sent them.
fn row_bytes(row: &sqlx::postgres::PgRow) -> usize {
    (0..sqlx::Row::columns(row).len())
        .map(|i| {
            sqlx::Row::try_get_raw(row, i)
                .ok()
                .and_then(|v| v.as_bytes().ok().map(<[u8]>::len))
                .unwrap_or(0)
        })
        .sum()
}

#[tokio::test]
async fn the_select_sends_the_flag_and_not_an_oversize_value() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql("ALTER TABLE audit_events DROP CONSTRAINT audit_events_event_json_check")
        .await;
    rig.admin_sql(NO_HASH_LENGTH).await;
    // With the unique indexes gone, the hash columns of the chain can be as long as any other.
    rig.admin_sql("ALTER TABLE audit_checkpoints DROP CONSTRAINT audit_checkpoints_seq_head_hash_fkey")
        .await;
    rig.admin_sql(
        "ALTER TABLE audit_events DROP CONSTRAINT audit_events_seq_hash_key, \
         DROP CONSTRAINT audit_events_hash_key, DROP CONSTRAINT audit_events_prev_hash_key",
    )
    .await;
    rig.admin_sql("UPDATE audit_events SET prev_hash = decode(repeat('ab', 1500000), 'hex') WHERE seq = 4")
        .await;
    rig.admin_sql("UPDATE audit_events SET hash = decode(repeat('cd', 1500000), 'hex') WHERE seq = 5")
        .await;
    // Server-side, so that the values never pass through this process: tens of megabytes in a row ...
    rig.admin_sql(
        "UPDATE audit_events SET github_login = repeat('x', 50000000), request_id = repeat('y', 3000000), \
         hash_before = decode(repeat('ab', 1500000), 'hex'), \
         attribution_evidence = ('[' || repeat('1,', 4000000) || '1]')::jsonb, \
         event_json = event_json || repeat(' ', 3000000), hash = sha256(prev_hash || convert_to(event_json || repeat(' ', 3000000), 'UTF8')) \
         WHERE seq = 3",
    )
    .await;
    // ... and in the checkpoint row at 12.
    rig.admin_sql("UPDATE audit_checkpoints SET gcs_object = repeat('o', 5000000), signature = decode(repeat('ab', 2000000), 'hex') WHERE seq = 12")
        .await;
    for (what, sql, binds) in [
        ("chain, first batch", chain_batch_sql(false), 0),
        ("chain, keyset batch", chain_batch_sql(true), 1),
        ("checkpoints, first batch", checkpoint_batch_sql(false), 0),
        ("checkpoints, keyset batch", checkpoint_batch_sql(true), 1),
    ] {
        let mut query = sqlx::query(&sql);
        if binds == 1 {
            query = query.bind(i64::MIN);
        }
        let rows = query.bind(5_000_i64).fetch_all(&rig.pool).await.unwrap();
        assert!(!rows.is_empty(), "{what}");
        let biggest = rows.iter().map(row_bytes).max().unwrap();
        assert!(
            biggest < 8 * 1024,
            "{what}: a row came back with {biggest} bytes: an oversize value was sent"
        );
    }
}

#[tokio::test]
async fn an_event_json_over_the_limit_is_a_hash_failure_even_when_its_hash_is_consistent() {
    // The table's CHECK on the size is dropped, and the hash is recomputed over the long text, so that the only thing wrong with
    // the row is its size. Without a bound the row verifies and the damage shows one row later, as a broken link.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open(true).await;
    rig.admin_sql("ALTER TABLE audit_events DROP CONSTRAINT audit_events_event_json_check")
        .await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = event_json || repeat(' ', 2000000), \
         hash = sha256(prev_hash || convert_to(event_json || repeat(' ', 2000000), 'UTF8')) WHERE seq = 3",
    )
    .await;
    let report = rig.chain_verifier().verify().await.unwrap();
    assert_eq!(report.failures, [failure(3, FailureKind::ChainHashMismatch)]);
}

#[tokio::test]
async fn a_hash_column_of_the_wrong_size_in_the_chain_is_a_hash_failure_not_an_error() {
    for (column, value) in [
        ("prev_hash", "decode(repeat('ab', 33), 'hex')"),
        // As long as a unique index on the column takes.
        ("hash", "decode(repeat('ab', 2000), 'hex')"),
    ] {
        let rig = rig_with_checkpoints(db!()).await;
        rig.tamper_open(true).await;
        rig.admin_sql(NO_HASH_LENGTH).await;
        rig.admin_sql(&format!(
            "UPDATE audit_events SET {column} = {value} WHERE seq = 3"
        ))
        .await;
        let outcome = rig.nightly().run_once().await.unwrap();
        let NightlyOutcome::Failed(report) = outcome else {
            panic!("{column}: expected a failed run, got {outcome:?}");
        };
        assert_eq!(
            report.failures.first(),
            Some(&failure(3, FailureKind::ChainHashMismatch)),
            "{column}"
        );
        assert_eq!(rig.metrics.audit_verify_errors_total(), 0, "{column}");
    }
}

/// The largest event the log accepts, field by field: every bounded field at its longest, the diff at its limit.
fn largest_event() -> ports::AuditEvent {
    let long = |unit: &str| domain::ShortText::parse(&unit.repeat(256 / unit.len())).unwrap();
    let mut e = full_event(0);
    e.actor = ports::AuditActor::System {
        component: long("é")
    };
    e.github_login = Some(long("é"));
    e.diff = Some("x".repeat(ports::AuditEvent::MAX_DIFF_BYTES));
    e.path = Some(
        domain::NfsPath::parse(&format!(
            "{}/{}/{}/{}.yml",
            "a".repeat(250),
            "b".repeat(250),
            "c".repeat(250),
            "d".repeat(200)
        ))
        .unwrap(),
    );
    if let Some(a) = e.attribution.as_mut() {
        a.evidence.executable = Some(long("é"));
        a.evidence.login_identity = Some(long("é"));
        a.actor = Some(domain::Actor::OsLogin { login: long("é") });
    }
    e
}

#[tokio::test]
async fn the_largest_event_the_log_accepts_verifies_clean() {
    // The bounds are derived from what an event can hold; none of them may flag an event the log wrote.
    let rig = AuditRig::new(db!()).await;
    for i in 0..3 {
        let mut e = largest_event();
        e.at = domain::Timestamp::from_unix_millis(1_700_000_000_000 + i);
        rig.record_event(e).await;
    }
    rig.checkpoint_job().run_once().await.unwrap();
    let outcome = rig.nightly().run_once().await.unwrap();
    assert!(matches!(outcome, NightlyOutcome::Clean(_)), "{outcome:?}");
}

#[tokio::test]
async fn the_text_form_of_a_jsonb_is_at_most_one_and_a_half_times_its_compact_form() {
    // `jsonb` prints `, ` and `: ` where the event's compact JSON has `,` and `:`: the worst case is an array of one-byte elements,
    // 2 bytes each in the event and 3 in the column. The bound on the column is twice the bound on the event.
    let db = db!();
    let rig = AuditRig::new(db).await;
    let compact = 1024 * 1024_i64;
    let text_len: i64 = sqlx::query_scalar(
        "SELECT octet_length(('[' || repeat('0,', ($1 / 2 - 1)::int) || '0]')::jsonb::text)::bigint",
    )
    .bind(compact)
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert!(text_len > compact, "the text form is the longer one: {text_len}");
    assert!(
        text_len * 2 <= compact * 3 + 8,
        "at most one and a half times: {text_len}"
    );
    assert!(text_len <= 2 * compact);
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3, N3: the comparison of a batch runs off the async threads.

#[tokio::test(flavor = "current_thread")]
async fn the_comparison_of_a_batch_does_not_block_the_async_runtime() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;
    let rig = AuditRig::new(db!()).await;
    for i in 0..400_i64 {
        let mut e = bare_event(i);
        e.diff = Some("x".repeat(ports::AuditEvent::MAX_DIFF_BYTES));
        rig.record_event(e).await;
    }
    // A task that wakes every 2 ms on the same single-threaded runtime. If the hashing and parsing of the events (25 MB of them)
    // ran on this thread, it would not run for as long as they take.
    let longest_gap_us = Arc::new(AtomicU64::new(0));
    let ticker = {
        let longest = longest_gap_us.clone();
        tokio::spawn(async move {
            let mut last = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(2)).await;
                let now = Instant::now();
                let gap = u64::try_from(now.duration_since(last).as_micros()).unwrap_or(u64::MAX);
                longest.fetch_max(gap, Ordering::Relaxed);
                last = now;
            }
        })
    };
    let started = Instant::now();
    let report = rig.chain_verifier().verify().await.unwrap();
    let took = started.elapsed();
    ticker.abort();
    assert!(report.is_clean());
    assert_eq!(report.events_checked, 400);
    let gap = Duration::from_micros(longest_gap_us.load(Ordering::Relaxed));
    assert!(
        gap < Duration::from_millis(250),
        "the runtime was held for {gap:?} during a verification that took {took:?}"
    );
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3, N5: the verifier's queries are bounded in time.

#[tokio::test]
async fn a_table_that_stays_locked_ends_the_run_as_errored_in_bounded_time() {
    let rig = rig_with_checkpoints(db!()).await;
    // An owner holds ACCESS EXCLUSIVE on the table and does not let go.
    let mut owner = rig.db.admin().await;
    sqlx::query("BEGIN").execute(&mut owner).await.unwrap();
    sqlx::query("LOCK TABLE audit_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut owner)
        .await
        .unwrap();

    let limited = || {
        rig.chain_verifier()
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(500))
    };
    let started = std::time::Instant::now();
    let aborted = tokio::time::timeout(Duration::from_secs(20), limited().verify())
        .await
        .expect("the verification ended within the bound")
        .unwrap_err();
    assert_eq!(aborted.error, VerifyError::Timeout);
    assert!(aborted.partial.is_clean() && aborted.partial.events_checked == 0);
    // The job: errored, counted, not renewing a lease for ever.
    let job = rig.nightly_with(limited());
    let outcome = tokio::time::timeout(Duration::from_secs(20), job.run_once())
        .await
        .expect("the job ended within the bound")
        .unwrap();
    assert_eq!(outcome, NightlyOutcome::Errored);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "took {:?}",
        started.elapsed()
    );
    sqlx::query("ROLLBACK").execute(&mut owner).await.unwrap();

    // Once the lock is gone the same verifier works again.
    assert!(limited().verify().await.unwrap().is_clean());
}

#[tokio::test]
async fn a_query_that_waits_past_the_statement_timeout_is_cancelled_by_the_server() {
    // The lock timeout is long here (a minute) and the statement timeout short: only the statement timeout can end this wait, and
    // it does so in the server. The client's own limit is thirty seconds past it.
    let rig = rig_with_checkpoints(db!()).await;
    let mut owner = rig.db.admin().await;
    sqlx::query("BEGIN").execute(&mut owner).await.unwrap();
    sqlx::query("LOCK TABLE audit_events IN ACCESS EXCLUSIVE MODE")
        .execute(&mut owner)
        .await
        .unwrap();
    let verifier = rig
        .chain_verifier()
        .with_timeouts(Duration::from_secs(1), Duration::from_secs(60));
    let aborted = tokio::time::timeout(Duration::from_secs(20), verifier.verify())
        .await
        .expect("the server ended the wait")
        .unwrap_err();
    assert_eq!(aborted.error, VerifyError::Timeout);
    sqlx::query("ROLLBACK").execute(&mut owner).await.unwrap();
}

#[tokio::test]
async fn the_timeouts_are_set_for_the_transaction_and_do_not_outlive_it() {
    // One connection in the pool: whatever a query left set on it is what the next user of the pool gets.
    let rig = rig_with_checkpoints(db!()).await;
    let pool = rig.db.app_pool(1).await;
    let verifier =
        hub_identity::audit::ChainVerifier::new(pool.clone(), rig.bucket.clone(), rig.public_key())
            .with_timeouts(Duration::from_secs(7), Duration::from_secs(3));
    assert!(verifier.verify().await.unwrap().is_clean());
    let statement: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    let lock: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((statement.as_str(), lock.as_str()), ("0", "0"));
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3: the headline of the alert is the most severe failure, whatever order they were found in.

#[tokio::test]
async fn the_alert_names_the_most_severe_failure_and_not_the_first_found() {
    let rig = rig_with_a_tamper_at_5(db!()).await;
    // A column difference at 2 is found first; the broken chain at 5 and the bad checkpoint at 12 after it.
    rig.admin_sql("UPDATE audit_events SET via = 'mcp' WHERE seq = 2")
        .await;
    rig.admin_sql("UPDATE audit_checkpoints SET head_hash = sha256('other') WHERE seq = 12")
        .await;
    let report = assert_failed_with(
        &rig,
        "order",
        &[
            failure(2, FailureKind::ColumnsDiffer(ColumnSet::of(&[Column::Via]))),
            failure(5, FailureKind::ChainHashMismatch),
            failure(12, FailureKind::CheckpointHeadMismatch),
        ],
    )
    .await;
    assert_eq!(report.failures.len(), 3);
    let sent = rig.notifier.sent();
    let summary = sent[0].summary.as_str();
    assert!(
        summary.contains("3 problem(s)") && summary.contains("event hash mismatch at position 5"),
        "{summary}"
    );
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 3: plans of the verifier's queries (P12 family: the keyset walk reads by the primary key, never sorts, never scans).

async fn plan_of(rig: &AuditRig, sql: &str) -> serde_json::Value {
    let mut admin = rig.db.admin().await;
    sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {sql}"))
        .fetch_one(&mut admin)
        .await
        .unwrap()
}

fn node_types(plan: &serde_json::Value) -> Vec<(String, String)> {
    fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
        out.push((
            node["Node Type"].as_str().unwrap_or_default().to_owned(),
            node["Index Name"]
                .as_str()
                .or_else(|| node["Relation Name"].as_str())
                .unwrap_or_default()
                .to_owned(),
        ));
        for child in node["Plans"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(&plan[0]["Plan"], &mut out);
    out
}

#[tokio::test]
async fn the_verifier_reads_the_chain_by_the_primary_key_without_sorting() {
    let rig = AuditRig::new(db!()).await;
    rig.record(300).await;
    let mut admin = rig.db.admin().await;
    sqlx::query("ANALYZE audit_events")
        .execute(&mut admin)
        .await
        .unwrap();
    for (what, sql) in [
        ("first batch", chain_batch_sql(false).replace("$1", "10")),
        (
            "keyset batch",
            chain_batch_sql(true).replace("$1", "100").replace("$2", "10"),
        ),
    ] {
        let nodes = node_types(&plan_of(&rig, &sql).await);
        assert!(
            nodes
                .iter()
                .any(|(t, i)| t == "Index Scan" && i == "audit_events_pkey"),
            "{what}: {nodes:?}"
        );
        assert!(
            !nodes.iter().any(|(t, _)| t == "Sort" || t == "Seq Scan"),
            "{what}: {nodes:?}"
        );
    }
}

#[tokio::test]
async fn the_look_for_an_event_without_a_seq_is_an_index_probe_while_the_key_is_there() {
    // With the primary key in place the probe reads an index and finds nothing, so a large table costs it nothing.
    let rig = AuditRig::new(db!()).await;
    rig.record(300).await;
    let mut admin = rig.db.admin().await;
    sqlx::query("ANALYZE audit_events")
        .execute(&mut admin)
        .await
        .unwrap();
    let plan = plan_of(
        &rig,
        "SELECT EXISTS (SELECT 1 FROM audit_events WHERE seq IS NULL)",
    )
    .await;
    let nodes = node_types(&plan);
    assert!(
        nodes
            .iter()
            .any(|(t, i)| t.starts_with("Index") && i == "audit_events_pkey"),
        "{nodes:?}"
    );
    assert!(!nodes.iter().any(|(t, _)| t == "Seq Scan"), "{nodes:?}");
}
