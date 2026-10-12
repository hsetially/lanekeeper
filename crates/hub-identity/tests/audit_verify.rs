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
use hub_identity::audit::columns::jsonb_rejections;
use hub_identity::audit::verify::{
    CheckpointColumn, CheckpointColumnSet, FailureKind, NightlyOutcome, VerifyError, chain_batch_sql,
    checkpoint_batch_sql,
};
use hub_identity::audit::{Column, ColumnSet};
use hub_identity::audit::{MemoryBucket, VerifyFailure};
use hub_identity::metrics::CounterSource;
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
    assert_eq!(
        report.batches, 3,
        "10 + 10 + 7 rows read: the second and third start by reading the last row of the one before again"
    );
    // A log that fills the last batch exactly ends on a read that finds nothing new, not on a guess: a batch starts at the last
    // position the one before read (so that rows sharing a position are never split), finds that row alone, skips it as already
    // compared, and is short.
    let report = rig.chain_verifier().with_batch_rows(25).verify().await.unwrap();
    assert_eq!(report.events_checked, 25, "no row is compared twice");
    assert_eq!(report.batches, 2);
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
async fn a_query_the_database_refuses_for_the_shape_of_its_tables_is_a_finding_and_a_dead_connection_is_an_error()
 {
    // The line between "the tables are not what the verifier reads" (a finding, at once: nothing transient does that) and "the
    // database cannot be read" (an error): a column the select names is gone ...
    let rig = rig_with_checkpoints(db!()).await;
    rig.admin_sql("ALTER TABLE audit_events RENAME COLUMN via TO via_renamed")
        .await;
    assert!(rig.chain_verifier().verify().await.is_err());
    let outcome = rig.nightly().run_once().await.unwrap();
    let NightlyOutcome::Incomplete(report) = outcome else {
        panic!("expected an incomplete run with a finding, got {outcome:?}");
    };
    assert!(
        matches!(report.failures.as_slice(), [f] if matches!(f.kind, FailureKind::SchemaChanged(_))),
        "{:?}",
        report.failures
    );
    assert_eq!(rig.notifier.sent().len(), 1, "the admins are told at once");
    // ... and a connection that is gone is an error, and not that.
    let rig = rig_with_checkpoints(db!()).await;
    let verifier = rig.chain_verifier();
    rig.pool.close().await;
    let aborted = verifier.verify().await.unwrap_err();
    assert!(
        !matches!(aborted.error, VerifyError::SchemaChanged(_)),
        "{:?}",
        aborted.error
    );
    assert!(aborted.partial.is_clean());
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

/// A verifier whose reads give up on a locked table after 200 ms.
fn quick_verifier(rig: &AuditRig) -> hub_identity::audit::ChainVerifier {
    rig.chain_verifier()
        .with_timeouts(Duration::from_secs(5), Duration::from_millis(200))
}

#[tokio::test]
async fn a_checkpoint_query_that_fails_still_reports_what_the_chain_walk_found() {
    // A real query-level fault in the second half (the checkpoints are locked by an owner, or the shape of their table is not what
    // the select names). The findings of the first half are not thrown away with it: they come back with the error, and the
    // admins are told.
    for (what, error) in [
        ("locked", VerifyError::Timeout),
        ("renamed", VerifyError::SchemaChanged(None)),
    ] {
        let rig = rig_with_a_tamper_at_5(db!()).await;
        let mut owner = None;
        if what == "locked" {
            owner = Some(lock_table(&rig, "audit_checkpoints").await);
        } else {
            rig.admin_sql("ALTER TABLE audit_checkpoints RENAME COLUMN gcs_object TO gcs_object_renamed")
                .await;
        }
        let aborted = quick_verifier(&rig).verify().await.unwrap_err();
        match (error, aborted.error) {
            (VerifyError::SchemaChanged(_), VerifyError::SchemaChanged(Some(_))) => {}
            (expected, got) => assert_eq!(got, expected, "{what}"),
        }
        let mut expected = vec![failure(5, FailureKind::ChainHashMismatch)];
        if what == "renamed" {
            // The refusal of the shape of the tables is a finding as well.
            expected.push(failure(
                0,
                FailureKind::SchemaChanged(match aborted.error {
                    VerifyError::SchemaChanged(code) => code,
                    _ => None,
                }),
            ));
        }
        assert_eq!(aborted.partial.failures, expected, "{what}");
        assert_eq!(aborted.partial.events_checked, 5, "{what}");

        let outcome = nights::job(&rig, &Arc::<nights::EveryNight>::default())
            .run_once()
            .await
            .unwrap();
        let NightlyOutcome::Incomplete(report) = outcome else {
            panic!("{what}: expected an incomplete run, got {outcome:?}");
        };
        assert_eq!(report.failures, expected, "{what}");
        assert_eq!(
            rig.metrics.audit_verify_errors_total(),
            1,
            "{what}: it is also an error"
        );
        assert_eq!(
            rig.metrics.audit_verify_failures_total(),
            u64::try_from(expected.len()).unwrap(),
            "{what}: and the findings are counted"
        );
        let sent = rig.notifier.sent();
        assert_eq!(sent.len(), 1, "{what}: the admins are told of the finding");
        let summary = sent[0].summary.as_str();
        assert!(
            summary.contains("position 5") && summary.contains("did not finish"),
            "{what}: {summary}"
        );
        if let Some(owner) = owner.as_mut() {
            unlock(owner).await;
        }
    }
}

#[tokio::test]
async fn a_chain_query_that_fails_does_not_stop_the_checkpoints_being_checked() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    // The checkpoint at 12 now signs a head the chain does not have ...
    rig.admin_sql("UPDATE audit_checkpoints SET head_hash = sha256('other') WHERE seq = 12")
        .await;
    // ... and the chain walk cannot run at all (the checkpoint query reads only `seq` and `hash` of the events).
    rig.admin_sql("ALTER TABLE audit_events RENAME COLUMN via TO via_renamed")
        .await;
    let aborted = rig.chain_verifier().verify().await.unwrap_err();
    let VerifyError::SchemaChanged(code) = aborted.error else {
        panic!(
            "expected a refusal of the shape of the table, got {:?}",
            aborted.error
        );
    };
    assert!(code.is_some());
    assert_eq!(
        aborted.partial.failures,
        [
            failure(12, FailureKind::CheckpointHeadMismatch),
            failure(0, FailureKind::SchemaChanged(code))
        ]
    );
    assert_eq!(aborted.partial.checkpoints_checked, 2);
}

#[tokio::test]
async fn a_run_that_found_nothing_before_it_failed_is_plainly_errored() {
    // A fault of the database that is not about the tables' shape: the checkpoints are locked and the read gives up. No finding,
    // so nothing is told to the admins that night (the nights in a row are counted: round 4, N5).
    let rig = rig_with_checkpoints(db!()).await;
    let mut owner = lock_table(&rig, "audit_checkpoints").await;
    let job = rig.nightly_with(quick_verifier(&rig));
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert_eq!(rig.metrics.audit_verify_failures_total(), 0);
    assert_eq!(rig.metrics.audit_verify_schema_changes_total(), 0);
    assert!(rig.notifier.sent().is_empty());
    unlock(&mut owner).await;
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

// ---------------------------------------------------------------------------------------------------------------------------
// Round 4, B1: no value stored in a `jsonb` column can make the select itself fail, and none costs more than a bounded amount.
//
// A `jsonb` number is stored as a compact `numeric`, and printed in full: `1e131000` is a dozen bytes stored and 131 KB printed. The
// INSERT-only app role can store 9,000 of them in 2.5 KB (144 KB decompressed), and printing that value is refused by the server
// (SQLSTATE 54000, out of memory, after a gigabyte). A select that prints the column to measure it therefore failed on a stored value, every night, and nothing after
// that row was ever verified. These tests append such rows as the app role (the only role that needs to), put legitimate events
// after them, tamper with a later row as the owner, and require the run to end as `Failed`, quickly, with the later tamper found.

use hub_identity::audit::Column as C;

/// The columns of `audit_events` that copy the event, in the order of `Column::ALL`.
fn copied_columns() -> Vec<&'static str> {
    C::ALL.iter().map(|c| c.name()).collect()
}

