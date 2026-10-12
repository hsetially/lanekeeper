//! T5 (S9): hourly KMS-signed checkpoints, written to the bucket and to `audit_checkpoints`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{ContentHash, Timestamp};
use hub_identity::audit::bucket::{CheckpointBucket, PutOutcome, object_name};
use hub_identity::audit::checkpoint::{CheckpointError, CheckpointObject, CheckpointOutcome};
use hub_identity::audit::{CheckpointVerifier, EcdsaP256Verifier, checkpoint_message};
use ports::fakes::FakeKmsSigner;
use ports::{KmsError, Leases};
use sqlx::Row;
use support::audit_rig::{AuditRig, VERSION};
use support::ecdsa_kms::EcdsaKms;

async fn checkpoint_rows(rig: &AuditRig) -> Vec<(i64, ContentHash, String, Vec<u8>, String)> {
    sqlx::query(
        "SELECT seq, head_hash, kms_key_version, signature, gcs_object FROM audit_checkpoints ORDER BY seq",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| {
        (
            r.get("seq"),
            ContentHash::from_bytes(r.get::<Vec<u8>, _>("head_hash").try_into().unwrap()),
            r.get("kms_key_version"),
            r.get("signature"),
            r.get("gcs_object"),
        )
    })
    .collect()
}

async fn head(rig: &AuditRig) -> (i64, ContentHash) {
    let row = sqlx::query("SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    (
        row.get("seq"),
        ContentHash::from_bytes(row.get::<Vec<u8>, _>("hash").try_into().unwrap()),
    )
}

#[tokio::test]
async fn checkpoint_signs_head_writes_bucket_and_row() {
    let rig = AuditRig::new(db!()).await;
    let last = rig.record(5).await;
    let (seq, hash) = head(&rig).await;
    assert_eq!(seq, last);

    let outcome = rig.checkpoint_job().run_once().await.unwrap();
    assert_eq!(
        outcome,
        CheckpointOutcome::Written {
            seq: u64::try_from(seq).unwrap()
        }
    );

    // The bucket has the object, with the documented content.
    assert_eq!(rig.bucket.names(), [object_name(u64::try_from(seq).unwrap())]);
    let stored = rig.bucket.get(&rig.bucket.names()[0]).await.unwrap().unwrap();
    let object = CheckpointObject::parse(&stored).unwrap();
    assert_eq!(object.seq, u64::try_from(seq).unwrap());
    assert_eq!(object.head, hash);
    assert_eq!(object.key_version.as_str(), VERSION);
    assert_eq!(object.created_at, Timestamp::from_unix_millis(1_791_676_900_000));

    // The table has the row, equal to the object.
    let rows = checkpoint_rows(&rig).await;
    assert_eq!(rows.len(), 1);
    let (row_seq, row_head, version, signature, name) = &rows[0];
    assert_eq!((*row_seq, *row_head), (seq, hash));
    assert_eq!(version, VERSION);
    assert_eq!(signature, &object.signature);
    assert_eq!(name, &object_name(u64::try_from(seq).unwrap()));
    assert_eq!(rig.kms.signatures_made(), 1);
    assert_eq!(rig.metrics.audit_checkpoints_written_total(), 1);
    // The lease was given back.
    assert!(
        rig.leases
            .try_acquire("audit.checkpoint", Duration::from_secs(5))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn checkpoint_is_skipped_when_head_unchanged() {
    let rig = AuditRig::new(db!()).await;
    let job = rig.checkpoint_job();
    assert_eq!(
        job.run_once().await.unwrap(),
        CheckpointOutcome::Empty,
        "nothing to sign yet"
    );
    rig.record(3).await;
    assert!(matches!(
        job.run_once().await.unwrap(),
        CheckpointOutcome::Written { .. }
    ));
    assert_eq!(job.run_once().await.unwrap(), CheckpointOutcome::Unchanged);
    assert_eq!(
        rig.kms.signatures_made(),
        1,
        "no second signature for the same head"
    );
    assert_eq!(rig.bucket.len(), 1);
    rig.record(1).await;
    assert!(matches!(
        job.run_once().await.unwrap(),
        CheckpointOutcome::Written { .. }
    ));
    assert_eq!(rig.bucket.len(), 2);
    assert_eq!(checkpoint_rows(&rig).await.len(), 2);
}

#[tokio::test]
async fn checkpoint_requires_the_lease() {
    let rig = AuditRig::new(db!()).await;
    rig.record(2).await;
    // Another replica holds the lease.
    let other = rig
        .leases
        .try_acquire("audit.checkpoint", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rig.checkpoint_job().run_once().await.unwrap(),
        CheckpointOutcome::NotLeader
    );
    assert_eq!(rig.kms.signatures_made(), 0);
    assert!(rig.bucket.is_empty());
    assert!(checkpoint_rows(&rig).await.is_empty());
    other.release().await.unwrap();
    assert!(matches!(
        rig.checkpoint_job().run_once().await.unwrap(),
        CheckpointOutcome::Written { .. }
    ));
}

#[tokio::test]
async fn checkpoint_restart_after_bucket_write_is_idempotent() {
    let rig = AuditRig::new(db!()).await;
    rig.record(3).await;
    let (seq, hash) = head(&rig).await;
    // An earlier attempt wrote the object and stopped before the insert. Its signature differs from the one a new attempt makes
    // (ECDSA is randomised), and its time is earlier.
    let earlier = CheckpointObject {
        seq: u64::try_from(seq).unwrap(),
        head: hash,
        signature: vec![0x30, 0x06, 1, 2, 3, 4, 5, 6],
        key_version: EcdsaKms::key(),
        created_at: Timestamp::from_unix_millis(1),
    };
    let name = object_name(earlier.seq);
    assert_eq!(
        rig.bucket.put(&name, earlier.encode().unwrap()).await.unwrap(),
        PutOutcome::Created
    );

    let outcome = rig.checkpoint_job().run_once().await.unwrap();
    assert_eq!(outcome, CheckpointOutcome::Written { seq: earlier.seq });
    // The bucket copy is the authority: the row carries its signature, and the object is untouched.
    let rows = checkpoint_rows(&rig).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].3, earlier.signature);
    let stored = CheckpointObject::parse(&rig.bucket.get(&name).await.unwrap().unwrap()).unwrap();
    assert_eq!(stored, earlier);
    assert_eq!(rig.bucket.len(), 1);
}

