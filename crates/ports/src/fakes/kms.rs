//! Fake KMS. NOT cryptography: these only mimic the contract (key lookup, associated-data binding,
//! tamper detection, fresh ciphertext per wrap) so tests can exercise callers.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::{KeyRef, KmsEnvelope, KmsError, KmsSigner, Signature, WrappedKey};

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Signs with a keyed hash of the digest, for the keys it was given.
#[derive(Debug, Clone)]
pub struct FakeKmsSigner {
    keys: HashSet<KeyRef>,
}

impl FakeKmsSigner {
    pub fn new(keys: &[KeyRef]) -> Self {
        Self {
            keys: keys.iter().cloned().collect(),
        }
    }

    fn sig(key: &KeyRef, digest: &[u8; 32]) -> [u8; 32] {
        sha(&[b"lk-fake-kms-sig", key.as_str().as_bytes(), digest])
    }

    /// Check a signature made by this fake.
    pub fn verify(&self, key: &KeyRef, digest: &[u8; 32], sig: &Signature) -> bool {
        self.keys.contains(key) && bool::from(Self::sig(key, digest).as_slice().ct_eq(sig.as_bytes()))
    }
}

#[async_trait]
impl KmsSigner for FakeKmsSigner {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        if !self.keys.contains(&key) {
            return Err(KmsError::KeyNotFound);
        }
        Ok(Signature::new(Bytes::copy_from_slice(&Self::sig(&key, &digest))))
    }
}

const MASTER: &[u8] = b"lanekeeper-fake-kms-master-key";
const NONCE: usize = 8;
const BODY: usize = 32;
const TAG: usize = 32;
const KEY_VERSION: &str = "fake/kms/keys/envelope/versions/1";

/// Wraps a data key as `nonce || (key xor pad) || tag`, where the tag covers the associated data.
#[derive(Debug, Default)]
pub struct FakeKmsEnvelope {
    counter: AtomicU64,
}

impl FakeKmsEnvelope {
    pub fn new() -> Self {
        Self::default()
    }

    fn tag(nonce: &[u8], aad: &[u8], body: &[u8]) -> [u8; 32] {
        sha(&[
            MASTER,
            b"tag",
            nonce,
            &(aad.len() as u64).to_be_bytes(),
            aad,
            body,
        ])
    }
}

#[async_trait]
impl KmsEnvelope for FakeKmsEnvelope {
    async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError> {
        // Associated data is what binds a ciphertext to its owner (S7): refuse to wrap without it.
        if aad.is_empty() {
            return Err(KmsError::Invalid);
        }
        let nonce = self.counter.fetch_add(1, Ordering::Relaxed).to_be_bytes();
        let pad = sha(&[MASTER, b"pad", &nonce]);
        let mut body = [0u8; BODY];
        for (i, b) in body.iter_mut().enumerate() {
            *b = dek.expose()[i] ^ pad[i];
        }
        let tag = Self::tag(&nonce, aad, &body);
        let mut out = Vec::with_capacity(NONCE + BODY + TAG);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&body);
        out.extend_from_slice(&tag);
        Ok(WrappedKey {
            key_version: KeyRef::parse(KEY_VERSION).map_err(|_| KmsError::Invalid)?,
            ciphertext: Bytes::from(out),
        })
    }

    async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError> {
        let ct = &w.ciphertext;
        if w.key_version.as_str() != KEY_VERSION || ct.len() != NONCE + BODY + TAG || aad.is_empty() {
            return Err(KmsError::Decrypt);
        }
        let (nonce, rest) = ct.split_at(NONCE);
        let (body, tag) = rest.split_at(BODY);
        if !bool::from(Self::tag(nonce, aad, body).as_slice().ct_eq(tag)) {
            return Err(KmsError::Decrypt);
        }
        let pad = sha(&[MASTER, b"pad", nonce]);
        let mut dek = [0u8; 32];
        for (i, b) in dek.iter_mut().enumerate() {
            *b = body[i] ^ pad[i];
        }
        Ok(Secret::new(dek))
    }
}