/// Append one row, as the INSERT-only app role: the last row's event, correctly chained to the head, with `column` set to the
/// SQL expression `value_sql` and every other copy left as it was. Returns its `seq`.
async fn append_copy_with(rig: &AuditRig, column: C, value_sql: &str) -> i64 {
    let select: Vec<String> = C::ALL
        .iter()
        .map(|c| {
            if *c == column {
                value_sql.to_owned()
            } else {
                c.name().to_owned()
            }
        })
        .collect();
    sqlx::query_scalar(&format!(
        "INSERT INTO audit_events (prev_hash, hash, event_json, {}) \
         SELECT hash, sha256(hash || convert_to(event_json, 'UTF8')), event_json, {} \
         FROM (SELECT * FROM audit_events ORDER BY seq DESC LIMIT 1) head RETURNING seq",
        copied_columns().join(", "),
        select.join(", ")
    ))
    .fetch_one(&rig.pool)
    .await
    .unwrap()
}

/// `n` copies of `element`, as a JSON array that ends in a plain `1`.
fn array_of(n: usize, element: &str) -> String {
    format!("('[' || repeat('{element},', {n}) || '1]')::jsonb")
}

/// An array nested `depth` levels deep, as a `jsonb` expression (8 bytes a level once decompressed: 1,024 levels are 8,192).
fn nested(depth: usize) -> String {
    format!("(repeat('[', {depth}) || repeat(']', {depth}))::jsonb")
}

/// How deep a value the server under test takes. The stack a server may use is `max_stack_depth`, and what fits in it depends on
/// how the binary was built (the frames of one build are larger than another's: the same 2 MB took 16,300 levels from Ubuntu's
/// Postgres 16.15 and fewer from the Debian build in the pinned image, on the gate run that found this). So no test names a depth
/// that is large; each asks the server, once, and builds what it needs from the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServerDepth {
    /// The deepest nesting the server takes as `jsonb` input (SQLSTATE 54001, "stack depth limit exceeded", past it), or
    /// [`DEPTH_PROBE_CAP`] when it takes that and more.
    parse: usize,
    /// The deepest nesting the third check of the select (the `jsonpath` one) reads. `jsonpath` needs more stack for a level than
    /// the parser, so it is at most `parse`; when it equals `parse`, no value the server can store is one `jsonpath` cannot read.
    jsonpath: usize,
}

/// The deepest the probe looks. Nothing here needs more, and a deeper value only costs the server time.
const DEPTH_PROBE_CAP: usize = 20_000;
/// The nesting of the deepest value the size gate lets through: its 1,024 levels are 8,192 bytes decompressed.
const GATE_DEPTH: usize = 1_024;
/// What these tests need of a server: to store a value 1,100 deep (over the gate: 8,800 bytes), and for `jsonpath` to read 1,100
/// levels (above the 1,024 that the gate lets through, with a little to spare). A server that cannot is one whose stack is too
/// small for the verifier's own bound, which is a finding about the server, not a reason for the tests to pass or to skip.
const NEEDED_DEPTH: usize = 1_100;

impl ServerDepth {
    /// The depth the tests build for "as deep as the server takes": a tenth under what the probe took. The statement that stores
    /// the value has more frames on the stack than the probe's, and the answer should hold for both.
    fn deepest(self) -> usize {
        self.parse - self.parse / 10
    }

    /// Why this server is not one the tests can run on, or `None` when it is.
    fn shortfall(self) -> Option<String> {
        (self.deepest() < NEEDED_DEPTH || self.jsonpath < NEEDED_DEPTH).then(|| {
            format!(
                "this server takes jsonb nested {} deep as input (the tests build up to {}) and `jsonpath` reads {} levels; the tests \
                 need {NEEDED_DEPTH} of each, to put a value over the {GATE_DEPTH}-level size gate under it. That is a 'max_stack_depth' \
                 of about 210kB for the Postgres 16 builds measured (the default is 2MB): the server under test has less stack than \
                 the verifier's own bound assumes",
                self.parse,
                self.deepest(),
                self.jsonpath
            )
        })
    }

    /// The nesting depths the verifier is run against, shallow to deep: just under the gate, the deepest value it lets through
    /// (whose `jsonpath` check does run), just over it, and then up to the deepest the server takes, with the depth at which
    /// `jsonpath` itself gives up and the one after it where the server takes values `jsonpath` cannot read.
    fn ladder(self) -> Vec<usize> {
        let mut depths: Vec<usize> = [1_000, GATE_DEPTH, GATE_DEPTH + 1, 1_100, 2_000, 4_000, 8_000]
            .into_iter()
            .chain([self.jsonpath, self.jsonpath + 1])
            .chain([self.deepest()])
            .filter(|d| *d <= self.deepest())
            .collect();
        depths.sort_unstable();
        depths.dedup();
        depths
    }
}

/// The deepest nesting `ok` accepts, between 1 (always taken) and `cap`: the statements `sql` builds for a depth, each of which
/// either succeeds or is refused for its stack (54001). Any other error is a bug in the probe.
async fn deepest_taken(admin: &mut sqlx::PgConnection, cap: usize, sql: impl Fn(usize) -> String) -> usize {
    let (mut taken, mut refused) = (1_usize, cap + 1);
    while refused - taken > 1 {
        let depth = taken + (refused - taken) / 2;
        match sqlx::query_scalar::<_, bool>(&sql(depth))
            .fetch_one(&mut *admin)
            .await
        {
            Ok(_) => taken = depth,
            Err(e) if support::pg::sqlstate(&e).as_deref() == Some("54001") => refused = depth,
            Err(e) => panic!("probing the depth {depth}: {e}"),
        }
    }
    taken
}

/// What the server under test takes, probed on the test's own database (about thirty small statements; the answer is a property
/// of the server's binary and its `max_stack_depth`, the same for every database on it), or `None` when it cannot run these tests
/// and the environment does not require it to: `LK_REQUIRE_DOCKER=1` fails the test instead, with the probed numbers in the
/// message.
#[allow(clippy::print_stderr)]
async fn server_depth(db: &support::pg::TestDb) -> Option<ServerDepth> {
    let mut admin = db.admin().await;
    let parse = deepest_taken(&mut admin, DEPTH_PROBE_CAP, |n| {
        format!("SELECT jsonb_typeof({}) = 'array'", nested(n))
    })
    .await;
    // The production expression, so that the probe and the select cannot drift apart.
    let check = &jsonb_rejections("v")[2];
    let jsonpath = deepest_taken(&mut admin, parse, |n| {
        format!("SELECT {check} FROM (SELECT {} AS v) s", nested(n))
    })
    .await;
    let depth = ServerDepth { parse, jsonpath };
    eprintln!(
        "server depth: jsonb input takes {parse} levels{}, jsonpath reads {jsonpath}{}",
        if parse == DEPTH_PROBE_CAP {
            " (the probe's cap)"
        } else {
            ""
        },
        if jsonpath == parse {
            ": every value the server stores is one jsonpath reads"
        } else {
            ": the server stores values jsonpath cannot read"
        }
    );
    if let Some(why) = depth.shortfall() {
        support::pg::skip_or_fail(&why);
        return None;
    }
    Some(depth)
}

#[test]
fn a_server_without_the_stack_for_the_gate_is_named_and_one_with_it_is_not() {
    // Measured on Ubuntu's Postgres 16.15 at max_stack_depth 100kB, 512kB, 1MB, 1536kB, 2MB and 6MB (the last is the probe's cap).
    for (parse, jsonpath, takes_it) in [
        (815, 540, false),
        (4_062, 2_695, true),
        (8_164, 5_429, true),
        (12_265, 8_164, true),
        (16_328, 10_859, true),
        (DEPTH_PROBE_CAP, 20_000, true),
        // The edges: 1,100 is needed of each (the parser's is taken a tenth off).
        (1_221, 1_100, false),
        (1_222, 1_100, true),
        (5_000, 1_099, false),
        (1_100, 1_100, false),
    ] {
        let why = ServerDepth { parse, jsonpath }.shortfall();
        assert_eq!(why.is_none(), takes_it, "{parse} / {jsonpath}: {why:?}");
        if let Some(why) = why {
            // Named, with the numbers it probed, so that the failure under LK_REQUIRE_DOCKER=1 says what the server can do.
            assert!(
                why.contains(&parse.to_string()) && why.contains(&jsonpath.to_string()),
                "{why}"
            );
        }
    }
}

