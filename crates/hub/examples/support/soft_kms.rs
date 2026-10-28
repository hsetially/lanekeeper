//! A KMS with software keys, for the dev example and the tests (plan 18, T3). NOT for production: the real hub never holds a CA's
//! private key, and nothing under `src/` names this file (`tests/release_build.rs` keeps it that way).
//!
//! It signs with ECDSA P-256 over the SHA-256 digest it is given, which is what Cloud KMS `asymmetricSign` does, so a certificate
//! signed through it verifies like one signed by KMS. It also gives the public half of a key, as `hub ca-init` asks for it.
//! Included with `#[path]` by `examples/hub_dev.rs` and by the tests; it is not a module of the library.
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
use hub::edges::PublicKeys;
use ports::{KeyRef, KmsError, KmsSigner, Signature};

pub struct SoftKms {
    keys: HashMap<String, EcdsaKeyPair>,
}

impl fmt::Debug for SoftKms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoftKms").field("keys", &self.keys.len()).finish()
    }
}

impl SoftKms {
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
impl KmsSigner for SoftKms {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        let pair = self.keys.get(key.as_str()).ok_or(KmsError::KeyNotFound)?;
        let digest = Digest::import_less_safe(&digest, &SHA256).map_err(|_| KmsError::Invalid)?;
        let signature = pair.sign_digest(&digest).map_err(|_| KmsError::Unavailable)?;
        Ok(Signature::new(Bytes::copy_from_slice(signature.as_ref())))
    }
}

#[async_trait]
impl PublicKeys for SoftKms {
    async fn spki_der(&self, key: &KeyRef) -> Result<Vec<u8>, KmsError> {
        self.public_key_spki_der(key).ok_or(KmsError::KeyNotFound)
    }
}
