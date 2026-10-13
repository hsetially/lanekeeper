//! `KmsSigner` and `KmsEnvelope` on the Cloud KMS REST API (T2, S5, S6, S7).
//!
//! - **Signing** (`asymmetricSign`): the CA's private key never leaves KMS. The hub sends a SHA-256 digest and receives
//!   a signature. The request carries the digest's CRC-32C and the answer must confirm it verified it and carry the
//!   signature's CRC-32C, so that a certificate is never signed over a digest that was damaged on the way, and a damaged
//!   signature is never put into one.
//! - **Envelope** (`encrypt`, `decrypt`): a data key is wrapped with the user id and credential id as additional
//!   authenticated data, so a ciphertext opens only for its owner. Wrapping without that data is refused here, because KMS
//!   itself would accept it. Which key version wrapped a key is kept in the [`WrappedKey`], so unwrapping survives rotation.
//!
//! Key names are parsed into their documented shape before they reach a URL. Nothing from a request or a response (a key
//! name, a digest, a plaintext) goes into an error or a log line.

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use ports::{KeyRef, KmsEnvelope, KmsError, KmsSigner, Signature, WrappedKey};
use serde::{Deserialize, Serialize};
use tracing::warn;
use zeroize::{Zeroize, Zeroizing};

use super::api::{ApiError, Call, GoogleApi};
use super::crc32c::crc32c;
use super::http::{GoogleRequest, GoogleResponse, Origin};
use super::metrics::{Operation, Outcome};
use super::wire::{CryptoKeyName, CryptoKeyVersionName, Int64, b64, from_b64};

/// An ECDSA signature over P-256 is at most 72 bytes in DER; the port allows 1,024.
const MAX_SIGNATURE_BYTES: usize = 1024;
/// A wrapped 32-byte data key is under 100 bytes. Anything near this is not one of ours.
const MAX_WRAPPED_BYTES: usize = 2048;
/// The associated data is a pair of ids.
const MAX_AAD_BYTES: usize = 1024;
const MAX_PEM_BYTES: usize = 8 * 1024;
const DEK_BYTES: usize = 32;

/// What a KMS call said, for the failure paths.
fn log_refused(code: &'static str, status: u16) {
    warn!(code, status, "kms refused the request");
}

fn api_error(error: ApiError) -> KmsError {
    warn!(code = "kms_unreachable", ?error);
    match error {
        ApiError::Rejected => KmsError::Invalid,
        ApiError::Timeout | ApiError::Unavailable | ApiError::TooLarge | ApiError::NoToken => {
            KmsError::Unavailable
        }
    }
}

/// The error for a status that the call does not treat as an answer.
fn refused(code: &'static str, status: u16, bad_request: KmsError) -> KmsError {
    log_refused(code, status);
    match status {
        404 => KmsError::KeyNotFound,
        400 => bad_request,
        _ => KmsError::Unavailable,
    }
}

fn done(call: Call<'_>, response: GoogleResponse, error: KmsError, code: &'static str) -> KmsError {
    let status = response.status;
    response.wipe();
    let outcome = Call::outcome_for_status(status);
    call.finish(outcome);
    refused(code, status, error)
}

#[derive(Serialize)]
struct Sha256Digest<'a> {
    sha256: &'a str,
}

#[derive(Serialize)]
struct SignRequest<'a> {
    digest: Sha256Digest<'a>,
    #[serde(rename = "digestCrc32c")]
    digest_crc32c: String,
}

#[derive(Deserialize)]
struct SignResponse {
    name: String,
    signature: String,
    #[serde(rename = "signatureCrc32c")]
    signature_crc32c: Option<Int64>,
    #[serde(rename = "verifiedDigestCrc32c", default)]
    verified_digest_crc32c: bool,
}

#[derive(Deserialize)]
struct PublicKeyResponse {
    pem: String,
    #[serde(rename = "pemCrc32c")]
    pem_crc32c: Option<Int64>,
    algorithm: String,
}

/// A KMS signing key's public half, for the CA ceremony (A8): the `SubjectPublicKeyInfo` in DER.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    spki_der: Bytes,
}

impl PublicKey {
    #[must_use]
    pub fn spki_der(&self) -> &[u8] {
        &self.spki_der
    }
}

/// Asymmetric signing with Cloud KMS.
#[derive(Debug, Clone)]
pub struct CloudKmsSigner {
    api: GoogleApi,
    origin: Origin,
}

impl CloudKmsSigner {
    #[must_use]
    pub fn new(api: GoogleApi, origin: Origin) -> Self {
        Self { api, origin }
    }