#[test]
fn the_ladder_runs_from_under_the_gate_to_the_deepest_and_stops_where_the_server_does() {
    // Where jsonpath gives up below the parser (Ubuntu at 2MB): both sides of it are in the ladder.
    let ladder = ServerDepth {
        parse: 16_328,
        jsonpath: 10_859,
    }
    .ladder();
    assert_eq!(
        ladder,
        [
            1_000, 1_024, 1_025, 1_100, 2_000, 4_000, 8_000, 10_859, 10_860, 14_696
        ]
    );
    // Where it reads whatever is stored: nothing past `deepest` is asked for, and nothing is repeated.
    let ladder = ServerDepth {
        parse: 8_164,
        jsonpath: 8_164,
    }
    .ladder();
    assert_eq!(ladder, [1_000, 1_024, 1_025, 1_100, 2_000, 4_000, 7_348]);
    let ladder = ServerDepth {
        parse: 1_222,
        jsonpath: 1_100,
    }
    .ladder();
    assert_eq!(ladder, [1_000, 1_024, 1_025, 1_100]);
}

/// A value that prints very large, or that makes a function over it fail, though it is stored small. The nesting ones are built
/// from what the server takes (see [`ServerDepth`]).
fn poisons(depth: ServerDepth) -> Vec<(String, String)> {
    let deepest = depth.deepest();
    let mut poisons: Vec<(String, String)> = vec![
        (
            "9000 x 1e131000: 144 KB decompressed, 2.5 KB stored; prints over 1 GiB and the server refuses",
            array_of(9000, "1e131000"),
        ),
        (
            "7000 x 1e131000: 112 KB decompressed, 1.9 KB stored; prints 917 MB and takes 8 to 13 s",
            array_of(7000, "1e131000"),
        ),
        ("9000 x -1e131000", array_of(9000, "-1e131000")),
        ("9000 x 1e-16383: prints 16 KB each", array_of(9000, "1e-16383")),
        (
            "9000 x 0e-16383: zero with the largest scale",
            array_of(9000, "0e-16383"),
        ),
        (
            "300 x 0e-16383: small raw, many numbers that print 16 KB each",
            array_of(300, "0e-16383"),
        ),
        ("8 x 1e131000: few numbers, 1 MB printed", array_of(8, "1e131000")),
        (
            "a number past the largest i64",
            "'{\"n\":9223372036854775808}'::jsonb".to_owned(),
        ),
    ]
    .into_iter()
    .map(|(label, sql)| (label.to_owned(), sql))
    .collect();
    poisons.push((
        format!(
            "nested {deepest} deep, the most this server is asked to take (it takes {}; jsonpath reads {})",
            depth.parse, depth.jsonpath
        ),
        nested(deepest),
    ));
    poisons.push(("nested 1,100 deep: 8.8 KB raw".to_owned(), nested(NEEDED_DEPTH)));
    poisons
}

/// One poisoned row, 3 legitimate events after it, a tamper of the stored event at 15 by the owner: the run is `Failed`, quickly,
/// names the poisoned column at 13 and the hash at 15, and is not an error.
async fn a_poisoned_column_is_a_finding_and_a_later_tamper_is_still_found(
    rig: &AuditRig,
    label: &str,
    column: C,
    value_sql: &str,
) {
    let poisoned = append_copy_with(rig, column, value_sql).await;
    assert_eq!(poisoned, 13, "{label}");
    assert_eq!(
        rig.record(3).await,
        16,
        "{label}: three legitimate events after it"
    );
    rig.tamper_open(true).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 15",
    )
    .await;
    let started = std::time::Instant::now();
    let outcome = rig.nightly().run_once().await.unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(5), "{label}: took {took:?}");
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("{label}: expected a failed run, got {outcome:?}");
    };
    assert_eq!(
        report.failures,
        [
            failure(13, FailureKind::ColumnsDiffer(ColumnSet::of(&[column]))),
            failure(15, FailureKind::ChainHashMismatch),
        ],
        "{label}"
    );
    assert_eq!(report.events_checked, 15, "{label}");
    assert_eq!(
        rig.metrics.audit_verify_errors_total(),
        0,
        "{label}: a finding, not an error"
    );
    assert_eq!(rig.notifier.sent().len(), 1, "{label}");
}

/// `poisons(..)[index]` in `column`, through [`a_poisoned_column_is_a_finding_and_a_later_tamper_is_still_found`], on a database
/// of its own (the nesting ones are built from what that server takes).
async fn the_poison_is_a_finding(index: usize, column: C) {
    let rig = rig_with_checkpoints(db!()).await;
    let Some(depth) = server_depth(&rig.db).await else {
        return;
    };
    let (label, sql) = poisons(depth).swap_remove(index);
    a_poisoned_column_is_a_finding_and_a_later_tamper_is_still_found(&rig, &label, column, &sql).await;
}

#[tokio::test]
async fn jsonb_numbers_that_print_over_a_gigabyte_are_a_finding_not_an_error() {
    the_poison_is_a_finding(0, C::AttributionEvidence).await;
}

#[tokio::test]
async fn jsonb_numbers_that_take_seconds_to_print_are_a_finding_and_cost_nothing() {
    the_poison_is_a_finding(1, C::AttributionEvidence).await;
}

#[tokio::test]
async fn jsonb_numbers_with_large_negative_and_positive_exponents_are_findings() {
    for i in [2, 3, 4, 5, 6, 7, 8] {
        the_poison_is_a_finding(i, C::AttributionEvidence).await;
    }
}

#[tokio::test]
async fn jsonb_nested_as_deep_as_the_server_takes_is_a_finding_not_an_error() {
    for i in [8, 9] {
        the_poison_is_a_finding(i, C::AttributionEvidence).await;
    }
}

/// The invariant behind the depth bound: at no depth the server takes does the run end `Errored` or stop short of a later row.
/// From just under the size gate, through the deepest value the gate lets through (whose `jsonpath` check does run, on the
/// most stack the verifier ever uses), to the deepest the server takes, and on a server that takes values `jsonpath` cannot
/// read (the Ubuntu build at 2 MB does; others may not) the depths on either side of where `jsonpath` gives up. The depths come
/// from the server, so a build with larger frames is held to the same invariant at the depths it has.
#[tokio::test]
async fn at_no_depth_the_server_takes_does_the_run_end_errored_or_skip_a_later_row() {
    let mut step = 0;
    loop {
        let rig = rig_with_checkpoints(db!()).await;
        let Some(depth) = server_depth(&rig.db).await else {
            return;
        };
        let ladder = depth.ladder();
        let Some(&at) = ladder.get(step) else {
            break;
        };
        assert!(
            ladder.contains(&GATE_DEPTH)
                && ladder.contains(&NEEDED_DEPTH)
                && ladder.contains(&depth.deepest()),
            "{ladder:?}"
        );
        let label = format!("nested {at} deep (the ladder is {ladder:?})");
        a_poisoned_column_is_a_finding_and_a_later_tamper_is_still_found(
            &rig,
            &label,
            C::AttributionEvidence,
            &nested(at),
        )
        .await;
        step += 1;
    }
    assert!(step >= 4, "only {step} depths were run");
}

#[tokio::test]
async fn the_attribution_actor_column_is_held_to_the_same_bounds() {
    for i in [0, 1, 3, 8] {
        the_poison_is_a_finding(i, C::AttributionActor).await;
    }
}

#[tokio::test]
async fn several_poisoned_rows_do_not_add_up_to_a_timeout() {
    // The reviewer's measure: a few rows that take 13 s each to print reached the 5-minute statement timeout. Here ten of them, in
    // both columns, and the run is still quick and still finds the later tamper.
    let rig = rig_with_checkpoints(db!()).await;
    for i in 0..10 {
        let column = if i % 2 == 0 {
            C::AttributionEvidence
        } else {
            C::AttributionActor
        };
        append_copy_with(&rig, column, &array_of(7000, "1e131000")).await;
    }
    rig.record(3).await;
    rig.tamper_open(true).await;
    rig.admin_sql(
        "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 25",
    )
    .await;
    let started = std::time::Instant::now();
    let outcome = rig.nightly().run_once().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(report.failures.len(), 11, "{:?}", report.failures);
    assert_eq!(
        report.failures.last(),
        Some(&failure(25, FailureKind::ChainHashMismatch))
    );
    assert_eq!(rig.metrics.audit_verify_errors_total(), 0);
}

/// What the select sends for `value_sql` in `attribution_evidence` (the last row of the first batch, after the ones before it), and
/// what `read_row` makes of it.
async fn sent_for(
    rig: &AuditRig,
    value_sql: &str,
) -> (
    Option<serde_json::Value>,
    hub_identity::audit::columns::ReadColumns,
) {
    use sqlx::Row;
    append_copy_with(rig, C::AttributionEvidence, value_sql).await;
    let rows = sqlx::query(&chain_batch_sql(false))
        .bind(5_000_i64)
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    let last = rows.last().unwrap();
    assert!(
        row_bytes(last) < 32 * 1024,
        "a row of {} bytes was sent",
        row_bytes(last)
    );
    let sent = last
        .try_get::<Option<serde_json::Value>, _>("attribution_evidence")
        .unwrap();
    (sent, hub_identity::audit::columns::read_row(last).unwrap())
}

