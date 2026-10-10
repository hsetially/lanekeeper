//! The agent's private key (S5): ECDSA P-256, made in memory, written only into the certificate Secret.
//!
//! The PKCS#8 bytes live in a [`Secret`], which prints as `[redacted]` and is zeroized on drop. The key is not
//! cloneable: a renewal makes a new one. The rcgen key pair that signs the CSR exists only for the length of that call
//! and is zeroized before it is dropped.

use std::fmt;

use bytes::Bytes;
use domain::Secret;
use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PublicKeyData};
use zeroize::Zeroize;

use super::error::KeyError;

/// PEM label of a PKCS#8 private key.
const PEM_PRIVATE_KEY: &str = "PRIVATE KEY";

/// An ECDSA P-256 key pair, as PKCS#8 DER.
pub struct KeyMaterial {
    pkcs8: Secret<Vec<u8>>,
    /// The public half (`SubjectPublicKeyInfo` DER), kept so a certificate can be matched to the key without
    /// touching the private bytes again.
    spki: Vec<u8>,
}

impl KeyMaterial {
    /// A fresh key from the system random number generator.
    pub fn generate() -> Result<Self, KeyError> {
        let mut pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|_| KeyError)?;
        let der = pair.serialize_der();
        let spki = pair.subject_public_key_info();
        pair.zeroize();
        Ok(Self {
            pkcs8: Secret::new(der),
            spki,
        })
    }

    /// Read a PKCS#8 key. Only ECDSA P-256 is accepted (S5); anything else is refused, and the bytes are wiped.
    pub fn from_pkcs8_der(mut der: Vec<u8>) -> Result<Self, KeyError> {
        let parsed = key_pair(&der);
        match parsed {
            Ok(mut pair) => {
                let spki = pair.subject_public_key_info();
                pair.zeroize();
                Ok(Self {
                    pkcs8: Secret::new(der),
                    spki,
                })
            }
            Err(e) => {
                der.zeroize();
                Err(e)
            }
        }
    }

    /// Read a key from the PEM text of a `tls.key` entry: exactly one `PRIVATE KEY` block.
    pub fn from_pem(text: &[u8]) -> Result<Self, KeyError> {
        let blocks = pem::parse_many(text).map_err(|_| KeyError)?;
        let mut keys = blocks.into_iter().filter(|b| b.tag() == PEM_PRIVATE_KEY);
        let (Some(block), None) = (keys.next(), keys.next()) else {
            return Err(KeyError);
        };
        Self::from_pkcs8_der(block.into_contents())
    }

    /// The key as PEM, for the certificate Secret. This is the only place the private bytes leave this type.
    pub fn to_pem(&self) -> Secret<String> {
        let block = pem::Pem::new(PEM_PRIVATE_KEY, self.pkcs8.expose().clone());
        let text = pem::encode_config(
            &block,
            pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF),
        );
        let mut contents = block.into_contents();
        contents.zeroize();
        Secret::new(text)
    }

    /// The public key, as `SubjectPublicKeyInfo` DER.
    pub fn spki_der(&self) -> &[u8] {
        &self.spki
    }

    /// A PKCS#10 request for this key, self-signed to prove we hold it. See `csr.rs`.
    pub fn csr_der(&self) -> Result<Bytes, KeyError> {
        super::csr::build(self)
    }

    /// The PKCS#8 bytes, for the TLS stack that signs the handshake. Keep the borrow short.
    pub fn pkcs8_der(&self) -> &[u8] {
        self.pkcs8.expose()
    }
}

impl fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyMaterial([redacted])")
    }
}

/// The rcgen key pair for P-256 PKCS#8 bytes. The caller zeroizes it as soon as it is done.
pub(super) fn key_pair(der: &[u8]) -> Result<KeyPair, KeyError> {
    let mut pair = KeyPair::try_from(der).map_err(|_| KeyError)?;
    if pair.is_compatible(&PKCS_ECDSA_P256_SHA256) {
        Ok(pair)
    } else {
        pair.zeroize();
        Err(KeyError)
    }
}

#[cfg(test)]
mod tests {
    use rcgen::PKCS_ECDSA_P384_SHA384;

    use super::*;

    #[test]
    fn a_generated_key_round_trips_through_pem() {
        let key = KeyMaterial::generate().unwrap();
        let pem = key.to_pem();
        assert!(pem.expose().starts_with("-----BEGIN PRIVATE KEY-----\n"));
        let back = KeyMaterial::from_pem(pem.expose().as_bytes()).unwrap();
        assert_eq!(back.pkcs8_der(), key.pkcs8_der());
        assert_eq!(back.spki_der(), key.spki_der());
    }

    #[test]
    fn two_keys_differ() {
        let a = KeyMaterial::generate().unwrap();
        let b = KeyMaterial::generate().unwrap();
        assert_ne!(a.spki_der(), b.spki_der());
        assert_ne!(a.pkcs8_der(), b.pkcs8_der());
    }

    #[test]
    fn debug_never_shows_key_bytes() {
        let key = KeyMaterial::generate().unwrap();
        for shown in [format!("{key:?}"), format!("{key:#?}")] {
            assert_eq!(shown, "KeyMaterial([redacted])");
        }
    }

    #[test]
    fn only_p256_is_accepted() {
        let p384 = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
        assert!(KeyMaterial::from_pkcs8_der(p384.serialize_der()).is_err());
        let ed = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        assert!(KeyMaterial::from_pkcs8_der(ed.serialize_der()).is_err());
        assert!(KeyMaterial::from_pkcs8_der(vec![1, 2, 3]).is_err());
        assert!(KeyMaterial::from_pkcs8_der(Vec::new()).is_err());
    }

    #[test]
    fn pem_with_zero_or_two_keys_is_refused() {
        let a = KeyMaterial::generate().unwrap().to_pem();
        let b = KeyMaterial::generate().unwrap().to_pem();
        assert!(KeyMaterial::from_pem(b"").is_err());
        assert!(KeyMaterial::from_pem(b"not pem").is_err());
        let two = format!("{}{}", a.expose(), b.expose());
        assert!(KeyMaterial::from_pem(two.as_bytes()).is_err());
        // A block with another label is not a key.
        let cert_like = a.expose().replace("PRIVATE KEY", "CERTIFICATE");
        assert!(KeyMaterial::from_pem(cert_like.as_bytes()).is_err());
    }

    #[test]
    fn the_csr_is_a_valid_request_for_this_key() {
        let key = KeyMaterial::generate().unwrap();
        let csr = key.csr_der().unwrap();
        // x509-parser checks the self-signature when it parses a request the way rcgen's parser does.
        let parsed = rcgen::CertificateSigningRequestParams::from_der(&csr.to_vec().into()).unwrap();
        assert_eq!(parsed.public_key.subject_public_key_info(), key.spki_der());
        assert!(parsed.params.subject_alt_names.is_empty());
        // The private scalar follows `ECPrivateKey { version 1, privateKey OCTET STRING (32) }` inside the PKCS#8 bytes.
        let der = key.pkcs8_der();
        let marker = [0x02_u8, 0x01, 0x01, 0x04, 0x20];
        let at = der.windows(marker.len()).position(|w| w == marker).unwrap() + marker.len();
        let scalar = &der[at..at + 32];
        assert!(
            !csr.windows(32).any(|w| w == scalar),
            "the private scalar is in the CSR"
        );
    }
}
