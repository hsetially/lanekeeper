//! The hourly checkpoint job (S9).
//!
//! Every hour, one replica (the holder of the `audit.checkpoint` lease) signs the head of the chain with KMS, writes the
//! signed checkpoint to the locked bucket, and records it in `audit_checkpoints`:
//!
//! 1. read the head `(seq, hash)`; nothing to do when the log is empty or the newest checkpoint already covers it;
//! 2. the digest of [`checkpoint_message`](super::signature::checkpoint_message) goes to [`KmsSigner::sign_digest`];
//! 3. the object `audit/checkpoints/<seq>.json` (`{seq, head, signature, key_version, created_at}`) is created with a
//!    create-only upload;
//! 4. the row is inserted. Its foreign key `(seq, head_hash)` refers to `audit_events`, so a checkpoint of a head that is not in
//!    the chain cannot be recorded.
//!
//! # Restart safety
//!
//! A replica can stop between 3 and 4, or two can race after a lease expiry. The upload then finds the object already there. The
//! job reads it and, when its `seq` and `head` are the ones being checkpointed, adopts it (its signature and key version go into
//! the row): the bucket copy is the authority, so the two stores cannot disagree. An object at this `seq` with another `head` is
//! a conflict and an error: the chain was different when that checkpoint was made, which is exactly what the check is for.
//! ECDSA signatures are randomised, so "the same bytes" is not the test; "the same `seq` and `head`" is.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use domain::{ContentHash, Timestamp};
use ports::{KeyRef, KmsError, KmsSigner, LeaseError, Leases};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::{PgPool, Row};

use super::JobHandle;
use super::bucket::{BucketError, CheckpointBucket, MAX_OBJECT_BYTES, PutOutcome, object_name};
use super::signature::checkpoint_digest;
use crate::clock::Clock;
use crate::metrics::IdentityMetrics;

/// The lease that makes the job single-holder across replicas.
pub const LEASE_NAME: &str = "audit.checkpoint";
/// How long the lease lasts. A checkpoint takes well under a minute; the rest is slack for a slow bucket.
const LEASE_TTL: Duration = Duration::from_secs(300);
/// KMS gets this long to sign.
const SIGN_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest signature stored: an ECDSA P-256 signature is at most 72 bytes in DER.
const MAX_SIGNATURE_BYTES: usize = 128;

/// Why a checkpoint was not made. No variant carries a hash, a signature or an object name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CheckpointError {
    #[error("lease store failed: {0}")]
    Lease(#[from] LeaseError),
    /// The lease ran out before the work was done; another replica may be doing it.
    #[error("the checkpoint lease was lost")]
    LeaseLost,
    #[error("the audit table could not be read or written")]
    Database,
    #[error("kms failed: {0}")]
    Kms(#[from] KmsError),
    #[error("kms did not answer in time")]
    KmsTimeout,
    #[error("bucket failed: {0}")]
    Bucket(#[from] BucketError),
    /// The bucket holds a checkpoint for this `seq` with another head, or one that cannot be read.
    #[error("the bucket holds a different checkpoint for this position")]
    Conflict,
    #[error("the checkpoint could not be encoded")]
    Encode,
}

/// The object stored in the bucket for one checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointObject {
    pub seq: u64,
    pub head: ContentHash,
    #[serde(with = "signature_text")]
    pub signature: Vec<u8>,
    pub key_version: KeyRef,
    pub created_at: Timestamp,
}

/// The signature as base64url text.
mod signature_text {
    use super::{Deserialize, Deserializer, Engine, MAX_SIGNATURE_BYTES, Serializer, URL_SAFE_NO_PAD};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() > MAX_SIGNATURE_BYTES * 2 {
            return Err(serde::de::Error::custom("signature is too long"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(text.as_bytes())
            .map_err(|_| serde::de::Error::custom("signature is not base64url"))?;
        if bytes.is_empty() || bytes.len() > MAX_SIGNATURE_BYTES {
            return Err(serde::de::Error::custom("signature has a wrong length"));
        }
        Ok(bytes)
    }
}

impl CheckpointObject {
    /// The bytes written to the bucket.
    pub fn encode(&self) -> Result<Bytes, CheckpointError> {
        serde_json::to_vec(self)
            .map(Bytes::from)
            .map_err(|_| CheckpointError::Encode)
    }

    /// Read an object from the bucket. The bytes are untrusted: a size limit, strict fields, a bounded signature.
    pub fn parse(bytes: &[u8]) -> Result<Self, CheckpointError> {
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(CheckpointError::Encode);
        }
        serde_json::from_slice(bytes).map_err(|_| CheckpointError::Encode)
    }
}

/// What one run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointOutcome {
    /// A checkpoint of the head at `seq` was written.
    Written { seq: u64 },
    /// The newest checkpoint already covers the head.
    Unchanged,
    /// The log has no event yet.
    Empty,
    /// Another replica holds the lease.
    NotLeader,
}

/// What the job needs.
pub struct CheckpointDeps {
    pub pool: PgPool,
    pub leases: Arc<dyn Leases>,
    pub signer: Arc<dyn KmsSigner>,
    /// The KMS key version that signs (a crypto key version resource name).
    pub key: KeyRef,
    pub bucket: Arc<dyn CheckpointBucket>,
    pub clock: Arc<dyn Clock>,
    pub metrics: Arc<IdentityMetrics>,
}

impl std::fmt::Debug for CheckpointDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointDeps")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

pub struct CheckpointJob {
    deps: CheckpointDeps,
}

impl std::fmt::Debug for CheckpointJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointJob")
            .field("key", &self.deps.key)
            .finish_non_exhaustive()
    }
}