#[tokio::test]
async fn a_jsonb_the_select_does_not_vet_is_sent_as_the_json_null_and_a_vetted_one_as_it_is() {
    let rig = rig_with_checkpoints(db!()).await;
    let Some(depth) = server_depth(&rig.db).await else {
        return;
    };
    // Vetted: what the log writes, and its extremes. These come back as they are, and `read_row` finds nothing unreadable in them.
    for (what, sql, expected) in [
        (
            "an object of text and a time",
            "'{\"executable\":\"/usr/bin/vim\",\"observed_at\":1791676800005}'::jsonb",
            serde_json::json!({"executable": "/usr/bin/vim", "observed_at": 1_791_676_800_005_i64}),
        ),
        (
            "the extremes of an i64",
            "'{\"a\":9223372036854775807,\"b\":-9223372036854775808}'::jsonb",
            serde_json::json!({"a": i64::MAX, "b": i64::MIN}),
        ),
        (
            "an integer written with a decimal point",
            "'{\"a\":5.0}'::jsonb",
            serde_json::from_str::<serde_json::Value>("{\"a\":5.0}").unwrap(),
        ),
    ] {
        let (sent, read) = sent_for(&rig, sql).await;
        assert_eq!(sent, Some(expected), "{what}");
        assert!(!read.unreadable.contains(Column::AttributionEvidence), "{what}");
    }
    // Not vetted: each comes back as the JSON `null`, which no event holds in these columns, and nothing is printed. `read_row`
    // records the column as unreadable.
    for (label, sql) in poisons(depth) {
        let (sent, read) = sent_for(&rig, &sql).await;
        assert_eq!(sent, Some(serde_json::Value::Null), "{label}");
        assert!(read.unreadable.contains(Column::AttributionEvidence), "{label}");
        assert!(!read.unreadable.contains(Column::AttributionActor), "{label}");
    }
    // A stored `null` (SQL) is still `None`: the column of an event with no attribution.
    let (sent, read) = sent_for(&rig, "NULL").await;
    assert_eq!(sent, None);
    assert!(!read.unreadable.contains(Column::AttributionEvidence));
}

/// The five checks of the select, each evaluated by itself on a row stored in a table (so that its size is the stored size, as it
/// is in `audit_events`), on the values each is for. A check is evaluated only where a check before it lets the value through, as
/// the select does: `jsonpath` itself refuses a value nested deeper than it reads (about 10,800 levels on Ubuntu's Postgres at 2 MB
/// and fewer on a build with larger frames, where the server may not even take a value that deep), and printing is what the first
/// four exist to avoid.
#[tokio::test]
async fn each_check_on_a_jsonb_rejects_what_it_is_for_and_lets_the_rest_through() {
    let rig = AuditRig::new(db!()).await;
    let Some(depth) = server_depth(&rig.db).await else {
        return;
    };
    let deepest = depth.deepest();
    let mut admin = rig.db.admin().await;
    sqlx::query("CREATE TABLE vet_probe (id int PRIMARY KEY, attribution_evidence jsonb)")
        .execute(&mut admin)
        .await
        .unwrap();
    let reasons = jsonb_rejections("attribution_evidence");

    // (value, the checks to evaluate on it with what each must say). Checks are numbered from 0: stored size, decompressed size,
    // a number that prints long, too many numbers, text length.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, String, Vec<(usize, bool)>)> = vec![
        (
            "what the log writes",
            "'{\"executable\":\"/usr/bin/vim\",\"login_identity\":\"jdoe@corp\",\"observed_at\":1791676800005}'::jsonb".to_owned(),
            vec![(0, false), (1, false), (2, false), (3, false), (4, false)],
        ),
        (
            "both extremes of an i64, and 5.0",
            "'{\"a\":9223372036854775807,\"b\":-9223372036854775808,\"c\":5.0}'::jsonb".to_owned(),
            vec![(0, false), (1, false), (2, false), (3, false), (4, false)],
        ),
        (
            "stored large: 128 KB of text that does not compress",
            "to_jsonb((SELECT string_agg(md5(i::text), '') FROM generate_series(1, 4000) i))".to_owned(),
            vec![(0, true), (1, true)],
        ),
        (
            "stored small, decompressed large: 500 KB of one character",
            "to_jsonb(repeat('x', 500000))".to_owned(),
            vec![(0, false), (1, true)],
        ),
        (
            "9,000 numbers of 1e131000: stored 2.5 KB, 144 KB decompressed",
            array_of(9000, "1e131000"),
            vec![(0, false), (1, true), (2, true), (3, true)],
        ),
        (
            "nested 1,024 deep, the most the gate lets through: 8,192 bytes decompressed, and every later check runs on it",
            nested(GATE_DEPTH),
            vec![(0, false), (1, false), (2, false), (3, false), (4, false)],
        ),
        (
            "nested 1,025 deep, one more than the gate lets through: under the stored limit, over the decompressed one",
            nested(GATE_DEPTH + 1),
            vec![(0, false), (1, true)],
        ),
        (
            "nested 1,100 deep: under the stored limit, over the decompressed one",
            nested(NEEDED_DEPTH),
            vec![(0, false), (1, true)],
        ),
        (
            // Stored, the nesting compresses to about 3.3 bytes a level, so it is over the stored limit from about 2,500 levels
            // (the checks before it, none of which reads the nesting, are what the size gate is for).
            "nested as deep as the server is asked to take (where jsonpath may fail): over the decompressed limit",
            nested(deepest),
            if deepest >= 3_000 {
                vec![(0, true), (1, true)]
            } else {
                vec![(1, true)]
            },
        ),
        ("8 numbers of 1e131000", array_of(8, "1e131000"), vec![(0, false), (1, false), (2, true), (3, false)]),
        ("8 numbers of -1e131000", array_of(8, "-1e131000"), vec![(2, true), (3, false)]),
        ("8 numbers of 1e-16383", array_of(8, "1e-16383"), vec![(2, true), (3, false)]),
        ("1.5", "'{\"n\":1.5}'::jsonb".to_owned(), vec![(2, true), (3, false)]),
        ("one past the largest i64", "'{\"n\":9223372036854775808}'::jsonb".to_owned(), vec![(2, true)]),
        ("one past the smallest i64", "'{\"n\":-9223372036854775809}'::jsonb".to_owned(), vec![(2, true)]),
        (
            "a bare number, 1e131000",
            "'1e131000'::jsonb".to_owned(),
            vec![(0, false), (1, false), (2, true), (3, false)],
        ),
        ("16 numbers", array_of(15, "5"), vec![(2, false), (3, false)]),
        ("17 numbers", array_of(16, "5"), vec![(2, false), (3, true)]),
        (
            "10 zeros with the largest scale: integers, 160 KB printed",
            array_of(10, "0e-16383"),
            vec![(0, false), (1, false), (2, false), (3, false), (4, true)],
        ),
        (
            "3,000 control characters: 3 KB stored, 18 KB printed",
            "to_jsonb(repeat(chr(1), 3000))".to_owned(),
            vec![(0, false), (1, false), (2, false), (3, false), (4, true)],
        ),
        (
            "1,000 control characters: 6 KB printed",
            "to_jsonb(repeat(chr(1), 1000))".to_owned(),
            vec![(4, false)],
        ),
    ];
    for (id, (what, sql, _)) in cases.iter().enumerate() {
        sqlx::query(&format!("INSERT INTO vet_probe VALUES ({id}, {sql})"))
            .execute(&mut admin)
            .await
            .unwrap_or_else(|e| panic!("{what}: {e}"));
    }
    for (id, (what, _, checks)) in cases.iter().enumerate() {
        for (check, expected) in checks {
            let got: bool = sqlx::query_scalar(&format!(
                "SELECT {} FROM vet_probe WHERE id = {id}",
                reasons[*check]
            ))
            .fetch_one(&mut admin)
            .await
            .unwrap_or_else(|e| panic!("{what}: check {check}: {e}"));
            assert_eq!(got, *expected, "{what}: check {check} ({})", reasons[*check]);
        }
    }
}

