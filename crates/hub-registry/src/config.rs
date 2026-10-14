//! Every limit and window of the registry, in one place (rule 5: everything is bounded).
//!
//! Nothing in this crate reads an environment variable: the hub builds a [`RegistryConfig`] from its settings and hands it
//! over, and a test builds one directly. Every field has a default that the tests pin, and [`RegistryConfig::validate`]
//! refuses a combination that would break a contract (a deny glob that is not text a `ShortText` can carry, a chunk of zero
//! paths).

use std::time::Duration;

use engine::{DEFAULT_DENY_GLOBS, DenyMatcher};
use ports::MAX_BLOB_BYTES;

/// The environment variables whose values the agent may report (D88). Anything else is a name only.
pub const DEFAULT_ENV_ALLOWLIST: [&str; 4] = [
    "SPRING_APPLICATION_NAME",
    "SPRING_PROFILES_ACTIVE",
    "CONFIG_CLIENT_CACHE_TTL",
    "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED",
];

/// Why a [`RegistryConfig`] was refused. Names the field, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("setting {0} is out of its range")]
    OutOfRange(&'static str),
    #[error("setting {0} is not valid")]
    Invalid(&'static str),
}

/// Limits and windows. `Default` is the production setting.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryConfig {
    // ---------------------------------------------------------------------------------------------- database
    /// One database call, and one wait for a connection. A statement that runs longer is also cancelled by the server.
    pub query_timeout: Duration,
    /// One put into the blob store (a file of up to `max_file_bytes`). A store that does not answer fails the message instead of
    /// holding the swimlane's lock for ever (rule 5).
    pub blob_timeout: Duration,

    // ---------------------------------------------------------------------------------------------- AgentConfig
    /// How often the agent walks the tree (D63).
    pub scan_interval_secs: u32,
    /// How often the agent sends its Merkle root (P3).
    pub heartbeat_interval_secs: u32,
    /// The largest file the agent reads and sends (2 MiB, `docs/performance.md`).
    pub max_file_bytes: u64,
    /// Deny globs the admin adds to the built-in ones (D79). They can add to the built-in list and never remove from it.
    pub extra_deny_globs: Vec<String>,
    /// Environment variables whose values are kept (D88). A value of any other variable is dropped, whatever the agent sends.
    pub env_allowlist: Vec<String>,

    // ---------------------------------------------------------------------------------------------- scan deltas
    /// Most messages in one logical delta.
    pub max_delta_parts: usize,
    /// Most file entries (versions) and removals in one logical delta, over all its parts.
    pub max_delta_entries: usize,
    /// Most file bytes in one logical delta, over all its parts.
    pub max_delta_bytes: u64,
    /// Paths applied in one transaction. Small, so that the audit chain's lock is held briefly (A12).
    pub chunk_paths: usize,
    /// Swimlanes with a live stream on this replica.
    pub max_streams: usize,
    /// How far in the future an `observed_at` may be before it is clamped to the hub's clock. A wrong agent clock must not
    /// make every later change look older than the stored one.
    pub max_clock_skew: Duration,
    /// How long a staged part waits for the rest of its delta before it is swept.
    pub staging_ttl: Duration,

    // ---------------------------------------------------------------------------------------------- cluster reports
    pub max_services_per_report: usize,
    pub max_pods_per_service: usize,
    pub max_env_names_per_service: usize,
    pub max_release_hints_per_report: usize,
    pub max_sync_windows_per_report: usize,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            query_timeout: Duration::from_secs(10),
            blob_timeout: Duration::from_secs(15),
            scan_interval_secs: 10,
            heartbeat_interval_secs: 10,
            max_file_bytes: 2 * 1024 * 1024,
            extra_deny_globs: Vec::new(),
            env_allowlist: DEFAULT_ENV_ALLOWLIST.iter().map(|s| (*s).to_owned()).collect(),
            max_delta_parts: 64,
            max_delta_entries: 100_000,
            max_delta_bytes: 512 * 1024 * 1024,
            chunk_paths: 50,
            max_streams: 4096,
            max_clock_skew: Duration::from_secs(5 * 60),
            staging_ttl: Duration::from_secs(24 * 3600),
            max_services_per_report: 5_000,
            max_pods_per_service: 500,
            max_env_names_per_service: 1_024,
            max_release_hints_per_report: 5_000,
            max_sync_windows_per_report: 100,
        }
    }
}