fn db_error(stage: &'static str, e: &sqlx::Error) -> CheckpointError {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned));
    tracing::warn!(
        stage,
        sqlstate = code.as_deref().unwrap_or("none"),
        "checkpoint database call failed"
    );
    CheckpointError::Database
}

impl CheckpointJob {
    pub fn new(deps: CheckpointDeps) -> Self {
        Self { deps }
    }

    /// Make a checkpoint now if this replica holds the lease and the head moved.
    pub async fn run_once(&self) -> Result<CheckpointOutcome, CheckpointError> {
        let Some(lease) = self.deps.leases.try_acquire(LEASE_NAME, LEASE_TTL).await? else {
            return Ok(CheckpointOutcome::NotLeader);
        };
        let outcome = self.checkpoint(&lease).await;
        if let Err(e) = lease.release().await {
            tracing::debug!(error = %e, "checkpoint lease was not released");
        }
        outcome
    }

    async fn checkpoint(&self, lease: &ports::LeaseGuard) -> Result<CheckpointOutcome, CheckpointError> {
        let head = sqlx::query("SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1")
            .fetch_optional(&self.deps.pool)
            .await
            .map_err(|e| db_error("head", &e))?;
        let Some(head) = head else {
            return Ok(CheckpointOutcome::Empty);
        };
        let seq: i64 = head.try_get("seq").map_err(|e| db_error("head", &e))?;
        let hash: Vec<u8> = head.try_get("hash").map_err(|e| db_error("head", &e))?;
        let hash =
            ContentHash::from_bytes(<[u8; 32]>::try_from(hash).map_err(|_| CheckpointError::Database)?);
        let seq = u64::try_from(seq).map_err(|_| CheckpointError::Database)?;

        let last: Option<i64> = sqlx::query_scalar("SELECT max(seq) FROM audit_checkpoints")
            .fetch_one(&self.deps.pool)
            .await
            .map_err(|e| db_error("last", &e))?;
        if last.is_some_and(|last| last >= i64::try_from(seq).unwrap_or(i64::MAX)) {
            return Ok(CheckpointOutcome::Unchanged);
        }

        let digest = checkpoint_digest(seq, &hash);
        let signature = tokio::time::timeout(
            SIGN_TIMEOUT,
            self.deps.signer.sign_digest(self.deps.key.clone(), digest),
        )
        .await
        .map_err(|_| CheckpointError::KmsTimeout)??;
        let mut object = CheckpointObject {
            seq,
            head: hash,
            signature: signature.as_bytes().to_vec(),
            key_version: self.deps.key.clone(),
            created_at: self.deps.clock.now(),
        };
        if object.signature.is_empty() || object.signature.len() > MAX_SIGNATURE_BYTES {
            return Err(CheckpointError::Kms(KmsError::Invalid));
        }

        if !lease.is_valid() {
            return Err(CheckpointError::LeaseLost);
        }
        let name = object_name(seq);
        if self.deps.bucket.put(&name, object.encode()?).await? == PutOutcome::AlreadyExists {
            // An earlier attempt got this far. The bucket copy is the authority.
            let existing = self
                .deps
                .bucket
                .get(&name)
                .await?
                .ok_or(CheckpointError::Conflict)?;
            let existing = CheckpointObject::parse(&existing).map_err(|_| CheckpointError::Conflict)?;
            if existing.seq != seq || existing.head != hash {
                tracing::error!("the bucket holds a checkpoint for this position with a different head");
                return Err(CheckpointError::Conflict);
            }
            object = existing;
        }

        if !lease.is_valid() {
            return Err(CheckpointError::LeaseLost);
        }
        sqlx::query(
            "INSERT INTO audit_checkpoints (seq, head_hash, kms_key_version, signature, gcs_object) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (seq) DO NOTHING",
        )
        .bind(i64::try_from(seq).map_err(|_| CheckpointError::Database)?)
        .bind(hash.as_bytes().as_slice())
        .bind(object.key_version.as_str())
        .bind(&object.signature)
        .bind(&name)
        .execute(&self.deps.pool)
        .await
        .map_err(|e| db_error("insert", &e))?;
        self.deps.metrics.checkpoint_written();
        Ok(CheckpointOutcome::Written { seq })
    }