#[tokio::test]
async fn the_largest_attribution_of_every_kind_of_actor_verifies_clean_and_is_far_under_the_bound() {
    let rig = AuditRig::new(db!()).await;
    let long = |unit: &str| domain::ShortText::parse(&unit.repeat(256 / unit.len())).unwrap();
    let actors = [
        domain::Actor::OsLogin { login: long("é") },
        domain::Actor::Job {
            job: domain::JobRef::new(&"a".repeat(253), &"b".repeat(64)).unwrap(),
        },
    ];
    for (i, actor) in actors.into_iter().enumerate() {
        let mut e = largest_event();
        e.at = domain::Timestamp::from_unix_millis(1_700_000_000_000 + i64::try_from(i).unwrap());
        if let Some(a) = e.attribution.as_mut() {
            a.actor = Some(actor);
            a.evidence.observed_at = Some(domain::Timestamp::from_unix_millis(i64::MIN));
        }
        rig.record_event(e).await;
    }
    rig.checkpoint_job().run_once().await.unwrap();
    let outcome = rig.nightly().run_once().await.unwrap();
    assert!(matches!(outcome, NightlyOutcome::Clean(_)), "{outcome:?}");
    // The sizes the database sees: at least ten times under the bound, the margin the module docs promise.
    let sizes: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT pg_column_size(attribution_actor #> '{}'), pg_column_size(attribution_evidence #> '{}') FROM audit_events",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap();
    assert_eq!(sizes.len(), 2);
    for (actor, evidence) in sizes {
        assert!(actor.max(evidence) * 10 <= 8192, "{actor} / {evidence} bytes");
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 4, N1: a position used twice is a finding, including when the two rows fall on either side of a batch edge.

const DROP_EVENTS_PK: &str = "ALTER TABLE audit_events DROP CONSTRAINT audit_events_pkey";
/// With no primary key, the order of rows that share a position is not promised. An ordinary index on the column orders them by
/// where they are stored, and a verifier that may not sort or scan the table (see `stored_order_verifier`) reads them that way,
/// which lets these tests say which row comes first: a row moved to an earlier position (an `UPDATE` writes a new copy, which is
/// stored after the old rows) comes second.
const INDEX_EVENTS_SEQ: &str = "CREATE INDEX audit_events_seq_idx ON audit_events (seq)";
const DROP_CHECKPOINTS_PK: &str = "ALTER TABLE audit_checkpoints DROP CONSTRAINT audit_checkpoints_pkey";
const INDEX_CHECKPOINTS_SEQ: &str = "CREATE INDEX audit_checkpoints_seq_idx ON audit_checkpoints (seq)";

/// A pool whose connections may neither sort nor scan a table, so that reads ordered by `seq` go through the index on it.
async fn stored_order_pool(rig: &AuditRig) -> sqlx::PgPool {
    use sqlx::postgres::PgPoolOptions;
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET enable_seqscan = off")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET enable_sort = off").execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect_with(rig.db.app_options())
        .await
        .unwrap()
}

fn verifier_on(rig: &AuditRig, pool: &sqlx::PgPool) -> hub_identity::audit::ChainVerifier {
    hub_identity::audit::ChainVerifier::new(pool.clone(), rig.bucket.clone(), rig.public_key())
}

/// A verifier whose connections may neither sort nor scan a table (see `stored_order_pool`).
async fn stored_order_verifier(rig: &AuditRig) -> hub_identity::audit::ChainVerifier {
    verifier_on(rig, &stored_order_pool(rig).await)
}

/// 12 events with checkpoints at 6 and 12, the primary key of `audit_events` gone and an index on `seq` in its place, and the
/// event at `from` moved to the position `to` (earlier), so that two rows have it and the chain is whole in the order they are read.
async fn rig_with_an_event_moved_up(db: support::pg::TestDb, from: i64, to: i64) -> AuditRig {
    let rig = rig_with_checkpoints(db).await;
    rig.tamper_open(false).await;
    rig.admin_sql(DROP_EVENTS_PK).await;
    rig.admin_sql(INDEX_EVENTS_SEQ).await;
    rig.admin_sql(&format!("UPDATE audit_events SET seq = {to} WHERE seq = {from}"))
        .await;
    rig
}

#[tokio::test]
async fn two_events_at_one_position_are_a_finding_even_when_the_chain_is_whole() {
    // The reviewer's scenario: one event moved onto the position of the one before it. The chain is intact in the order the rows
    // come back, every hash is right, every checkpoint is right, and only the repeated position shows.
    let rig = rig_with_an_event_moved_up(db!(), 10, 9).await;
    let verifier = stored_order_verifier(&rig).await;
    let outcome = rig.nightly_with(verifier).run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(
        report.failures,
        [failure(9, FailureKind::EventPositionDuplicated)]
    );
    assert_eq!(report.events_checked, 12);
    assert_eq!(report.checkpoints_checked, 2);
    assert_eq!(rig.metrics.audit_verify_failures_total(), 1);
    assert_eq!(rig.notifier.sent().len(), 1);
    assert!(
        rig.notifier.sent()[0].summary.as_str().contains("position 9"),
        "{:?}",
        rig.notifier.sent()[0].summary
    );
}

#[tokio::test]
async fn a_repeated_position_is_found_wherever_it_falls_against_the_edge_of_a_batch() {
    // For every size of batch from the smallest to one that holds the whole log, and for repeats at the start, in the middle and at
    // the end: the two rows are in one batch, or one at the end of a batch and the other at the start of the next, and either way
    // the repeat is found, once, with nothing else reported.
    for (from, to) in [(3, 2), (5, 4), (8, 7), (10, 9), (11, 10)] {
        let rig = rig_with_an_event_moved_up(db!(), from, to).await;
        // 0 and 1 are asked for and 2 is what is used: a batch of one row could never show a position used twice.
        let pool = stored_order_pool(&rig).await;
        for batch in [0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 5_000] {
            let verifier = verifier_on(&rig, &pool).with_batch_rows(batch);
            let report = tokio::time::timeout(Duration::from_secs(30), verifier.verify())
                .await
                .unwrap_or_else(|_| {
                    panic!("event {from} moved to {to}, batches of {batch}: the walk did not end")
                })
                .unwrap();
            assert_eq!(
                report.failures,
                [failure(to, FailureKind::EventPositionDuplicated)],
                "event {from} moved to {to}, batches of {batch}"
            );
            assert_eq!(
                report.events_checked, 12,
                "moved {from} to {to}, batches of {batch}"
            );
        }
    }
}

#[tokio::test]
async fn several_rows_at_one_position_are_found_and_the_walk_goes_on_when_the_batch_holds_them() {
    // Rows 4 and 5 moved to the position of 3: three rows have it, in the order they were written, so the chain is whole. A later
    // tamper is found whenever the three fit one read (a batch of at least three); in a smaller batch the repeat is still found,
    // the walk ends, and it does not ask for the same rows for ever.
    for batch in [2, 3, 4, 5, 100] {
        let rig = rig_with_checkpoints(db!()).await;
        rig.tamper_open(true).await;
        rig.admin_sql(DROP_EVENTS_PK).await;
        rig.admin_sql(INDEX_EVENTS_SEQ).await;
        rig.admin_sql("UPDATE audit_events SET seq = 3 WHERE seq IN (4, 5)")
            .await;
        rig.admin_sql(
            "UPDATE audit_events SET event_json = replace(event_json, 'settings_changed', 'role_changed') WHERE seq = 9",
        )
        .await;
        let verifier = stored_order_verifier(&rig).await.with_batch_rows(batch);
        let report = tokio::time::timeout(Duration::from_secs(30), verifier.verify())
            .await
            .unwrap_or_else(|_| panic!("batches of {batch}: the walk did not end"))
            .unwrap();
        assert!(
            report
                .failures
                .contains(&failure(3, FailureKind::EventPositionDuplicated)),
            "batches of {batch}: {:?}",
            report.failures
        );
        assert_eq!(
            report
                .failures
                .iter()
                .filter(|f| f.kind == FailureKind::EventPositionDuplicated)
                .count(),
            1,
            "batches of {batch}: reported once"
        );
        if batch >= 3 {
            assert_eq!(
                report.failures,
                [
                    failure(3, FailureKind::EventPositionDuplicated),
                    failure(9, FailureKind::ChainHashMismatch)
                ],
                "batches of {batch}"
            );
        }
    }
}

/// A checkpoint row for a position no event has, as the owner could write one (the foreign key is not enforced for the owner's
/// replication role).
fn insert_checkpoints_sql(from: i64, to: i64) -> String {
    format!(
        "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
         SELECT i, sha256(convert_to(i::text, 'UTF8')), 'v', '\\x3006010203040506', 'audit/checkpoints/' || i || '.json' \
         FROM generate_series({from}, {to}) i"
    )
}

#[tokio::test]
async fn two_checkpoints_at_one_position_are_a_finding() {
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql(DROP_CHECKPOINTS_PK).await;
    rig.admin_sql(INDEX_CHECKPOINTS_SEQ).await;
    // An exact copy of the checkpoint at 6: each row is, on its own, a good checkpoint.
    rig.admin_sql("INSERT INTO audit_checkpoints SELECT * FROM audit_checkpoints WHERE seq = 6")
        .await;
    let verifier = stored_order_verifier(&rig).await;
    let outcome = rig.nightly_with(verifier).run_once().await.unwrap();
    let NightlyOutcome::Failed(report) = outcome else {
        panic!("expected a failed run, got {outcome:?}");
    };
    assert_eq!(
        report.failures,
        [failure(6, FailureKind::CheckpointPositionDuplicated)]
    );
    assert_eq!(report.checkpoints_checked, 3);
    // Whatever the size of the batch (0 and 1 are 2: a batch of one row could never show a repeat), the repeat is found once, no
    // checkpoint is checked twice, and the one at 12 after it is still checked.
    let pool = stored_order_pool(&rig).await;
    for batch in [0, 1, 2, 3, 4, 500] {
        let report = tokio::time::timeout(
            Duration::from_secs(30),
            verifier_on(&rig, &pool)
                .with_checkpoint_batch_rows(batch)
                .verify(),
        )
        .await
        .unwrap_or_else(|_| panic!("checkpoint batches of {batch}: the walk did not end"))
        .unwrap();
        assert_eq!(
            report.failures,
            [failure(6, FailureKind::CheckpointPositionDuplicated)],
            "checkpoint batches of {batch}"
        );
        assert_eq!(report.checkpoints_checked, 3, "checkpoint batches of {batch}");
    }
}

#[tokio::test]
async fn two_checkpoints_at_one_position_are_found_across_the_edge_of_a_batch() {
    // The checkpoint batch is 500 rows. 600 rows for positions no event has (each is a finding of its own, in a list of its own),
    // and a copy of the 500th row in the order they are read, so that the copy is the first row of the second batch.
    let rig = rig_with_checkpoints(db!()).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql(DROP_CHECKPOINTS_PK).await;
    rig.admin_sql(INDEX_CHECKPOINTS_SEQ).await;
    rig.admin_sql(&insert_checkpoints_sql(1001, 1600)).await;
    // Positions 6 and 12 are the first two; 1001.. follow, so the 500th is 1001 + 497.
    rig.admin_sql("INSERT INTO audit_checkpoints SELECT * FROM audit_checkpoints WHERE seq = 1498")
        .await;
    let verifier = stored_order_verifier(&rig).await;
    let report = verifier.verify().await.unwrap();
    assert!(
        report
            .failures
            .contains(&failure(1498, FailureKind::CheckpointPositionDuplicated)),
        "{:?}",
        report
            .failures
            .iter()
            .filter(|f| f.kind == FailureKind::CheckpointPositionDuplicated)
            .collect::<Vec<_>>()
    );
    assert_eq!(report.checkpoints_checked, 603);
}

#[tokio::test]
async fn a_repeated_position_where_a_checkpoint_is_does_not_repeat_the_checkpoint() {
    // The checkpoint at 6 is joined to the event at 6. With two events there it is still one checkpoint, and a good one: the
    // first of the two is the event it signed. Only the repeated event position is a finding.
    let rig = rig_with_an_event_moved_up(db!(), 7, 6).await;
    let verifier = stored_order_verifier(&rig).await;
    let report = verifier.verify().await.unwrap();
    assert_eq!(
        report.failures,
        [failure(6, FailureKind::EventPositionDuplicated)]
    );
    assert_eq!(report.checkpoints_checked, 2);
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 4, N2: a change to the shape of the tables that blinds the verifier is a finding on the night it happens.
//
// An owner can retype or drop any column the verifier reads. Every query then fails, and until now the run ended `Errored` with no
// alert, the same as a lost connection. No fault of the database looks like that: the server says so with SQLSTATE class 42
// (undefined column, undefined function, datatype mismatch ...), and the run is reported as a finding and alerts that night.

/// Drop every `CHECK`, foreign key and unique constraint of the two audit tables, as an owner does before retyping a column they
/// mean to retype (the checks name the old types).
const DEMOLISH_CONSTRAINTS: &str = "DO $$ DECLARE r record; BEGIN \
     FOR r IN SELECT conrelid::regclass AS t, conname FROM pg_constraint \
              WHERE conrelid IN ('audit_events'::regclass, 'audit_checkpoints'::regclass) AND contype IN ('c', 'f', 'u', 'p') \
              ORDER BY (contype = 'f') DESC LOOP \
       EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', r.t, r.conname); \
     END LOOP; END $$";

/// What the nightly run does after a change to the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Becomes {
    /// `Incomplete`, with a `SchemaChanged` finding: the query was refused.
    Refused,
    /// `Failed`, with a column named as differing: the query still ran, and the stored value can no longer be read as what it is.
    UnreadableColumn(Column),
    /// `Clean`: the type changed and every value is what it was.
    Unchanged,
}

async fn after_ddl(db: support::pg::TestDb, statements: &[String]) -> (AuditRig, NightlyOutcome) {
    let rig = rig_with_checkpoints(db).await;
    // A full event and a bare one, so that the optional columns hold values and `NULL`s.
    rig.record_event(full_event(100)).await;
    rig.record_event(bare_event(101)).await;
    rig.tamper_open(false).await;
    rig.tamper_open_checkpoints().await;
    rig.admin_sql(DEMOLISH_CONSTRAINTS).await;
    for statement in statements {
        rig.admin_sql(statement).await;
    }
    let outcome = rig.nightly().run_once().await.unwrap();
    (rig, outcome)
}

fn assert_becomes(rig: &AuditRig, what: &str, outcome: &NightlyOutcome, expected: Becomes) {
    match (expected, outcome) {
        (Becomes::Refused, NightlyOutcome::Incomplete(report)) => {
            assert!(
                report
                    .failures
                    .iter()
                    .any(|f| matches!(f.kind, FailureKind::SchemaChanged(_)) && f.seq == 0),
                "{what}: {:?}",
                report.failures
            );
            assert_eq!(rig.metrics.audit_verify_errors_total(), 1, "{what}");
            assert!(rig.metrics.audit_verify_failures_total() >= 1, "{what}");
            assert_eq!(
                rig.notifier.sent().len(),
                1,
                "{what}: the admins are told at once"
            );
            let sent = rig.notifier.sent();
            let summary = sent[0].summary.as_str();
            assert!(
                summary.contains("shape") && summary.contains("did not finish"),
                "{what}: {summary}"
            );
        }
        (Becomes::UnreadableColumn(column), NightlyOutcome::Failed(report)) => {
            assert!(
                report
                    .failures
                    .iter()
                    .all(|f| matches!(f.kind, FailureKind::ColumnsDiffer(set) if set.contains(column))),
                "{what}: {:?}",
                report.failures
            );
            assert!(!report.failures.is_empty(), "{what}");
            assert_eq!(rig.notifier.sent().len(), 1, "{what}");
            assert_eq!(rig.metrics.audit_verify_errors_total(), 0, "{what}");
        }
        (Becomes::Unchanged, NightlyOutcome::Clean(_)) => {
            assert!(rig.notifier.sent().is_empty(), "{what}");
        }
        (expected, outcome) => panic!("{what}: expected {expected:?}, got {outcome:?}"),
    }
}

/// The columns of `audit_events` the verifier reads, besides the ones it compares.
const CHAIN_COLUMNS: [&str; 4] = ["seq", "prev_hash", "hash", "event_json"];

#[tokio::test]
async fn retyping_a_column_of_the_events_to_int_is_a_finding_at_once() {
    let columns: Vec<&str> = CHAIN_COLUMNS
        .into_iter()
        .chain(Column::ALL.iter().map(|c| c.name()))
        .collect();
    for column in columns {
        // `seq` keeps its values (a primary key of zeros cannot be built); the others become 0.
        let using = if column == "seq" { "seq::int" } else { "0" };
        let (rig, outcome) = after_ddl(
            db!(),
            &[format!(
                "ALTER TABLE audit_events ALTER COLUMN {column} TYPE int USING {using}"
            )],
        )
        .await;
        // `approval_id` is the one column that survives the change to the query: a bigint becomes an int, the select works, and the
        // value cannot be read as the bigint it was.
        let expected = if column == "approval_id" {
            Becomes::UnreadableColumn(Column::ApprovalId)
        } else {
            Becomes::Refused
        };
        assert_becomes(&rig, &format!("{column} to int"), &outcome, expected);
    }
}

#[tokio::test]
async fn dropping_a_column_of_the_events_is_a_finding_at_once() {
    let columns: Vec<&str> = CHAIN_COLUMNS
        .into_iter()
        .chain(Column::ALL.iter().map(|c| c.name()))
        .collect();
    for column in columns {
        let (rig, outcome) = after_ddl(
            db!(),
            &[format!("ALTER TABLE audit_events DROP COLUMN {column} CASCADE")],
        )
        .await;
        assert_becomes(&rig, &format!("drop {column}"), &outcome, Becomes::Refused);
    }
}

#[tokio::test]
async fn retyping_or_dropping_a_column_of_the_checkpoints_is_a_finding_at_once() {
    for column in ["seq", "head_hash", "kms_key_version", "signature", "gcs_object"] {
        let using = if column == "seq" { "seq::int" } else { "0" };
        let (rig, outcome) = after_ddl(
            db!(),
            &[format!(
                "ALTER TABLE audit_checkpoints ALTER COLUMN {column} TYPE int USING {using}"
            )],
        )
        .await;
        assert_becomes(
            &rig,
            &format!("checkpoint {column} to int"),
            &outcome,
            Becomes::Refused,
        );
        let (rig, outcome) = after_ddl(
            db!(),
            &[format!(
                "ALTER TABLE audit_checkpoints DROP COLUMN {column} CASCADE"
            )],
        )
        .await;
        assert_becomes(
            &rig,
            &format!("checkpoint drop {column}"),
            &outcome,
            Becomes::Refused,
        );
    }
}

#[tokio::test]
async fn other_retypes_are_a_finding_or_clean_by_what_the_values_are_afterwards() {
    for (what, statements, expected) in [
        (
            "at to text: isfinite(text) does not exist",
            vec!["ALTER TABLE audit_events ALTER COLUMN at TYPE text USING at::text".to_owned()],
            Becomes::Refused,
        ),
        (
            "a jsonb column to text: the operator on it does not exist",
            vec![
                "ALTER TABLE audit_events ALTER COLUMN attribution_evidence TYPE text USING attribution_evidence::text".to_owned(),
            ],
            Becomes::Refused,
        ),
        (
            "a text column to bytea: the select works, the value cannot be read as text",
            vec![
                "ALTER TABLE audit_events ALTER COLUMN actor_ref TYPE bytea USING convert_to(actor_ref, 'UTF8')".to_owned(),
            ],
            Becomes::UnreadableColumn(Column::ActorRef),
        ),
        (
            "a bigint column to numeric: the select works, the value cannot be read as a bigint",
            vec![
                "ALTER TABLE audit_events ALTER COLUMN approval_id TYPE numeric".to_owned(),
            ],
            Becomes::UnreadableColumn(Column::ApprovalId),
        ),
        (
            "a text column to varchar: every value is what it was, so nothing was tampered with",
            vec![
                "ALTER TABLE audit_events ALTER COLUMN actor_ref TYPE varchar".to_owned(),
                "ALTER TABLE audit_events ALTER COLUMN request_id TYPE varchar(100)".to_owned(),
            ],
            Becomes::Unchanged,
        ),
        (
            "at to a date: the select works, and the times are no longer the events' times",
            vec!["ALTER TABLE audit_events ALTER COLUMN at TYPE date".to_owned()],
            Becomes::UnreadableColumn(Column::At),
        ),
    ] {
        let (rig, outcome) = after_ddl(db!(), &statements).await;
        assert_becomes(&rig, what, &outcome, expected);
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Round 4, N5: the admins hear when the verification has not completed for N nights in a row.
//
// A run that ends in an error (or finds something and then cannot finish) says nothing to the admins that night: a lost
// connection or a lock is usually gone by the next. But a verifier that errors every night verifies nothing, and used to say so to
// nobody. The job now counts such nights, in memory (a restart resets the count: see the module docs of `audit::verify`), and from
// the second in a row tells the admins, every night until a run completes.

mod nights {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use hub_identity::audit::NightlyVerifyJob;
    use ports::{LeaseBackend, LeaseError, LeaseGuard, Leases};
    use tokio::time::Instant;

    use super::AuditRig;

    /// A lease that every night's run gets, so that a test can run the job night after night without waiting for a lease to run
    /// out. Once `lose` is set, the next renewal fails: the run loses its lease.
    ///
    /// Once `elsewhere` is set, another replica holds the lease: the run is `NotLeader`.
    #[derive(Default)]
    pub struct EveryNight {
        pub lose: Arc<AtomicBool>,
        pub elsewhere: Arc<AtomicBool>,
    }

    struct Backend {
        lose: Arc<AtomicBool>,
    }

    #[async_trait]
    impl LeaseBackend for Backend {
        async fn renew(&self, _: &str, _: u64, ttl: Duration) -> Result<Instant, LeaseError> {
            if self.lose.load(Ordering::SeqCst) {
                Err(LeaseError::Lost)
            } else {
                Ok(Instant::now() + ttl)
            }
        }

        async fn release(&self, _: &str, _: u64) -> Result<(), LeaseError> {
            Ok(())
        }
    }

    #[async_trait]
    impl Leases for EveryNight {
        async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
            if self.elsewhere.load(Ordering::SeqCst) {
                return Ok(None);
            }
            Ok(Some(LeaseGuard::new(
                name.to_owned(),
                1,
                Instant::now() + ttl,
                Arc::new(Backend {
                    lose: self.lose.clone(),
                }),
            )))
        }
    }

    /// A job that runs every time it is asked, and gives up on a locked table after 200 ms.
    pub fn job(rig: &AuditRig, leases: &Arc<EveryNight>) -> NightlyVerifyJob {
        NightlyVerifyJob::new(
            rig.chain_verifier()
                .with_timeouts(Duration::from_secs(5), Duration::from_millis(200)),
            leases.clone(),
            Arc::new(rig.notifier.clone()),
            rig.metrics.clone(),
        )
    }
}

/// An owner holds the table, so that every read of it times out on its lock.
async fn lock_table(rig: &AuditRig, table: &str) -> sqlx::PgConnection {
    let mut owner = rig.db.admin().await;
    sqlx::query("BEGIN").execute(&mut owner).await.unwrap();
    sqlx::query(&format!("LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE"))
        .execute(&mut owner)
        .await
        .unwrap();
    owner
}

async fn lock_events(rig: &AuditRig) -> sqlx::PgConnection {
    lock_table(rig, "audit_events").await
}

async fn unlock(owner: &mut sqlx::PgConnection) {
    sqlx::query("ROLLBACK").execute(&mut *owner).await.unwrap();
}

fn unfinished(rig: &AuditRig) -> u64 {
    rig.metrics.audit_verify_unfinished_runs_in_a_row()
}

#[tokio::test]
async fn the_second_unfinished_night_in_a_row_alerts_and_the_first_does_not() {
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default());
    let mut owner = lock_events(&rig).await;

    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert!(rig.notifier.sent().is_empty(), "one night is not enough to alert");
    assert_eq!(unfinished(&rig), 1);

    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 2);
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 1, "the second night alerts");
    assert_eq!(sent[0].kind, NotificationKind::CriticalFinding);
    assert_eq!(sent[0].severity, Severity::Critical);
    assert_eq!(sent[0].title.as_str(), "Audit log verification is not completing");
    // The headline, the count and the class of the error: no text of the database's, no value.
    assert_eq!(
        sent[0].summary.as_str(),
        "Audit log verification could not complete 2 nights in a row (last error: the audit tables did not answer in time)."
    );

    // And every night after, until a run completes.
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 3);
    let sent = rig.notifier.sent();
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1].summary.as_str().contains("3 nights in a row"),
        "{:?}",
        sent[1].summary
    );
    assert_eq!(rig.metrics.audit_verify_errors_total(), 3);
    assert_eq!(rig.metrics.audit_verify_runs_total(), 0);

    unlock(&mut owner).await;
    // A run that completes ends it, and says nothing.
    assert!(matches!(job.run_once().await.unwrap(), NightlyOutcome::Clean(_)));
    assert_eq!(unfinished(&rig), 0);
    assert_eq!(rig.notifier.sent().len(), 2);
}