impl RegistryConfig {
    /// Checks every field against what the rest of the system can carry.
    pub fn validate(&self) -> Result<(), ConfigError> {
        fn within(name: &'static str, value: u64, low: u64, high: u64) -> Result<(), ConfigError> {
            if (low..=high).contains(&value) {
                Ok(())
            } else {
                Err(ConfigError::OutOfRange(name))
            }
        }
        let millis = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        within("query_timeout", millis(self.query_timeout), 100, 120_000)?;
        within("blob_timeout", millis(self.blob_timeout), 100, 120_000)?;
        within("scan_interval_secs", u64::from(self.scan_interval_secs), 1, 3600)?;
        within(
            "heartbeat_interval_secs",
            u64::from(self.heartbeat_interval_secs),
            1,
            300,
        )?;
        within("max_file_bytes", self.max_file_bytes, 1, MAX_BLOB_BYTES as u64)?;
        within("max_delta_parts", self.max_delta_parts as u64, 1, 4096)?;
        within("max_delta_entries", self.max_delta_entries as u64, 1, 1_000_000)?;
        within("max_delta_bytes", self.max_delta_bytes, 1, 8 * 1024 * 1024 * 1024)?;
        within("chunk_paths", self.chunk_paths as u64, 1, 1_000)?;
        within("max_streams", self.max_streams as u64, 1, 100_000)?;
        within("max_clock_skew", millis(self.max_clock_skew), 0, 24 * 3600 * 1000)?;
        within(
            "staging_ttl",
            millis(self.staging_ttl),
            1_000,
            30 * 24 * 3600 * 1000,
        )?;
        within(
            "max_services_per_report",
            self.max_services_per_report as u64,
            1,
            100_000,
        )?;
        within(
            "max_pods_per_service",
            self.max_pods_per_service as u64,
            1,
            10_000,
        )?;
        within(
            "max_env_names_per_service",
            self.max_env_names_per_service as u64,
            1,
            10_000,
        )?;
        within(
            "max_release_hints_per_report",
            self.max_release_hints_per_report as u64,
            0,
            100_000,
        )?;
        within(
            "max_sync_windows_per_report",
            self.max_sync_windows_per_report as u64,
            0,
            10_000,
        )?;
        // The globs travel to the agent as `ShortText` (256 bytes) and must compile.
        if self.extra_deny_globs.iter().any(|g| g.len() > 256) {
            return Err(ConfigError::Invalid("extra_deny_globs"));
        }
        DenyMatcher::with_globs(self.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        if self.env_allowlist.len() > 64
            || self
                .env_allowlist
                .iter()
                .any(|n| !crate::ingest::cluster::is_env_name(n))
        {
            return Err(ConfigError::Invalid("env_allowlist"));
        }
        Ok(())
    }

    /// The deny globs the agent is told to apply: the built-in ones, then the admin's.
    pub fn deny_globs(&self) -> Vec<String> {
        DEFAULT_DENY_GLOBS
            .iter()
            .map(|g| (*g).to_owned())
            .chain(self.extra_deny_globs.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_pinned() {
        let c = RegistryConfig::default();
        assert_eq!(c.validate(), Ok(()));
        // The numbers the plan and the budgets rely on.
        assert_eq!(
            c.chunk_paths, 50,
            "A12: the audit chain's lock is held for a chunk of 50 paths"
        );
        assert_eq!(
            c.max_file_bytes,
            2 * 1024 * 1024,
            "docs/performance.md: files up to 2 MiB"
        );
        assert_eq!(c.scan_interval_secs, 10, "D63: a stat walk every 10 seconds");
        assert_eq!(c.heartbeat_interval_secs, 10, "P3: never faster than 10 seconds");
        assert_eq!(
            c.staging_ttl,
            Duration::from_secs(24 * 3600),
            "A14: a 24 hour sweep"
        );
        assert_eq!(c.blob_timeout, Duration::from_secs(15), "one put of up to 2 MiB");
        assert_eq!(c.env_allowlist.len(), 4, "D88: four variables keep their value");
        assert!(usize::try_from(c.max_file_bytes).is_ok_and(|v| v <= MAX_BLOB_BYTES));
    }

    #[test]
    fn every_bound_is_enforced() {
        let bad = |f: &dyn Fn(&mut RegistryConfig)| {
            let mut c = RegistryConfig::default();
            f(&mut c);
            c.validate()
        };
        assert_eq!(
            bad(&|c| c.chunk_paths = 0),
            Err(ConfigError::OutOfRange("chunk_paths"))
        );
        assert_eq!(
            bad(&|c| c.max_file_bytes = MAX_BLOB_BYTES as u64 + 1),
            Err(ConfigError::OutOfRange("max_file_bytes"))
        );
        assert_eq!(
            bad(&|c| c.max_delta_parts = 0),
            Err(ConfigError::OutOfRange("max_delta_parts"))
        );
        assert_eq!(
            bad(&|c| c.query_timeout = Duration::from_millis(1)),
            Err(ConfigError::OutOfRange("query_timeout"))
        );
        assert_eq!(
            bad(&|c| c.blob_timeout = Duration::from_millis(1)),
            Err(ConfigError::OutOfRange("blob_timeout"))
        );
        assert_eq!(
            bad(&|c| c.blob_timeout = Duration::from_secs(3600)),
            Err(ConfigError::OutOfRange("blob_timeout"))
        );
        assert_eq!(
            bad(&|c| c.extra_deny_globs = vec!["[".to_owned()]),
            Err(ConfigError::Invalid("extra_deny_globs"))
        );
        assert_eq!(
            bad(&|c| c.extra_deny_globs = vec!["a".repeat(300)]),
            Err(ConfigError::Invalid("extra_deny_globs"))
        );
        assert_eq!(
            bad(&|c| c.env_allowlist = vec!["NOT AN ENV NAME".to_owned()]),
            Err(ConfigError::Invalid("env_allowlist"))
        );
    }

    #[test]
    fn admin_globs_add_to_the_built_in_ones_and_never_replace_them() {
        let c = RegistryConfig {
            extra_deny_globs: vec!["secrets/*".to_owned()],
            ..RegistryConfig::default()
        };
        let globs = c.deny_globs();
        assert_eq!(globs.len(), DEFAULT_DENY_GLOBS.len() + 1);
        assert!(globs.iter().any(|g| g == "*.pem"));
        assert!(globs.iter().any(|g| g == "secrets/*"));
    }
}
