//! The agent CA (S5, S6): certificates built by rcgen and signed through [`ports::KmsSigner`].
//!
//! The CA's private key lives in Cloud KMS and never leaves it. This module has no code path that reads, loads or holds a
//! private CA key: [`CaIssuer::new`] takes the signer, the key's *name* and the CA *certificate* (public), and a signature
//! is a call to [`ports::KmsSigner::sign_digest`]. `tests/ca_key_use.rs` fails if a private-key constructor appears anywhere
//! in the hub's source.
//!
//! - [`profile`]: what a certificate says (identity SAN, lifetime, use).
//! - [`csr`]: the request, as proof of possession of a P-256 key and nothing else.
//! - [`kms_key`]: the signing key rcgen sees, which asks KMS.
//! - [`ceremony`]: the one-off creation of the CA certificate (`hub ca-init`).

pub mod ceremony;
pub mod csr;
mod kms_key;
pub mod profile;

use std::sync::Arc;
use std::time::Duration;

use aws_lc_rs::digest::{Digest, SHA256};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use bytes::Bytes;
use domain::Timestamp;
use ports::{KeyRef, KmsError, KmsSigner};
use rcgen::{Issuer, SerialNumber};
use rustls::pki_types::CertificateDer;
use tokio::sync::Semaphore;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::clock::{Clock, SystemClock};
pub use csr::{Csr, CsrError, parse_csr};
use kms_key::KmsSigningKey;
pub use profile::{CertKind, CertProfile, PeerName, ProfileError};