#[tokio::test]
async fn a_clean_run_resets_the_count() {
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default());
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 1);
    unlock(&mut owner).await;
    assert!(matches!(job.run_once().await.unwrap(), NightlyOutcome::Clean(_)));
    assert_eq!(unfinished(&rig), 0);
    // One unfinished night after a completed one is the first again: no alert.
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 1);
    assert!(rig.notifier.sent().is_empty());
    unlock(&mut owner).await;
}

#[tokio::test]
async fn a_failed_run_resets_the_count_too() {
    // A run that found something ran to the end and has told the admins; it is a verdict.
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default());
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    unlock(&mut owner).await;
    rig.tamper_open(true).await;
    rig.admin_sql(TAMPER_5).await;
    assert!(matches!(job.run_once().await.unwrap(), NightlyOutcome::Failed(_)));
    assert_eq!(unfinished(&rig), 0);
    assert_eq!(rig.notifier.sent().len(), 1, "the finding, once");
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 1);
    assert_eq!(
        rig.notifier.sent().len(),
        1,
        "no alert for the first unfinished night after a verdict"
    );
    unlock(&mut owner).await;
}

#[tokio::test]
async fn an_incomplete_run_counts_as_an_unfinished_night() {
    // Night 1: an error. Night 2: the run finds a problem (the shape of the tables) and cannot finish. Two in a row.
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default());
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    unlock(&mut owner).await;
    rig.admin_sql("ALTER TABLE audit_events RENAME COLUMN via TO via_renamed")
        .await;
    assert!(matches!(
        job.run_once().await.unwrap(),
        NightlyOutcome::Incomplete(_)
    ));
    assert_eq!(unfinished(&rig), 2);
    let sent = rig.notifier.sent();
    // The finding that night, which says the tables changed shape, and the count of nights.
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent[0].summary.as_str().contains("shape"),
        "{:?}",
        sent[0].summary
    );
    assert_eq!(
        sent[1].summary.as_str(),
        "Audit log verification could not complete 2 nights in a row (last error: the audit tables changed shape, SQLSTATE 42703)."
    );
}

