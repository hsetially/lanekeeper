//! A KMS signer with real keys, for tests that need a signature to verify (the CA tests in T3 use it too).
//!
//! The `ports` fake is a keyed hash and is not cryptography; this one signs with ECDSA P-256 over the digest it is given,
//! exactly what Cloud KMS `asymmetricSign` does, and can verify what it signed.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use aws_lc_rs::digest::{Digest, SHA256};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair, UnparsedPublicKey,
};
use bytes::Bytes;
use ports::{KeyRef, KmsError, KmsSigner, Signature};

pub struct SoftKmsSigner {
    keys: HashMap<String, EcdsaKeyPair>,
}

impl fmt::Debug for SoftKmsSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoftKmsSigner")
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl SoftKmsSigner {
    /// One fresh key for each name.
    pub fn new(names: &[&KeyRef]) -> Self {
        let keys = names
            .iter()
            .map(|name| {
                let pkcs8 =
                    EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
                        .unwrap();
                let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
                (name.as_str().to_owned(), pair)
            })
            .collect();
        Self { keys }
    }

    pub fn has(&self, key: &KeyRef) -> bool {
        self.keys.contains_key(key.as_str())
    }

    /// `SubjectPublicKeyInfo`, DER.
    pub fn public_key_spki_der(&self, key: &KeyRef) -> Option<Vec<u8>> {
        let pair = self.keys.get(key.as_str())?;
        let der = pair.public_key().as_der().unwrap();
        Some(der.as_ref().to_vec())
    }

    /// Whether `signature` is this key's signature over `digest`.
    pub fn verify(&self, key: &KeyRef, digest: &[u8; 32], signature: &[u8]) -> bool {
        let Some(pair) = self.keys.get(key.as_str()) else {
            return false;
        };
        let digest = Digest::import_less_safe(digest, &SHA256).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, pair.public_key().as_ref())
            .verify_digest(&digest, signature)
            .is_ok()
    }
}

#[async_trait]
impl KmsSigner for SoftKmsSigner {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        let pair = self.keys.get(key.as_str()).ok_or(KmsError::KeyNotFound)?;
        let digest = Digest::import_less_safe(&digest, &SHA256).map_err(|_| KmsError::Invalid)?;
        let signature = pair.sign_digest(&digest).map_err(|_| KmsError::Unavailable)?;
        Ok(Signature::new(Bytes::copy_from_slice(signature.as_ref())))
    }
}
