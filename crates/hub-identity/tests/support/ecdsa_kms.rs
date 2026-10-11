//! A `KmsSigner` backed by a real P-256 key, so that a checkpoint signature can be checked with the public key.
//!
//! `ports::fakes::FakeKmsSigner` signs with a keyed hash, which no public key can verify. This signer makes real ECDSA
//! signatures. `KmsSigner::sign_digest` gets only the SHA-256 digest, and aws-lc-rs signs messages, not raw digests, so the
//! signer finds the message the digest stands for: it recomputes the checkpoint digest of the newest events in the audit table
//! and signs the message whose digest matches. A digest that belongs to no checkpoint of the chain is refused, which is also
//! what a real KMS key policy would be asked to do.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use bytes::Bytes;
use domain::ContentHash;
use hub_identity::audit::{checkpoint_digest, checkpoint_message};
use ports::{KeyRef, KmsError, KmsSigner, Signature};
use sqlx::{PgPool, Row};

pub struct EcdsaKms {
    pair: EcdsaKeyPair,
    pool: PgPool,
    signed: AtomicUsize,
}

impl EcdsaKms {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pair: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap(),
            pool,
            signed: AtomicUsize::new(0),
        }
    }

    /// The `SubjectPublicKeyInfo` of the key, as KMS publishes it.
    pub fn spki_der(&self) -> Vec<u8> {
        self.pair.public_key().as_der().unwrap().as_ref().to_vec()
    }

    pub fn signatures_made(&self) -> usize {
        self.signed.load(Ordering::SeqCst)
    }

    pub fn key() -> KeyRef {
        KeyRef::parse("projects/lk/locations/eu/keyRings/audit/cryptoKeys/checkpoint/cryptoKeyVersions/1")
            .unwrap()
    }
}

#[async_trait]
impl KmsSigner for EcdsaKms {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        if key != Self::key() {
            return Err(KmsError::KeyNotFound);
        }
        let rows = sqlx::query("SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1000")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| KmsError::Unavailable)?;
        for row in rows {
            let seq: i64 = row.get("seq");
            let hash: Vec<u8> = row.get("hash");
            let head = ContentHash::from_bytes(hash.try_into().unwrap());
            let seq = u64::try_from(seq).unwrap();
            if checkpoint_digest(seq, &head) == digest {
                let sig = self.pair.sign(
                    &aws_lc_rs::rand::SystemRandom::new(),
                    &checkpoint_message(seq, &head),
                );
                self.signed.fetch_add(1, Ordering::SeqCst);
                return Ok(Signature::new(Bytes::copy_from_slice(
                    sig.map_err(|_| KmsError::Invalid)?.as_ref(),
                )));
            }
        }
        Err(KmsError::Invalid)
    }
}
