//! The one-off creation of a CA certificate (`hub ca-init`, A8).
//!
//! The CA key is generated inside KMS and never leaves it. The ceremony reads its public key (the inherent
//! `CloudKmsSigner::public_key`), builds a self-signed CA certificate and has KMS sign it through [`ports::KmsSigner`], the
//! same path every issued certificate takes. The result is public: it goes into Secret Manager or a `ConfigMap`, and the
//! runtime loads only that.

use std::sync::Arc;
use std::time::Duration;

use domain::Timestamp;
use ports::{KeyRef, KmsSigner};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyUsagePurpose, SerialNumber,
};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer;
use x509_parser::x509::SubjectPublicKeyInfo;

use super::{CaError, kms_key::KmsSigningKey};
use crate::ca::csr::p256_point;

/// A self-signed CA certificate for the KMS key `key`, whose public key is `spki_der` (`SubjectPublicKeyInfo`, DER), valid
/// from `now` for `valid_for`. It may sign leaf certificates and no other CA (path length 0). Returns the DER.
pub async fn self_signed_ca(
    signer: Arc<dyn KmsSigner>,
    key: KeyRef,
    spki_der: &[u8],
    common_name: &str,
    now: Timestamp,
    valid_for: Duration,
) -> Result<Vec<u8>, CaError> {
    let (rest, spki) = SubjectPublicKeyInfo::from_der(spki_der).map_err(|_| CaError::BadCaCertificate)?;
    let (true, Some(point)) = (rest.is_empty(), p256_point(&spki)) else {
        return Err(CaError::BadCaCertificate);
    };
    let at = |ms: i64| OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).map_err(|_| CaError::Build);
    let valid_ms = i64::try_from(valid_for.as_millis()).unwrap_or(i64::MAX);
    let mut params = CertificateParams::default();
    params.not_before = at(now.unix_millis())?;
    params.not_after = at(now.unix_millis().saturating_add(valid_ms))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.serial_number = Some(SerialNumber::from_slice(&[0x40, 0x01]));
    let runtime = tokio::runtime::Handle::current();
    let work = tokio::task::spawn_blocking(move || {
        let signing_key = KmsSigningKey::new(signer, key, point, runtime);
        match params.self_signed(&signing_key) {
            Ok(certificate) => Ok(certificate.der().to_vec()),
            Err(_) => Err(signing_key.take_failure().map_or(CaError::Build, CaError::Kms)),
        }
    });
    work.await.map_err(|_| CaError::Build)?
}
