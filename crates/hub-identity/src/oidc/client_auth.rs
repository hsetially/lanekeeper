//! How the hub authenticates to Entra's token endpoint (S1, S7, Q25).
//!
//! - **Federated (preferred):** the client assertion is the projected Kubernetes service-account token that Entra trusts through
//!   a federated credential. The kubelet rotates the file, so it is read again on every exchange and never cached.
//! - **Secret:** the client secret from `SecretSource`, read again on every exchange as well, so a rotation needs no restart.
//!
//! Values come back as `Secret<String>`: redacted in `Debug`, zeroized on drop.

use std::sync::Arc;

use domain::Secret;
use ports::SecretSource;
use tokio::io::AsyncReadExt;

use crate::config::ClientAuth;
use crate::error::IdentityError;

/// The `client_assertion_type` for a JWT assertion (RFC 7523).
pub const JWT_BEARER_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
/// The longest assertion file read. A projected token is about 1 to 2 KiB.
const MAX_ASSERTION_BYTES: u64 = 16 * 1024;

/// The credential half of a token request.
pub struct ClientCredentials {
    auth: ClientAuth,
    secrets: Arc<dyn SecretSource>,
}

impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCredentials")
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl ClientCredentials {
    pub fn new(auth: ClientAuth, secrets: Arc<dyn SecretSource>) -> Self {
        Self { auth, secrets }
    }

    /// The form fields that authenticate the client, freshly read.
    pub async fn fields(&self) -> Result<Vec<(&'static str, Secret<String>)>, IdentityError> {
        match &self.auth {
            ClientAuth::Secret { name } => Ok(vec![("client_secret", self.secrets.get(name).await?)]),
            ClientAuth::Federated { token_file } => {
                let assertion = read_assertion(token_file).await?;
                Ok(vec![
                    (
                        "client_assertion_type",
                        Secret::new(JWT_BEARER_ASSERTION_TYPE.to_owned()),
                    ),
                    ("client_assertion", assertion),
                ])
            }
        }
    }
}

async fn read_assertion(path: &str) -> Result<Secret<String>, IdentityError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| IdentityError::ClientAssertion)?;
    let mut raw = Vec::new();
    // One byte more than allowed, so an oversize file is told apart from one of exactly the limit.
    file.take(MAX_ASSERTION_BYTES + 1)
        .read_to_end(&mut raw)
        .await
        .map_err(|_| IdentityError::ClientAssertion)?;
    let text = String::from_utf8(raw).map_err(|_| IdentityError::ClientAssertion)?;
    let token = text.trim();
    // A JWT is three base64url segments. Anything else is not a token the kubelet wrote.
    let looks_like_jwt = !token.is_empty()
        && token.len() as u64 <= MAX_ASSERTION_BYTES
        && token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'));
    if looks_like_jwt {
        Ok(Secret::new(token.to_owned()))
    } else {
        Err(IdentityError::ClientAssertion)
    }
}
