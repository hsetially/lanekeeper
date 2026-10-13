//! The read-only credential the mirror fetches with (Q17, A12, S7).
//!
//! One service credential, read-only on both repositories. User tokens are never used here (D41, D42 belong to the write paths).
//! The credential reaches `gix` through its credential hook, in memory: it is never part of a URL, a command line, an
//! environment variable or a log line, and the hook hands it out only to the host the remote's URL names, over `https`.
//!
//! [`SecretManagerCredentials`] (the default) reads the token from Secret Manager through [`SecretSource`], which caches it for at
//! most five minutes, so a rotated token is picked up without a restart. [`MountedFileCredentials`] reads it from a file for
//! Q17's interim (a Kubernetes Secret mounted into the pod) and re-reads it on every fetch for the same reason.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Secret;
use ports::{SecretError, SecretSource};

/// The longest token file or secret the hub reads. A GitHub token is under 100 bytes.
const MAX_TOKEN_BYTES: usize = 4096;

/// GitHub accepts any non-empty user name next to a token; this is the one its documentation shows.
pub const DEFAULT_USERNAME: &str = "x-access-token";

/// Why a credential could not be read. Carries neither the credential nor the name of its secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("the git credential is not available")]
    Unavailable,
    #[error("the git credential is not valid")]
    Invalid,
}

/// A user name and a token, as HTTP basic authentication. The token is redacted in `Debug` and wiped on drop.
#[derive(Debug)]
pub struct GitCredential {
    pub username: String,
    pub token: Secret<String>,
}

impl GitCredential {
    /// `None` when the token is empty, has a control character, or is over the size limit.
    #[must_use]
    pub fn new(username: &str, token: Secret<String>) -> Option<Self> {
        let t = token.expose();
        let ok = !t.is_empty()
            && t.len() <= MAX_TOKEN_BYTES
            && !t.chars().any(char::is_control)
            && !username.is_empty()
            && !username.chars().any(|c| c.is_control() || c == ':');
        ok.then(|| Self {
            username: username.to_owned(),
            token,
        })
    }
}

/// Where the mirror gets the credential it fetches with.
#[async_trait]
pub trait GitCredentials: Send + Sync + 'static {
    /// `Ok(None)` means anonymous access (a public repository). Called before each fetch, so rotation needs no restart.
    async fn credential(&self) -> Result<Option<GitCredential>, CredentialError>;
}

/// No credential: public repositories and the tests over the file transport.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCredentials;

#[async_trait]
impl GitCredentials for NoCredentials {
    async fn credential(&self) -> Result<Option<GitCredential>, CredentialError> {
        Ok(None)
    }
}

/// The token is a secret in Secret Manager.
#[derive(Clone)]
pub struct SecretManagerCredentials {
    source: Arc<dyn SecretSource>,
    secret_name: String,
    username: String,
}

impl std::fmt::Debug for SecretManagerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretManagerCredentials").finish_non_exhaustive()
    }
}

impl SecretManagerCredentials {
    #[must_use]
    pub fn new(source: Arc<dyn SecretSource>, secret_name: &str) -> Self {
        Self {
            source,
            secret_name: secret_name.to_owned(),
            username: DEFAULT_USERNAME.to_owned(),
        }
    }

    #[must_use]
    pub fn with_username(mut self, username: &str) -> Self {
        username.clone_into(&mut self.username);
        self
    }
}

#[async_trait]
impl GitCredentials for SecretManagerCredentials {
    async fn credential(&self) -> Result<Option<GitCredential>, CredentialError> {
        let token = self.source.get(&self.secret_name).await.map_err(|e| match e {
            SecretError::NotFound | SecretError::InvalidName => CredentialError::Invalid,
            SecretError::Unavailable => CredentialError::Unavailable,
        })?;
        // The secret may end with the newline an editor added.
        let trimmed = Secret::new(token.expose().trim().to_owned());
        GitCredential::new(&self.username, trimmed)
            .map(Some)
            .ok_or(CredentialError::Invalid)
    }
}

/// The token is in a mounted file (Q17's interim: a Kubernetes Secret volume).
#[derive(Debug, Clone)]
pub struct MountedFileCredentials {
    path: PathBuf,
    username: String,
}

impl MountedFileCredentials {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            username: DEFAULT_USERNAME.to_owned(),
        }
    }
}

#[async_trait]
impl GitCredentials for MountedFileCredentials {
    async fn credential(&self) -> Result<Option<GitCredential>, CredentialError> {
        let path = self.path.clone();
        // A file read blocks (rule 4). The size is checked after the read; a Secret volume is tiny.
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
            .await
            .map_err(|_| CredentialError::Unavailable)?
            .map_err(|_| CredentialError::Unavailable)?;
        if bytes.len() > MAX_TOKEN_BYTES {
            return Err(CredentialError::Invalid);
        }
        let text = String::from_utf8(bytes).map_err(|_| CredentialError::Invalid)?;
        GitCredential::new(&self.username, Secret::new(text.trim().to_owned()))
            .map(Some)
            .ok_or(CredentialError::Invalid)
    }
}
