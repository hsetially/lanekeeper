//! The CA's signing key as rcgen sees it: a public key and a `sign` that asks KMS (S5).
//!
//! There is no private key in this type, and none anywhere in the hub: `sign` hashes the message and sends the digest to
//! [`KmsSigner::sign_digest`]. rcgen's signing hook is synchronous, so the async KMS call is driven with
//! [`Handle::block_on`] from a thread that is already blocking (`CaIssuer` calls it only inside `spawn_blocking`).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aws_lc_rs::digest::{SHA256, digest};
use ports::{KeyRef, KmsError, KmsSigner};
use rcgen::{PKCS_ECDSA_P256_SHA256, PublicKeyData, SignatureAlgorithm, SigningKey};
use tokio::runtime::Handle;

/// One KMS signature is bounded, whatever the signer does (rule 5). The Google client has its own, shorter deadline.
const SIGN_TIMEOUT: Duration = Duration::from_secs(30);

pub struct KmsSigningKey {
    signer: Arc<dyn KmsSigner>,
    key: KeyRef,
    /// The uncompressed P-256 point from the CA certificate.
    point: Vec<u8>,
    runtime: Handle,
    /// Why the last signature failed, because rcgen's error type cannot say. One key serves one certificate, so there is
    /// no other signature to confuse it with.
    failure: Mutex<Option<KmsError>>,
}

impl fmt::Debug for KmsSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KmsSigningKey")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl KmsSigningKey {
    /// `point` is the CA certificate's public key; `runtime` is where the KMS call runs.
    pub(crate) fn new(signer: Arc<dyn KmsSigner>, key: KeyRef, point: Vec<u8>, runtime: Handle) -> Self {
        Self {
            signer,
            key,
            point,
            runtime,
            failure: Mutex::new(None),
        }
    }

    /// The KMS error behind a failed `sign`, if there was one.
    pub(crate) fn take_failure(&self) -> Option<KmsError> {
        self.failure.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl PublicKeyData for KmsSigningKey {
    fn der_bytes(&self) -> &[u8] {
        &self.point
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

impl SigningKey for KmsSigningKey {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        let mut sha = [0u8; 32];
        sha.copy_from_slice(digest(&SHA256, msg).as_ref());
        let call = tokio::time::timeout(SIGN_TIMEOUT, self.signer.sign_digest(self.key.clone(), sha));
        // Cloud KMS returns the DER `ECDSA-Sig-Value` that X.509 wants, so the bytes go into the certificate as they are.
        let failed = |error: KmsError| {
            *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
            Err(rcgen::Error::RingUnspecified)
        };
        match self.runtime.block_on(call) {
            Ok(Ok(signature)) => Ok(signature.as_bytes().to_vec()),
            Ok(Err(error)) => failed(error),
            Err(_elapsed) => failed(KmsError::Unavailable),
        }
    }
}