    /// The public key of `key`, which must be an `EC_SIGN_P256_SHA256` version. Not on the [`KmsSigner`] port: only the
    /// offline CA ceremony (`hub ca-init`) needs it, and the runtime never does (A8).
    pub async fn public_key(&self, key: &KeyRef) -> Result<PublicKey, KmsError> {
        let version = CryptoKeyVersionName::parse(key).ok_or(KmsError::Invalid)?;
        let call = self.api.begin(Operation::KmsPublicKey);
        let request = GoogleRequest::get(format!("/v1/{}/publicKey", version.as_str()));
        let response = call.send(&self.origin, request).await.map_err(api_error)?;
        if response.status != 200 {
            return Err(done(call, response, KmsError::Invalid, "kms_public_key_refused"));
        }
        let parsed = serde_json::from_slice::<PublicKeyResponse>(&response.body).ok();
        response.wipe();
        let Some(answer) = parsed else {
            call.finish(Outcome::BadResponse);
            return Err(KmsError::Unavailable);
        };
        let intact = answer.pem.len() <= MAX_PEM_BYTES
            && answer
                .pem_crc32c
                .is_some_and(|c| c.is_crc32c(crc32c(answer.pem.as_bytes())));
        if !intact {
            warn!(code = "kms_public_key_bad_answer");
            call.finish(Outcome::BadResponse);
            return Err(KmsError::Unavailable);
        }
        if answer.algorithm != "EC_SIGN_P256_SHA256" {
            // Not a key the CA can use: say so, rather than let a certificate be built over the wrong algorithm.
            call.finish(Outcome::Ok);
            return Err(KmsError::Invalid);
        }
        let block = pem::parse(answer.pem.as_bytes())
            .ok()
            .filter(|b| b.tag() == "PUBLIC KEY");
        call.finish(Outcome::Ok);
        let block = block.ok_or(KmsError::Unavailable)?;
        Ok(PublicKey {
            spki_der: Bytes::from(block.into_contents()),
        })
    }
}

#[async_trait]
impl KmsSigner for CloudKmsSigner {
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError> {
        let version = CryptoKeyVersionName::parse(&key).ok_or(KmsError::Invalid)?;
        let digest_b64 = b64(&digest);
        let body = serde_json::to_vec(&SignRequest {
            digest: Sha256Digest { sha256: &digest_b64 },
            digest_crc32c: crc32c(&digest).to_string(),
        })
        .map_err(|_| KmsError::Invalid)?;
        let call = self.api.begin(Operation::KmsSign);
        let request = GoogleRequest::post_json(format!("/v1/{}:asymmetricSign", version.as_str()), body);
        let response = call.send(&self.origin, request).await.map_err(api_error)?;
        if response.status != 200 {
            return Err(done(call, response, KmsError::Invalid, "kms_sign_refused"));
        }
        let parsed = serde_json::from_slice::<SignResponse>(&response.body).ok();
        response.wipe();
        let signature = parsed.and_then(|answer| {
            let signature = from_b64(&answer.signature)?;
            let intact = answer.verified_digest_crc32c
                && answer.name == version.as_str()
                && answer
                    .signature_crc32c
                    .is_some_and(|c| c.is_crc32c(crc32c(&signature)))
                && !signature.is_empty()
                && signature.len() <= MAX_SIGNATURE_BYTES;
            intact.then_some(signature)
        });
        if let Some(signature) = signature {
            call.finish(Outcome::Ok);
            Ok(Signature::new(Bytes::from(signature)))
        } else {
            warn!(code = "kms_sign_bad_answer");
            call.finish(Outcome::BadResponse);
            Err(KmsError::Unavailable)
        }
    }
}

#[derive(Serialize)]
struct EncryptRequest<'a> {
    plaintext: &'a str,
    #[serde(rename = "additionalAuthenticatedData")]
    aad: &'a str,
    #[serde(rename = "plaintextCrc32c")]
    plaintext_crc32c: String,
    #[serde(rename = "additionalAuthenticatedDataCrc32c")]
    aad_crc32c: String,
}

#[derive(Deserialize)]
struct EncryptResponse {
    name: String,
    ciphertext: String,
    #[serde(rename = "ciphertextCrc32c")]
    ciphertext_crc32c: Option<Int64>,
    #[serde(rename = "verifiedPlaintextCrc32c", default)]
    verified_plaintext_crc32c: bool,
    #[serde(rename = "verifiedAdditionalAuthenticatedDataCrc32c", default)]
    verified_aad_crc32c: bool,
}

#[derive(Serialize)]
struct DecryptRequest<'a> {
    ciphertext: &'a str,
    #[serde(rename = "additionalAuthenticatedData")]
    aad: &'a str,
    #[serde(rename = "ciphertextCrc32c")]
    ciphertext_crc32c: String,
    #[serde(rename = "additionalAuthenticatedDataCrc32c")]
    aad_crc32c: String,
}

#[derive(Deserialize)]
struct DecryptResponse {
    plaintext: String,
    #[serde(rename = "plaintextCrc32c")]
    plaintext_crc32c: Option<Int64>,
}

/// Envelope encryption of data keys with one Cloud KMS symmetric key.
#[derive(Debug, Clone)]
pub struct CloudKmsEnvelope {
    api: GoogleApi,
    origin: Origin,
    key: CryptoKeyName,
}