    /// Run [`CheckpointJob::run_once`] every `every` (with a little jitter) until the handle is dropped.
    pub fn spawn(self: Arc<Self>, every: Duration) -> JobHandle {
        JobHandle::spawn(async move {
            loop {
                tokio::time::sleep(super::jittered(every)).await;
                match self.run_once().await {
                    Ok(CheckpointOutcome::Written { seq }) => tracing::info!(seq, "audit checkpoint written"),
                    Ok(other) => tracing::debug!(outcome = ?other, "audit checkpoint skipped"),
                    Err(e) => tracing::error!(error = %e, "audit checkpoint failed"),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object() -> CheckpointObject {
        CheckpointObject {
            seq: 42,
            head: ContentHash::from_bytes([7; 32]),
            signature: vec![1, 2, 3, 250],
            key_version: KeyRef::parse("projects/p/keys/k/versions/1").unwrap(),
            created_at: Timestamp::from_unix_millis(1_700_000_000_000),
        }
    }

    #[test]
    fn the_object_round_trips_and_has_the_documented_fields() {
        let o = object();
        let bytes = o.encode().unwrap();
        assert_eq!(CheckpointObject::parse(&bytes).unwrap(), o);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["created_at", "head", "key_version", "seq", "signature"]);
        assert_eq!(v["head"].as_str().unwrap(), "07".repeat(32));
    }

    #[test]
    fn parse_refuses_what_it_does_not_write() {
        let good = String::from_utf8(object().encode().unwrap().to_vec()).unwrap();
        let mut cases = vec![
            String::new(),
            "{}".to_owned(),
            good.replace("\"seq\":42", "\"seq\":-1"),
            good.replace("\"seq\":42", "\"seq\":42,\"extra\":1"),
            good.replace("\"signature\":\"AQID-g\"", "\"signature\":\"\""),
            good.replace("\"signature\":\"AQID-g\"", "\"signature\":\"!!!\""),
            good.replace("07070707", "zz"),
            "x".repeat(MAX_OBJECT_BYTES + 1),
        ];
        cases.push(good.replace("AQID-g", &"A".repeat(400)));
        for bad in cases {
            assert!(CheckpointObject::parse(bad.as_bytes()).is_err(), "{bad:.80}");
        }
    }
}
