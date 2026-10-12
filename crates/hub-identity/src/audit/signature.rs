//! What a checkpoint signs, and how its signature is checked with the public key (S9).
//!
//! The signed message is `lk-audit-checkpoint-v1 || seq (u64, big endian) || head hash`. Cloud KMS signs the SHA-256 digest of
//! that message with an asymmetric P-256 key ([`ports::KmsSigner::sign_digest`]); the signature is ASN.1 DER. Anyone with the
//! public key (the SPKI of the KMS key version, which is configuration and not a secret) checks it with
//! [`EcdsaP256Verifier`], so the nightly job does not need KMS to verify, and a database owner cannot forge a checkpoint.
//!
//! The domain prefix keeps a checkpoint signature from being valid for any other message the same key signs (the CA, S6).

use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1, ParsedPublicKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use domain::ContentHash;
use sha2::{Digest, Sha256};

/// The first bytes of every checkpoint message.
pub const CHECKPOINT_DOMAIN: &[u8] = b"lk-audit-checkpoint-v1";

/// The most key versions a verifier holds: the current one and a few earlier ones, after rotations.
pub const MAX_KEY_VERSIONS: usize = 16;

/// The longest DER signature checked. A P-256 signature is at most 72 bytes.
const MAX_SIGNATURE_BYTES: usize = 80;

/// The longest PEM text read.
const MAX_PEM_BYTES: usize = 2048;

/// `SubjectPublicKeyInfo` of a P-256 key up to the uncompressed point: a SEQUENCE of the `id-ecPublicKey` and `prime256v1`
/// algorithm identifier, then a BIT STRING of 66 bytes that starts with the unused-bits byte `00`.
const SPKI_P256_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86,
    0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
/// Prefix, `04`, then X and Y.
const SPKI_P256_LEN: usize = 26 + 65;

/// Why a public key was refused. Carries no key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("public key is not a P-256 SubjectPublicKeyInfo")]
    NotP256Spki,
    #[error("public key is not on the curve")]
    BadPoint,
    #[error("public key PEM is not valid")]
    BadPem,
    #[error("key version name is not valid")]
    BadKeyVersion,
    #[error("too many key versions")]
    TooManyKeys,
}

/// The message a checkpoint of the chain head `head` at `seq` signs.
pub fn checkpoint_message(seq: u64, head: &ContentHash) -> Vec<u8> {
    let mut m = Vec::with_capacity(CHECKPOINT_DOMAIN.len() + 8 + 32);
    m.extend_from_slice(CHECKPOINT_DOMAIN);
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(head.as_bytes());
    m
}

/// The SHA-256 digest of [`checkpoint_message`], which is what KMS signs.
pub fn checkpoint_digest(seq: u64, head: &ContentHash) -> [u8; 32] {
    Sha256::digest(checkpoint_message(seq, head)).into()
}

/// Checks checkpoint signatures. A trait so that tests and other key types can stand in.
pub trait CheckpointVerifier: Send + Sync + 'static {
    /// True only if `signature` is a valid signature of `message` by the key version named `key_version`.
    fn verify(&self, key_version: &str, message: &[u8], signature: &[u8]) -> bool;
}

/// ECDSA over P-256 with SHA-256, one public key per KMS key version.
#[derive(Default)]
pub struct EcdsaP256Verifier {
    keys: Vec<(String, ParsedPublicKey)>,
}

impl std::fmt::Debug for EcdsaP256Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EcdsaP256Verifier")
            .field(
                "key_versions",
                &self.keys.iter().map(|(v, _)| v).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl EcdsaP256Verifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust the key in `spki_der` for signatures that name `key_version`.
    pub fn with_spki_der(mut self, key_version: &str, spki_der: &[u8]) -> Result<Self, SignatureError> {
        if ports::KeyRef::parse(key_version).is_err() {
            return Err(SignatureError::BadKeyVersion);
        }
        if self.keys.len() >= MAX_KEY_VERSIONS {
            return Err(SignatureError::TooManyKeys);
        }
        let point = point_from_spki(spki_der)?;
        let key =
            ParsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, point).map_err(|_| SignatureError::BadPoint)?;
        self.keys.retain(|(v, _)| v != key_version);
        self.keys.push((key_version.to_owned(), key));
        Ok(self)
    }

    /// Like [`EcdsaP256Verifier::with_spki_der`], from the PEM that `gcloud kms keys versions get-public-key` prints.
    pub fn with_spki_pem(self, key_version: &str, pem: &str) -> Result<Self, SignatureError> {
        let der = spki_der_from_pem(pem)?;
        self.with_spki_der(key_version, &der)
    }

    pub fn key_versions(&self) -> Vec<&str> {
        self.keys.iter().map(|(v, _)| v.as_str()).collect()
    }
}

impl CheckpointVerifier for EcdsaP256Verifier {
    fn verify(&self, key_version: &str, message: &[u8], signature: &[u8]) -> bool {
        if signature.is_empty() || signature.len() > MAX_SIGNATURE_BYTES {
            return false;
        }
        self.keys
            .iter()
            .find(|(v, _)| v == key_version)
            .is_some_and(|(_, key)| key.verify_sig(message, signature).is_ok())
    }
}