impl CloudKmsEnvelope {
    /// `key` names the crypto key (not a version): `projects/P/locations/L/keyRings/R/cryptoKeys/K`.
    pub fn new(api: GoogleApi, origin: Origin, key: &KeyRef) -> Result<Self, KmsError> {
        Ok(Self {
            api,
            origin,
            key: CryptoKeyName::parse(key).ok_or(KmsError::Invalid)?,
        })
    }
}

#[async_trait]
impl KmsEnvelope for CloudKmsEnvelope {
    async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError> {
        // Associated data is what binds a ciphertext to its owner (S7), and KMS would accept none: refuse it here.
        if aad.is_empty() || aad.len() > MAX_AAD_BYTES {
            return Err(KmsError::Invalid);
        }
        let plaintext = Zeroizing::new(b64(dek.expose()));
        let aad_b64 = b64(aad);
        let body = serde_json::to_vec(&EncryptRequest {
            plaintext: &plaintext,
            aad: &aad_b64,
            plaintext_crc32c: crc32c(dek.expose()).to_string(),
            aad_crc32c: crc32c(aad).to_string(),
        })
        .map_err(|_| KmsError::Invalid)?;
        let call = self.api.begin(Operation::KmsEncrypt);
        let request = GoogleRequest::post_json(format!("/v1/{}:encrypt", self.key.as_str()), body);
        let response = call.send(&self.origin, request).await.map_err(api_error)?;
        if response.status != 200 {
            return Err(done(call, response, KmsError::Invalid, "kms_encrypt_refused"));
        }
        let parsed = serde_json::from_slice::<EncryptResponse>(&response.body).ok();
        response.wipe();
        let wrapped = parsed.and_then(|answer| {
            let ciphertext = from_b64(&answer.ciphertext)?;
            let version = KeyRef::parse(&answer.name).ok().filter(|v| {
                // The version that wrapped it must be one of this key's, or unwrapping would look in the wrong place.
                CryptoKeyVersionName::parse(v).is_some_and(|v| self.key.owns(&v))
            })?;
            let intact = answer.verified_plaintext_crc32c
                && answer.verified_aad_crc32c
                && answer
                    .ciphertext_crc32c
                    .is_some_and(|c| c.is_crc32c(crc32c(&ciphertext)))
                && !ciphertext.is_empty()
                && ciphertext.len() <= MAX_WRAPPED_BYTES;
            intact.then(|| WrappedKey {
                key_version: version,
                ciphertext: Bytes::from(ciphertext),
            })
        });
        if let Some(wrapped) = wrapped {
            call.finish(Outcome::Ok);
            Ok(wrapped)
        } else {
            warn!(code = "kms_encrypt_bad_answer");
            call.finish(Outcome::BadResponse);
            Err(KmsError::Unavailable)
        }
    }

    async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError> {
        // A key that this envelope did not wrap, a ciphertext of the wrong size and missing associated data all
        // fail the same way as a wrong owner would: `Decrypt`, without asking KMS.
        let version = CryptoKeyVersionName::parse(&w.key_version).filter(|v| self.key.owns(v));
        if version.is_none()
            || aad.is_empty()
            || aad.len() > MAX_AAD_BYTES
            || w.ciphertext.is_empty()
            || w.ciphertext.len() > MAX_WRAPPED_BYTES
        {
            return Err(KmsError::Decrypt);
        }
        let ciphertext = b64(&w.ciphertext);
        let aad_b64 = b64(aad);
        let body = serde_json::to_vec(&DecryptRequest {
            ciphertext: &ciphertext,
            aad: &aad_b64,
            ciphertext_crc32c: crc32c(&w.ciphertext).to_string(),
            aad_crc32c: crc32c(aad).to_string(),
        })
        .map_err(|_| KmsError::Invalid)?;
        let call = self.api.begin(Operation::KmsDecrypt);
        let request = GoogleRequest::post_json(format!("/v1/{}:decrypt", self.key.as_str()), body);
        let response = call.send(&self.origin, request).await.map_err(api_error)?;
        if response.status != 200 {
            // A wrong owner, a tampered or a truncated ciphertext is a 400 from KMS.
            return Err(done(call, response, KmsError::Decrypt, "kms_decrypt_refused"));
        }
        let parsed = serde_json::from_slice::<DecryptResponse>(&response.body).ok();
        response.wipe();
        let dek = parsed.and_then(|answer| {
            let plaintext = Zeroizing::new(from_b64(&answer.plaintext)?);
            let intact = answer
                .plaintext_crc32c
                .is_some_and(|c| c.is_crc32c(crc32c(&plaintext)));
            (intact && plaintext.len() == DEK_BYTES).then(|| {
                let mut key = [0u8; DEK_BYTES];
                key.copy_from_slice(&plaintext);
                let secret = Secret::new(key);
                key.zeroize();
                secret
            })
        });
        if let Some(dek) = dek {
            call.finish(Outcome::Ok);
            Ok(dek)
        } else {
            warn!(code = "kms_decrypt_bad_answer");
            call.finish(Outcome::BadResponse);
            Err(KmsError::Unavailable)
        }
    }
}
