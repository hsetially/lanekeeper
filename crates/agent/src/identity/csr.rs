//! The certificate signing request (S5).
//!
//! The request carries the public key and a signature made with the private key, which proves possession. It asks for
//! no name: the hub decides the identity in the certificate from the kind of peer that joins and the credential it
//! presents, so a request cannot ask for someone else's identity.

use bytes::Bytes;
use rcgen::{CertificateParams, DistinguishedName, DnType};
use zeroize::Zeroize;

use super::error::KeyError;
use super::key::{KeyMaterial, key_pair};

/// A PKCS#10 request for `key`, DER. The rcgen key pair that signs it lives only for this call and is zeroized.
pub(super) fn build(key: &KeyMaterial) -> Result<Bytes, KeyError> {
    let mut pair = key_pair(key.pkcs8_der())?;
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "lanekeeper-agent");
    let request = params.serialize_request(&pair);
    pair.zeroize();
    let request = request.map_err(|_| KeyError)?;
    Ok(Bytes::copy_from_slice(request.der()))
}
