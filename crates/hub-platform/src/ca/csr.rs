//! The certificate signing request (S5, S11): proof of possession of a P-256 key, and nothing else.
//!
//! A request is untrusted bytes from the network. It is read for two things only: the public key, and the signature that
//! shows its sender holds the matching private key. Its subject, its SANs and every extension it asks for are ignored (the
//! hub decides the identity, [`super::profile::CertProfile`]). Anything that is not exactly one DER request, not P-256 or not
//! signed by its own key is refused before a certificate is considered.

use rcgen::{CertificateSigningRequestParams, PublicKey};
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256};
use x509_parser::prelude::FromDer;
use x509_parser::x509::SubjectPublicKeyInfo;

pub use proto::limits::MAX_CSR_BYTES;

/// Why a request was refused. Never carries any of its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CsrError {
    #[error("the request is larger than {MAX_CSR_BYTES} bytes")]
    TooLarge,
    #[error("the request is not one well-formed PKCS#10 structure")]
    Malformed,
    #[error("the request's key is not ECDSA P-256 with SHA-256")]
    UnsupportedKey,
    #[error("the request is not signed by its own key")]
    BadSignature,
}

/// A request that passed every check: it holds a P-256 key and proved possession of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Csr {
    public_key: PublicKey,
}

impl Csr {
    /// The public key to certify, for [`super::CaIssuer::issue`].
    #[must_use]
    pub fn public_key(&self) -> &PublicKey {
        &self.public_key
    }

    /// The 65 bytes of the uncompressed P-256 point.
    #[must_use]
    pub fn public_key_point(&self) -> &[u8] {
        rcgen::PublicKeyData::der_bytes(&self.public_key)
    }
}

/// Parse and check a DER certificate signing request.
pub fn parse_csr(der: &[u8]) -> Result<Csr, CsrError> {
    if der.len() > MAX_CSR_BYTES {
        return Err(CsrError::TooLarge);
    }
    let (rest, request) = X509CertificationRequest::from_der(der).map_err(|_| CsrError::Malformed)?;
    if !rest.is_empty() {
        return Err(CsrError::Malformed);
    }
    // The key and the signature algorithm are checked here, from the request's own fields, and not left to the library to
    // infer: a P-384 key with a SHA-256 signature OID must not become a certificate that claims P-256.
    if request.signature_algorithm.algorithm != OID_SIG_ECDSA_WITH_SHA256 {
        return Err(CsrError::UnsupportedKey);
    }
    if p256_point(&request.certification_request_info.subject_pki).is_none() {
        return Err(CsrError::UnsupportedKey);
    }
    // rcgen checks the self-signature (proof of possession) and hands back the key in the form it signs certificates for.
    let params =
        CertificateSigningRequestParams::from_der(&der.to_vec().into()).map_err(|error| match error {
            rcgen::Error::InvalidCertificationRequestSignature => CsrError::BadSignature,
            rcgen::Error::UnsupportedSignatureAlgorithm => CsrError::UnsupportedKey,
            _ => CsrError::Malformed,
        })?;
    Ok(Csr {
        public_key: params.public_key,
    })
}

/// The uncompressed point of an `id-ecPublicKey` key on `prime256v1`, or `None` for any other key.
pub(crate) fn p256_point(spki: &SubjectPublicKeyInfo<'_>) -> Option<Vec<u8>> {
    if spki.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY {
        return None;
    }
    let curve = spki.algorithm.parameters.as_ref()?.as_oid().ok()?;
    if curve != OID_EC_P256 {
        return None;
    }
    let point = spki.subject_public_key.data.as_ref();
    (point.len() == 65 && point.first() == Some(&0x04)).then(|| point.to_vec())
}
