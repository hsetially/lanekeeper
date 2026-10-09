//! Cloud KMS and Secret Manager ports (S6-S9). Implemented by 03b (and 03a for the CA).

use std::fmt;

use async_trait::async_trait;
use bytes::Bytes;
use domain::Secret;
use serde::{Deserialize, Serialize};

/// Why a key reference was rejected. Carries no input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyRefError {
    #[error("key reference is empty")]
    Empty,
    #[error("key reference is too long")]
    TooLong,
    #[error("key reference contains a character that is not allowed")]
    BadChar,
}

/// The name of a KMS key, for example a Cloud KMS crypto key version resource name. Opaque to callers.
///
/// 1-512 characters from `[A-Za-z0-9/_.:-]`. It never contains a secret: it only names a key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct KeyRef(String);

impl KeyRef {
    pub const MAX_LEN: usize = 512;

    pub fn parse(s: &str) -> Result<Self, KeyRefError> {
        if s.is_empty() {
            return Err(KeyRefError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(KeyRefError::TooLong);
        }
        if !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'.' | b':' | b'-'))
        {
            return Err(KeyRefError::BadChar);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for KeyRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A signature over a digest. Public data (it is stored with the audit checkpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(Bytes);

impl Signature {
    pub fn new(bytes: Bytes) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A data key encrypted by KMS. Safe to store: only KMS can open it, and only with the same `aad`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    /// Which KMS key version wrapped it, so that unwrapping survives key rotation.
    pub key_version: KeyRef,
    pub ciphertext: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KmsError {
    #[error("kms key not found")]
    KeyNotFound,
    /// Wrong associated data, a tampered ciphertext or an unknown key version.
    #[error("kms could not decrypt the key")]
    Decrypt,
    #[error("kms rejected the request")]
    Invalid,
    #[error("kms is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait KmsSigner: Send + Sync + 'static {
    /// Sign a SHA-256 digest with the named key. Used for audit checkpoints (S9) and the CA (S6).
    async fn sign_digest(&self, key: KeyRef, digest: [u8; 32]) -> Result<Signature, KmsError>;
}

/// Envelope encryption of GitHub tokens (S7): the data key is wrapped by KMS with the user id and
/// credential id as associated data, which binds each ciphertext to its owner.
#[async_trait]
pub trait KmsEnvelope: Send + Sync + 'static {
    async fn wrap(&self, dek: &Secret<[u8; 32]>, aad: &[u8]) -> Result<WrappedKey, KmsError>;
    /// Fails with [`KmsError::Decrypt`] unless `aad` equals the value used to wrap.
    async fn unwrap(&self, w: &WrappedKey, aad: &[u8]) -> Result<Secret<[u8; 32]>, KmsError>;
}

/// Why a secret could not be read. Carries neither the secret nor its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found")]
    NotFound,
    #[error("secret name is not valid")]
    InvalidName,
    #[error("secret manager is unavailable")]
    Unavailable,
}

/// Runtime secrets from Secret Manager (S8).
#[async_trait]
pub trait SecretSource: Send + Sync + 'static {
    /// `name` is 1-255 characters from `[A-Za-z0-9_-]`.
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError>;
}

/// Whether `name` is acceptable to [`SecretSource::get`]. Shared by implementations and fakes.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