/// The uncompressed point (`04 || X || Y`) of a P-256 `SubjectPublicKeyInfo`. Anything else, another curve or an extra
/// field included, is refused.
fn point_from_spki(spki: &[u8]) -> Result<&[u8], SignatureError> {
    if spki.len() != SPKI_P256_LEN
        || !spki.starts_with(&SPKI_P256_PREFIX)
        || spki[SPKI_P256_PREFIX.len()] != 0x04
    {
        return Err(SignatureError::NotP256Spki);
    }
    Ok(&spki[SPKI_P256_PREFIX.len()..])
}

/// The DER bytes inside a `-----BEGIN PUBLIC KEY-----` block.
fn spki_der_from_pem(pem: &str) -> Result<Vec<u8>, SignatureError> {
    if pem.len() > MAX_PEM_BYTES {
        return Err(SignatureError::BadPem);
    }
    let body = pem
        .trim()
        .strip_prefix("-----BEGIN PUBLIC KEY-----")
        .and_then(|rest| rest.strip_suffix("-----END PUBLIC KEY-----"))
        .ok_or(SignatureError::BadPem)?;
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(compact).map_err(|_| SignatureError::BadPem)
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::encoding::AsDer;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    use super::*;

    fn head(n: u8) -> ContentHash {
        ContentHash::from_bytes([n; 32])
    }

    fn key() -> (EcdsaKeyPair, Vec<u8>) {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).unwrap();
        let der = pair.public_key().as_der().unwrap().as_ref().to_vec();
        (pair, der)
    }

    #[test]
    fn message_layout_is_domain_then_seq_then_head() {
        let m = checkpoint_message(0x0102_0304_0506_0708, &head(9));
        assert_eq!(&m[..22], b"lk-audit-checkpoint-v1");
        assert_eq!(&m[22..30], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&m[30..], &[9; 32]);
        assert_eq!(m.len(), 62);
    }

    #[test]
    fn digest_is_sha256_of_the_message() {
        let d = checkpoint_digest(7, &head(1));
        assert_eq!(
            d.as_slice(),
            Sha256::digest(checkpoint_message(7, &head(1))).as_slice()
        );
    }

    #[test]
    fn a_signature_over_the_message_verifies_and_other_inputs_do_not() {
        let (pair, der) = key();
        let v = EcdsaP256Verifier::new().with_spki_der("kv/1", &der).unwrap();
        let msg = checkpoint_message(5, &head(3));
        let sig = pair.sign(&aws_lc_rs::rand::SystemRandom::new(), &msg).unwrap();
        assert!(v.verify("kv/1", &msg, sig.as_ref()));
        assert!(!v.verify("kv/2", &msg, sig.as_ref()), "unknown key version");
        assert!(
            !v.verify("kv/1", &checkpoint_message(6, &head(3)), sig.as_ref()),
            "other seq"
        );
        assert!(
            !v.verify("kv/1", &checkpoint_message(5, &head(4)), sig.as_ref()),
            "other head"
        );
        let mut bad = sig.as_ref().to_vec();
        bad[10] ^= 1;
        assert!(!v.verify("kv/1", &msg, &bad));
        assert!(!v.verify("kv/1", &msg, &[]));
        assert!(!v.verify("kv/1", &msg, &[0u8; 200]));
    }

    #[test]
    fn spki_must_be_exactly_a_p256_key() {
        let (_, der) = key();
        assert!(EcdsaP256Verifier::new().with_spki_der("kv/1", &der).is_ok());
        for bad in [
            Vec::new(),
            der[..90].to_vec(),
            [der.clone(), vec![0]].concat(),
            vec![0; 91],
        ] {
            assert_eq!(
                EcdsaP256Verifier::new().with_spki_der("kv/1", &bad).unwrap_err(),
                SignatureError::NotP256Spki
            );
        }
        // Right shape, point not on the curve.
        let mut off_curve = der.clone();
        off_curve[40] ^= 0xff;
        assert_eq!(
            EcdsaP256Verifier::new()
                .with_spki_der("kv/1", &off_curve)
                .unwrap_err(),
            SignatureError::BadPoint
        );
        assert_eq!(
            EcdsaP256Verifier::new()
                .with_spki_der("not valid!", &der)
                .unwrap_err(),
            SignatureError::BadKeyVersion
        );
    }

    #[test]
    fn pem_is_read_and_junk_is_refused() {
        let (_, der) = key();
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(&der)
        );
        assert!(EcdsaP256Verifier::new().with_spki_pem("kv/1", &pem).is_ok());
        for bad in [
            "",
            "nothing",
            "-----BEGIN PUBLIC KEY-----\n!!!\n-----END PUBLIC KEY-----",
            &"A".repeat(5000),
        ] {
            assert!(EcdsaP256Verifier::new().with_spki_pem("kv/1", bad).is_err());
        }
    }

    #[test]
    fn key_versions_are_bounded() {
        let (_, der) = key();
        let mut v = EcdsaP256Verifier::new();
        for n in 0..MAX_KEY_VERSIONS {
            v = v.with_spki_der(&format!("kv/{n}"), &der).unwrap();
        }
        assert_eq!(
            v.with_spki_der("kv/extra", &der).unwrap_err(),
            SignatureError::TooManyKeys
        );
    }
}