#[tokio::test]
async fn checkpoint_conflicting_bucket_object_is_an_error() {
    let rig = AuditRig::new(db!()).await;
    rig.record(3).await;
    let (seq, _) = head(&rig).await;
    // The bucket already holds a checkpoint for this position that signed a different head: the chain was different then.
    let other = CheckpointObject {
        seq: u64::try_from(seq).unwrap(),
        head: ContentHash::from_bytes([9; 32]),
        signature: vec![1; 8],
        key_version: EcdsaKms::key(),
        created_at: Timestamp::from_unix_millis(1),
    };
    rig.bucket
        .put(&object_name(other.seq), other.encode().unwrap())
        .await
        .unwrap();
    let err = rig.checkpoint_job().run_once().await.unwrap_err();
    assert_eq!(err, CheckpointError::Conflict);
    assert!(
        checkpoint_rows(&rig).await.is_empty(),
        "nothing recorded for a conflicting position"
    );
    assert_eq!(rig.metrics.audit_checkpoints_written_total(), 0);
}

#[tokio::test]
async fn a_kms_failure_writes_nothing() {
    let rig = AuditRig::new(db!()).await;
    rig.record(2).await;
    // A signer that knows no key.
    let job = rig.checkpoint_job_with(Arc::new(FakeKmsSigner::new(&[])), rig.bucket.clone());
    assert_eq!(
        job.run_once().await.unwrap_err(),
        CheckpointError::Kms(KmsError::KeyNotFound)
    );
    assert!(rig.bucket.is_empty());
    assert!(checkpoint_rows(&rig).await.is_empty());
    // The lease is back, so a healthy replica can go on.
    assert!(matches!(
        rig.checkpoint_job().run_once().await.unwrap(),
        CheckpointOutcome::Written { .. }
    ));
}

#[tokio::test]
async fn a_bucket_that_is_down_records_no_row() {
    struct Down;
    #[async_trait::async_trait]
    impl CheckpointBucket for Down {
        async fn put(&self, _: &str, _: Bytes) -> Result<PutOutcome, hub_identity::audit::BucketError> {
            Err(hub_identity::audit::BucketError::Unavailable)
        }
        async fn get(&self, _: &str) -> Result<Option<Bytes>, hub_identity::audit::BucketError> {
            Err(hub_identity::audit::BucketError::Unavailable)
        }
    }
    let rig = AuditRig::new(db!()).await;
    rig.record(2).await;
    let job = rig.checkpoint_job_with(rig.kms.clone(), Arc::new(Down));
    assert!(matches!(
        job.run_once().await.unwrap_err(),
        CheckpointError::Bucket(_)
    ));
    // A checkpoint that exists only in the table would defeat its purpose: the second copy must exist first.
    assert!(checkpoint_rows(&rig).await.is_empty());
}

#[tokio::test]
async fn checkpoint_signature_verifies_with_public_key() {
    let rig = AuditRig::new(db!()).await;
    rig.record(4).await;
    rig.checkpoint_job().run_once().await.unwrap();
    let (seq, hash, version, signature, _) = checkpoint_rows(&rig).await.remove(0);
    let message = checkpoint_message(u64::try_from(seq).unwrap(), &hash);
    let public = EcdsaP256Verifier::new()
        .with_spki_der(&version, &rig.kms.spki_der())
        .unwrap();
    assert!(public.verify(&version, &message, &signature));
    // Not for another position or another head.
    assert!(!public.verify(
        &version,
        &checkpoint_message(u64::try_from(seq).unwrap() + 1, &hash),
        &signature
    ));
    assert!(!public.verify(
        &version,
        &checkpoint_message(u64::try_from(seq).unwrap(), &ContentHash::from_bytes([0; 32])),
        &signature
    ));
}

#[tokio::test]
async fn checkpoint_signature_by_other_key_fails() {
    let rig = AuditRig::new(db!()).await;
    rig.record(4).await;
    rig.checkpoint_job().run_once().await.unwrap();
    let (seq, hash, version, signature, _) = checkpoint_rows(&rig).await.remove(0);
    let message = checkpoint_message(u64::try_from(seq).unwrap(), &hash);
    // The public key of some other P-256 key.
    let other = EcdsaKms::new(rig.pool.clone());
    let public = EcdsaP256Verifier::new()
        .with_spki_der(&version, &other.spki_der())
        .unwrap();
    assert!(!public.verify(&version, &message, &signature));
}