#[tokio::test]
async fn a_changed_shape_alerts_on_the_first_night_without_waiting_for_a_second() {
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default());
    rig.admin_sql("ALTER TABLE audit_events DROP COLUMN github_login")
        .await;
    assert!(matches!(
        job.run_once().await.unwrap(),
        NightlyOutcome::Incomplete(_)
    ));
    assert_eq!(unfinished(&rig), 1);
    assert_eq!(
        rig.notifier.sent().len(),
        1,
        "the finding, at once, and not the count"
    );
    assert_eq!(rig.metrics.audit_verify_schema_changes_total(), 1);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert!(rig.metrics.audit_verify_failures_total() >= 1);
    // The Prometheus side: the counter that alerts at once, and the gauge.
    let names: Vec<_> = rig
        .metrics
        .counters()
        .into_iter()
        .filter(|c| c.value > 0)
        .map(|c| c.name)
        .collect();
    assert!(names.contains(&"audit_verify_schema_changes_total"), "{names:?}");
    assert_eq!(
        rig.metrics
            .gauges()
            .into_iter()
            .map(|g| (g.name, g.value))
            .collect::<Vec<_>>(),
        [("audit_verify_unfinished_runs_in_a_row", 1)]
    );
}

#[tokio::test]
async fn a_run_that_lost_its_lease_neither_counts_nor_resets() {
    // Another replica holds the lease then and decides the night: this one's run is abandoned, and says nothing of the database.
    use std::sync::atomic::Ordering;
    let rig = rig_with_checkpoints(db!()).await;
    let leases = Arc::new(nights::EveryNight::default());
    let job = nights::job(&rig, &leases).with_lease_renew_every(Duration::from_millis(50));
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 1);
    unlock(&mut owner).await;

    // The same job, but its lease is lost at the first renewal; the table is locked for longer than the 50 ms between renewals (the
    // job's verifier waits 200 ms for the lock), so the run is still going when it is.
    leases.lose.store(true, Ordering::SeqCst);
    let mut owner = lock_events(&rig).await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), job.run_once())
        .await
        .expect("the run was abandoned when the lease was lost")
        .unwrap();
    assert_eq!(outcome, NightlyOutcome::LeaseLost);
    assert_eq!(
        unfinished(&rig),
        1,
        "a lost lease neither counts (2) nor resets (0)"
    );
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert!(rig.notifier.sent().is_empty());
    unlock(&mut owner).await;

    // And the next night with the lease goes on from where the count was: the second unfinished night.
    leases.lose.store(false, Ordering::SeqCst);
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 2, "the next unfinished night is the second");
    assert_eq!(rig.notifier.sent().len(), 1);
    unlock(&mut owner).await;
}

