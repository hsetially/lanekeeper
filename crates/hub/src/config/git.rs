//! The Git mirrors and the webhook (T7 of prompt 03b). All of it is absent unless the remotes are set.
//!
//! The two Secret Manager names are `LK_GIT_READ_REF` and `LK_GIT_WEBHOOK_REF` (plan 18, G9): a variable whose name says
//! `TOKEN` or `SECRET` is what the pod contract flags as a secret in the environment, and these hold a name, not a secret.
//! The token's is `LK_GIT_READ_REF` (the read-only GitHub credential), the webhook secret's `LK_GIT_WEBHOOK_REF`.

use std::path::PathBuf;
use std::time::Duration;

use hub_platform::git::mirror::RemoteUrl;
use hub_platform::git::payload::RepoFullName;

use super::ConfigError;
use super::reader::Reader;

/// Where the read-only Git credential comes from (Q17, A12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCredentialSource {
    /// The token is this secret in Secret Manager (the default, S7).
    SecretManager(String),
    /// The token is in this mounted file (Q17's interim: a Kubernetes Secret volume).
    MountedFile(PathBuf),
}

#[derive(Debug, Clone)]
pub struct GitSettings {
    pub base_url: RemoteUrl,
    pub tenant_url: RemoteUrl,
    /// `owner/name` as GitHub reports it in webhook payloads.
    pub base_repo: RepoFullName,
    pub tenant_repo: RepoFullName,
    /// The name of the webhook secret in Secret Manager.
    pub webhook_secret: String,
    pub credential: GitCredentialSource,
    pub fetch_timeout: Duration,
    /// Distinct pushes that can wait for the fetch worker.
    pub queue_capacity: usize,
    /// Index entries the tree-index cache holds.
    pub tree_index_entries: u64,
}

impl GitSettings {
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let url = |name: &'static str| -> Result<Option<RemoteUrl>, ConfigError> {
            reader
                .parse(name)?
                .map(|v| RemoteUrl::https(&v).map_err(|_| ConfigError::Invalid(name)))
                .transpose()
        };
        let repo = |name: &'static str| -> Result<Option<RepoFullName>, ConfigError> {
            reader
                .parse(name)?
                .map(|v| RepoFullName::parse(&v).map_err(|_| ConfigError::Invalid(name)))
                .transpose()
        };
        let (base_url, tenant_url) = (url("LK_GIT_BASE_URL")?, url("LK_GIT_TENANT_URL")?);
        let (base_repo, tenant_repo) = (repo("LK_GIT_BASE_REPO")?, repo("LK_GIT_TENANT_REPO")?);
        if base_url.is_none() && tenant_url.is_none() && base_repo.is_none() && tenant_repo.is_none() {
            return Ok(None);
        }
        let credential = match (
            reader.parse("LK_GIT_TOKEN_FILE")?,
            reader.parse("LK_GIT_READ_REF")?,
        ) {
            (Some(_), Some(_)) => return Err(ConfigError::Invalid("LK_GIT_TOKEN_FILE")),
            (Some(path), None) => GitCredentialSource::MountedFile(PathBuf::from(path)),
            (None, _) => GitCredentialSource::SecretManager(
                reader.secret_ref_or("LK_GIT_READ_REF", "github-read-token")?,
            ),
        };
        Ok(Some(Self {
            base_url: base_url.ok_or(ConfigError::Missing("LK_GIT_BASE_URL"))?,
            tenant_url: tenant_url.ok_or(ConfigError::Missing("LK_GIT_TENANT_URL"))?,
            base_repo: base_repo.ok_or(ConfigError::Missing("LK_GIT_BASE_REPO"))?,
            tenant_repo: tenant_repo.ok_or(ConfigError::Missing("LK_GIT_TENANT_REPO"))?,
            webhook_secret: reader.secret_ref_or("LK_GIT_WEBHOOK_REF", "github-webhook-secret")?,
            credential,
            fetch_timeout: Duration::from_secs(reader.number("LK_GIT_FETCH_TIMEOUT_SECS", 120, 5, 1800)?),
            queue_capacity: reader.size("LK_GIT_QUEUE_CAPACITY", 256, 1, 100_000)?,
            tree_index_entries: reader.number("LK_TREE_INDEX_ENTRIES", 400_000, 1_000, 50_000_000)?,
        }))
    }
}
