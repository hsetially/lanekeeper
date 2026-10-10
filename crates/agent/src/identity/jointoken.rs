//! The join-token fallback (S5, Q26): a one-time token an Admin issued, kept in a Secret, for clusters without Workload
//! Identity.
//!
//! The agent may `get` that one Secret by name (A2). It reads it each time a join needs it and keeps nothing: the token
//! is single-use, so after a join consumes it an Admin must issue a new one (A23).

use std::fmt;

use async_trait::async_trait;
use domain::Secret;
use k8s_openapi::api::core::v1::Secret as KubeSecret;
use kube::Client;
use kube::api::Api;

use super::error::JoinTokenError;
use super::kubecall::kube_call;
use crate::config::KubeName;

/// The entry of the Secret that holds the token.
const TOKEN_ENTRY: &str = "token";
const READ: &str = "read the join token Secret";

#[async_trait]
pub trait JoinTokenSource: Send + Sync + fmt::Debug {
    /// The current join token.
    async fn join_token(&self) -> Result<Secret<String>, JoinTokenError>;
}

/// The join-token Secret in the agent's own namespace.
pub struct KubeSecretJoinToken {
    api: Api<KubeSecret>,
    name: String,
}

impl KubeSecretJoinToken {
    pub fn new(client: Client, name: &KubeName) -> Self {
        Self {
            api: Api::default_namespaced(client),
            name: name.as_str().to_owned(),
        }
    }
}

impl fmt::Debug for KubeSecretJoinToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KubeSecretJoinToken")
            .field("secret", &self.name)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl JoinTokenSource for KubeSecretJoinToken {
    async fn join_token(&self) -> Result<Secret<String>, JoinTokenError> {
        let secret = kube_call(READ, self.api.get_opt(&self.name))
            .await?
            .ok_or(JoinTokenError::SecretMissing)?;
        let bytes = secret
            .data
            .as_ref()
            .and_then(|d| d.get(TOKEN_ENTRY))
            .ok_or(JoinTokenError::Missing)?;
        let text = std::str::from_utf8(&bytes.0).map_err(|_| JoinTokenError::Missing)?;
        let token = text.trim_end_matches(['\r', '\n']);
        // The hub's tokens are opaque, so only the shape is checked: one printable word, within the contract's limit.
        let printable = token.bytes().all(|b| b.is_ascii_graphic());
        if token.is_empty() || token.len() > proto::limits::MAX_TOKEN_BYTES || !printable {
            return Err(JoinTokenError::Missing);
        }
        Ok(Secret::new(token.to_owned()))
    }
}