#[tokio::test]
async fn a_replica_that_did_not_hold_the_lease_neither_counts_nor_resets() {
    // The same job, so that its own count is what is tested: another replica holds the lease for one night (`NotLeader`), and the
    // gauge stays where it was (not 2, not 0), and the next unfinished night is the second.
    use std::sync::atomic::Ordering;
    let rig = rig_with_checkpoints(db!()).await;
    let leases = Arc::new(nights::EveryNight::default());
    let job = nights::job(&rig, &leases);
    let mut owner = lock_events(&rig).await;
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 1);
    leases.elsewhere.store(true, Ordering::SeqCst);
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::NotLeader);
    assert_eq!(unfinished(&rig), 1, "neither counted (2) nor reset (0)");
    assert_eq!(rig.metrics.audit_verify_errors_total(), 1);
    assert!(rig.notifier.sent().is_empty());
    leases.elsewhere.store(false, Ordering::SeqCst);
    assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(unfinished(&rig), 2, "the next unfinished night is the second");
    assert_eq!(rig.notifier.sent().len(), 1);
    unlock(&mut owner).await;
}

#[tokio::test]
async fn the_number_of_nights_is_configurable_and_never_below_one() {
    let rig = rig_with_checkpoints(db!()).await;
    let three = nights::job(&rig, &Arc::<nights::EveryNight>::default()).with_alert_after(3);
    let mut owner = lock_events(&rig).await;
    for night in 1..=2 {
        assert_eq!(three.run_once().await.unwrap(), NightlyOutcome::Errored);
        assert!(rig.notifier.sent().is_empty(), "night {night}");
    }
    assert_eq!(three.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.notifier.sent().len(), 1);
    assert!(
        rig.notifier.sent()[0]
            .summary
            .as_str()
            .contains("3 nights in a row")
    );
    unlock(&mut owner).await;

    // Zero is one: every unfinished night alerts.
    let rig = rig_with_checkpoints(db!()).await;
    let every = nights::job(&rig, &Arc::<nights::EveryNight>::default()).with_alert_after(0);
    let mut owner = lock_events(&rig).await;
    assert_eq!(every.run_once().await.unwrap(), NightlyOutcome::Errored);
    assert_eq!(rig.notifier.sent().len(), 1);
    assert_eq!(
        rig.notifier.sent()[0].summary.as_str(),
        "Audit log verification could not complete 1 night in a row (last error: the audit tables did not answer in time)."
    );
    unlock(&mut owner).await;
    assert_eq!(hub_identity::audit::DEFAULT_ALERT_AFTER_FAILED_RUNS, 2);
}

#[tokio::test]
async fn a_notifier_that_is_down_does_not_stop_the_count_or_the_run() {
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
    let job = hub_identity::audit::NightlyVerifyJob::new(
        rig.chain_verifier()
            .with_timeouts(Duration::from_secs(5), Duration::from_millis(200)),
        Arc::new(nights::EveryNight::default()),
        Arc::new(Broken),
        rig.metrics.clone(),
    );
    let mut owner = lock_events(&rig).await;
    for _ in 0..3 {
        assert_eq!(job.run_once().await.unwrap(), NightlyOutcome::Errored);
    }
    // The count and the counter are what Prometheus reads when the notifier cannot be reached.
    assert_eq!(unfinished(&rig), 3);
    assert_eq!(rig.metrics.audit_verify_errors_total(), 3);
    unlock(&mut owner).await;
}

#[tokio::test]
async fn a_renewal_period_of_zero_is_a_millisecond_and_not_a_panic() {
    // `tokio::time::interval` panics on a zero period; the test knob clamps it.
    let rig = rig_with_checkpoints(db!()).await;
    let job = nights::job(&rig, &Arc::<nights::EveryNight>::default()).with_lease_renew_every(Duration::ZERO);
    assert!(matches!(job.run_once().await.unwrap(), NightlyOutcome::Clean(_)));
}
