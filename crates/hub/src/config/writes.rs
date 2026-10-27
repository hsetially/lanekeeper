//! The write paths' settings (plan 18, T5, G7, G9; S7, S8, S18): the key that wraps the GitHub token vault, where GitHub is, which
//! branch a base pull request targets, the Teams hosts, and how often the four background jobs run.
//!
//! Nothing here is a secret. The vault's key is a Cloud KMS key the hub only asks to wrap and unwrap (a resource name), the
//! preview key is the Secret Manager secret `write-preview-key` (a constant of `hub-writes`), the Teams webhook URLs are secrets
//! named by `notification_settings.destination_ref` in the database, and the GitHub tokens are the users' own, stored encrypted.
//!
//! `LK_VAULT_KMS_REF` is the one required variable: a hub with no vault cannot store a user's token, and so cannot raise a pull
//! request, and it does not start in a state where a write path is half there. The others have defaults.
//!
//! **The jobs (G7).** `hub-writes` has four workers that are bare `run_once` methods; the hub gives each a lease of its own name and an
//! interval. The intervals below are the plan's numbers and are settings within bounds; the lease times are constants (a few intervals,
//! and long enough for the longest unit of work the worker does between asking `still_held`).

use std::time::Duration;

use hub_platform::gcp::wire::CryptoKeyName;
use hub_writes::github::BranchName;
use ports::KeyRef;

use super::ConfigError;
use super::reader::Reader;

/// Where GitHub's API is, unless `LK_GITHUB_API_BASE` says otherwise (a GitHub Enterprise Server names its own).
pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
/// The branch a base-targeted pull request goes to, unless `LK_GIT_BASE_BRANCH` says otherwise.
pub const DEFAULT_BASE_BRANCH: &str = "main";
/// The Deployment name of the config-server (D83), unless `LK_CONFIG_SERVER_DEPLOYMENT` says otherwise.
pub const DEFAULT_CONFIG_SERVER_DEPLOYMENT: &str = "csp-configuration-server";
/// The most Teams host patterns, and the longest.
const MAX_TEAMS_HOSTS: usize = 16;
const MAX_HOST_LEN: usize = 253;

/// How often each worker of `hub-writes` runs (the holder of its lease), and the lease times, which are not settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteJobTimes {
    /// Finish or clear the writes that landed without being recorded, and sweep expired idempotency rows (D63, D68).
    pub reconcile: Duration,
    /// Mark proposals and drafts that nobody decided as expired (D37).
    pub expire: Duration,
    /// Take up the pull-request jobs a crash or a restart left, and the ones an approval made (D77).
    pub pr_jobs: Duration,
    /// Delete the GitHub credentials of users not seen for 30 days (S8).
    pub cleanup: Duration,
}

impl Default for WriteJobTimes {
    fn default() -> Self {
        Self {
            reconcile: Duration::from_secs(30),
            expire: Duration::from_secs(60),
            pr_jobs: Duration::from_secs(5),
            cleanup: Duration::from_secs(3600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WritesSettings {
    /// The Cloud KMS crypto key (not a version) that wraps the data key of each stored GitHub token (S8).
    pub vault_key: KeyRef,
    /// The origin of GitHub's API. The client may reach this host and no other.
    pub github_api_base: String,
    /// The host of `github_api_base`, which is the one host the client is allowed.
    pub github_api_host: String,
    /// The default branch of the base repository.
    pub base_branch: BranchName,
    /// The config-server's Deployment name (D83), for the restart that check C11 asks for.
    pub config_server_deployment: String,
    /// The host patterns a Teams webhook URL may name. `None`: `hub-writes`' defaults (the Microsoft Workflows hosts).
    pub teams_hosts: Option<Vec<String>>,
    pub jobs: WriteJobTimes,
}

impl WritesSettings {
    /// `None` when `LK_VAULT_KMS_REF` is not set; serving refuses that. The other variables are read and checked either way, so a
    /// malformed one is refused in a half-written pod too.
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let vault_key = match reader.parse("LK_VAULT_KMS_REF")? {
            None => None,
            Some(text) => {
                let key = KeyRef::parse(&text).map_err(|_| ConfigError::Invalid("LK_VAULT_KMS_REF"))?;
                // The key, not a version: KMS picks the primary version to wrap with and finds the right one to unwrap, so a key
                // can be rotated without a change here.
                CryptoKeyName::parse(&key).ok_or(ConfigError::Invalid("LK_VAULT_KMS_REF"))?;
                Some(key)
            }
        };
        let github_api_base = reader
            .parse("LK_GITHUB_API_BASE")?
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE.to_owned());
        let github_api_host = api_host(&github_api_base).ok_or(ConfigError::Invalid("LK_GITHUB_API_BASE"))?;
        let base_branch = BranchName::parse(
            &reader
                .parse("LK_GIT_BASE_BRANCH")?
                .unwrap_or_else(|| DEFAULT_BASE_BRANCH.to_owned()),
        )
        .map_err(|_| ConfigError::Invalid("LK_GIT_BASE_BRANCH"))?;
        let config_server_deployment = reader
            .parse("LK_CONFIG_SERVER_DEPLOYMENT")?
            .unwrap_or_else(|| DEFAULT_CONFIG_SERVER_DEPLOYMENT.to_owned());
        domain::ServiceRef::new("config-server", &config_server_deployment)
            .map_err(|_| ConfigError::Invalid("LK_CONFIG_SERVER_DEPLOYMENT"))?;
        let teams_hosts = reader.list("LK_TEAMS_ALLOWED_HOSTS", MAX_TEAMS_HOSTS, MAX_HOST_LEN)?;
        if teams_hosts.iter().flatten().any(|host| !is_host_pattern(host)) {
            return Err(ConfigError::Invalid("LK_TEAMS_ALLOWED_HOSTS"));
        }
        let defaults = WriteJobTimes::default();
        let secs =
            |name: &'static str, default: Duration, min: u64, max: u64| -> Result<Duration, ConfigError> {
                Ok(Duration::from_secs(reader.number(
                    name,
                    default.as_secs(),
                    min,
                    max,
                )?))
            };
        let jobs = WriteJobTimes {
            reconcile: secs("LK_WRITES_RECONCILE_SECS", defaults.reconcile, 1, 3600)?,
            expire: secs("LK_WRITES_EXPIRE_SECS", defaults.expire, 1, 86_400)?,
            pr_jobs: secs("LK_WRITES_PR_JOBS_SECS", defaults.pr_jobs, 1, 300)?,
            cleanup: secs("LK_WRITES_CLEANUP_SECS", defaults.cleanup, 60, 7 * 86_400)?,
        };
        Ok(vault_key.map(|vault_key| Self {
            vault_key,
            github_api_base,
            github_api_host,
            base_branch,
            config_server_deployment,
            teams_hosts,
            jobs,
        }))
    }
}

/// The host of an API base that is `https://host` and nothing more (no user, port, path, query or fragment), in lower case.
fn api_host(base: &str) -> Option<String> {
    let rest = base.strip_prefix("https://")?;
    let host = rest.strip_suffix('/').unwrap_or(rest);
    (is_host(host) && !host.is_empty()).then(|| host.to_owned())
}

/// A DNS name in lower case: labels of letters, digits and hyphens, no empty label, no leading or trailing hyphen.
fn is_host(host: &str) -> bool {
    host.len() <= MAX_HOST_LEN
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// A host, or `*.` and a host (a Teams webhook is on a tenant-specific name under a Microsoft domain).
fn is_host_pattern(pattern: &str) -> bool {
    is_host(pattern.strip_prefix("*.").unwrap_or(pattern))
}