/// How many certificates are signed at once. Each one is a KMS call on a blocking thread, so the bound protects both.
const MAX_CONCURRENT_ISSUES: usize = 8;
/// A whole issuance, from the request to the certificate.
const ISSUE_TIMEOUT: Duration = Duration::from_secs(45);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a certificate could not be issued. A `Display` message never carries a name, a key or request bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaError {
    #[error("the CA certificate is not a usable CA certificate")]
    BadCaCertificate,
    #[error("the CA certificate has expired")]
    CaExpired,
    /// The CA certificate's key is not the one KMS signs with under the configured key name.
    #[error("the CA certificate does not belong to the KMS key")]
    KeyMismatch,
    #[error("the certificate profile is not valid")]
    Profile(#[from] ProfileError),
    #[error("kms could not sign the certificate")]
    Kms(KmsError),
    #[error("the certificate could not be built")]
    Build,
    #[error("issuing took too long")]
    Timeout,
}

/// A certificate the CA issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCertificate {
    /// DER, leaf first, then the CA certificate.
    pub chain_der: Vec<Bytes>,
    /// The serial number, lower-case hex, as stored in `agents.cert_serial`.
    pub serial: String,
    /// The CA's clock when the certificate was issued: the start of its life as far as renewal is concerned (`notBefore` is
    /// backdated for clock skew).
    pub issued_at: Timestamp,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

/// Issues certificates signed by a KMS key. Cheap to share: clone the `Arc`.
pub struct CaIssuer {
    signer: Arc<dyn KmsSigner>,
    key: KeyRef,
    ca_der: Vec<u8>,
    /// The CA certificate's public point, which is what a signature must verify under.
    point: Vec<u8>,
    clock: Arc<dyn Clock>,
    gate: Semaphore,
}

impl std::fmt::Debug for CaIssuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaIssuer")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl CaIssuer {
    /// Start an issuer over `key` in KMS and the CA certificate made for it by the ceremony.
    ///
    /// The certificate must be a CA certificate for a P-256 key, and KMS must sign with that key under `key`: the hub signs
    /// one probe digest and verifies it against the certificate (A8). A mismatch means every certificate issued would fail
    /// to verify, so the hub refuses to start.
    pub async fn new(signer: Arc<dyn KmsSigner>, key: KeyRef, ca_cert_der: Vec<u8>) -> Result<Self, CaError> {
        let issuer = Self::new_unchecked(signer, key, ca_cert_der)?;
        issuer.probe().await?;
        Ok(issuer)
    }

    /// Like [`CaIssuer::new`], without asking KMS to sign anything. For tests that need an issuer over a signer that fails.
    #[cfg(any(test, feature = "fakes"))]
    pub fn new_unchecked(
        signer: Arc<dyn KmsSigner>,
        key: KeyRef,
        ca_cert_der: Vec<u8>,
    ) -> Result<Self, CaError> {
        Self::build(signer, key, ca_cert_der)
    }

    #[cfg(not(any(test, feature = "fakes")))]
    fn new_unchecked(signer: Arc<dyn KmsSigner>, key: KeyRef, ca_cert_der: Vec<u8>) -> Result<Self, CaError> {
        Self::build(signer, key, ca_cert_der)
    }

    fn build(signer: Arc<dyn KmsSigner>, key: KeyRef, ca_cert_der: Vec<u8>) -> Result<Self, CaError> {
        let (rest, cert) = X509Certificate::from_der(&ca_cert_der).map_err(|_| CaError::BadCaCertificate)?;
        if !rest.is_empty() {
            return Err(CaError::BadCaCertificate);
        }
        let is_ca = cert
            .basic_constraints()
            .ok()
            .flatten()
            .is_some_and(|ext| ext.value.ca);
        let may_sign = cert
            .key_usage()
            .ok()
            .flatten()
            .is_some_and(|ext| ext.value.key_cert_sign());
        if !is_ca || !may_sign {
            return Err(CaError::BadCaCertificate);
        }
        let point = csr::p256_point(cert.public_key()).ok_or(CaError::BadCaCertificate)?;
        Ok(Self {
            signer,
            key,
            ca_der: ca_cert_der,
            point,
            clock: Arc::new(SystemClock),
            gate: Semaphore::new(MAX_CONCURRENT_ISSUES),
        })
    }

    /// Use `clock` for certificate times. For tests, and for nothing else.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The CA certificate, DER. Public: it is what peers trust.
    #[must_use]
    pub fn ca_certificate_der(&self) -> &[u8] {
        &self.ca_der
    }

    /// Whether the CA certificate is still within its validity at `clock.now()`.
    fn check_not_expired(&self) -> Result<(), CaError> {
        let (_, cert) = X509Certificate::from_der(&self.ca_der).map_err(|_| CaError::BadCaCertificate)?;
        let now_s = self.clock.now().unix_millis().div_euclid(1000);
        if now_s >= cert.validity().not_after.timestamp() {
            return Err(CaError::CaExpired);
        }
        Ok(())
    }

    async fn probe(&self) -> Result<(), CaError> {
        self.check_not_expired()?;
        let mut challenge = [0u8; 32];
        SystemRandom::new()
            .fill(&mut challenge)
            .map_err(|_| CaError::Build)?;
        let digest = aws_lc_rs::digest::digest(&SHA256, &challenge);
        let mut sha = [0u8; 32];
        sha.copy_from_slice(digest.as_ref());
        let signature = tokio::time::timeout(PROBE_TIMEOUT, self.signer.sign_digest(self.key.clone(), sha))
            .await
            .map_err(|_| CaError::Kms(KmsError::Unavailable))?
            .map_err(CaError::Kms)?;
        let digest = Digest::import_less_safe(&sha, &SHA256).map_err(|_| CaError::Build)?;
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &self.point)
            .verify_digest(&digest, signature.as_bytes())
            .map_err(|_| CaError::KeyMismatch)
    }

    /// Issue the certificate `profile` describes for the key in `csr`. The request contributes its public key and nothing
    /// else.
    pub async fn issue(&self, profile: &CertProfile, csr: &Csr) -> Result<IssuedCertificate, CaError> {
        let _permit = self.gate.acquire().await.map_err(|_| CaError::Build)?;
        self.check_not_expired()?;
        let now = self.clock.now();
        let serial = random_serial()?;
        let serial_hex = hex::encode(serial.to_bytes());
        let params = profile.params(now, serial)?;
        let (not_before, not_after) = (
            Timestamp::from_unix_millis(params.not_before.unix_timestamp().saturating_mul(1000)),
            Timestamp::from_unix_millis(params.not_after.unix_timestamp().saturating_mul(1000)),
        );
        let public_key = csr.public_key().clone();
        let signer = Arc::clone(&self.signer);
        let key = self.key.clone();
        let point = self.point.clone();
        let ca_der = self.ca_der.clone();
        let runtime = tokio::runtime::Handle::current();
        // rcgen signs synchronously, and the signature is a network call to KMS: both belong on a blocking thread.
        let work = tokio::task::spawn_blocking(move || {
            let signing_key = KmsSigningKey::new(signer, key, point, runtime);
            let issuer = Issuer::from_ca_cert_der(&CertificateDer::from(ca_der.as_slice()), signing_key)
                .map_err(|_| CaError::BadCaCertificate)?;
            match params.signed_by(&public_key, &issuer) {
                Ok(certificate) => Ok(certificate.der().to_vec()),
                Err(_) => Err(issuer.key().take_failure().map_or(CaError::Build, CaError::Kms)),
            }
        });
        let leaf = tokio::time::timeout(ISSUE_TIMEOUT, work)
            .await
            .map_err(|_| CaError::Timeout)?
            .map_err(|_| CaError::Build)??;
        Ok(IssuedCertificate {
            chain_der: vec![Bytes::from(leaf), Bytes::copy_from_slice(&self.ca_der)],
            serial: serial_hex,
            issued_at: now,
            not_before,
            not_after,
        })
    }
}

/// A random positive serial of 20 bytes (RFC 5280). rcgen's default is derived from the public key, which would repeat for
/// a renewal that keeps its key.
fn random_serial() -> Result<SerialNumber, CaError> {
    let mut bytes = [0u8; 20];
    SystemRandom::new().fill(&mut bytes).map_err(|_| CaError::Build)?;
    // Top bit clear (positive), next bit set (the length stays 20).
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    Ok(SerialNumber::from_slice(&bytes))
}
