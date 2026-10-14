//! Every limit and window of the registry, in one place (rule 5: everything is bounded).
//!
//! Nothing in this crate reads an environment variable: the hub builds a [`RegistryConfig`] from its settings and hands it
//! over, and a test builds one directly. Every field has a default that the tests pin, and [`RegistryConfig::validate`]
//! refuses a combination that would break a contract (a deny glob that is not text a `ShortText` can carry, a chunk of zero
//! paths).

use std::time::Duration;

use engine::{DEFAULT_DENY_GLOBS, DenyMatcher};
use ports::{MAX_BLOB_BYTES, MAX_GET_MANY};

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

    // ---------------------------------------------------------------------------------------------- indexes (T2)
    /// Threads of the registry's rayon pool, which parses blobs for the settings index (rule 4: CPU work is rayon inside
    /// `spawn_blocking`). Small on purpose: indexing is off the acknowledgement path and must not starve the readers.
    pub pool_threads: usize,
    /// Blob hashes waiting for the settings index on this replica. A hash offered to a full queue is dropped and counted; the
    /// sweeper finds it again, so the bound costs latency and never an index row.
    pub index_queue: usize,
    /// Blobs indexed per transaction (and read from the blob store per call).
    pub index_batch: usize,
    /// How often the leased sweeper looks for current structured blobs that have no index marker.
    pub index_sweep_interval: Duration,
    /// The most blobs one sweep run indexes, so a run ends well inside its `max_run`; the next run goes on.
    pub index_sweep_max_blobs: usize,
    /// Rows per statement when the registry writes many rows at once (settings rows, effective files and directories).
    pub rows_per_statement: usize,
    /// The most paths of one swimlane that a moved Git head may name before the swimlane is recomputed whole (A21).
    pub max_affected_paths: usize,
    /// The bytes the cache of normalised Git hashes may weigh (A23). A key weighs its text and a fixed overhead.
    pub git_hash_cache_bytes: u64,
    /// One Git operation (a tree diff, a tree index, a blob read), as seen by the caller. The operation itself runs in
    /// `spawn_blocking` and cannot be cancelled, so a timeout frees the caller and not the thread.
    pub git_timeout: Duration,

    // ---------------------------------------------------------------------------------------------- the pipeline (T3)
    /// How long a swimlane's changes are gathered before they are recomputed together (D66, P13): a burst is one recompute.
    pub coalesce_window: Duration,
    /// Swimlanes with changes waiting in the coalescer. Past it a swimlane's changes are not kept; the next reconcile finds them.
    pub max_coalesced_lanes: usize,
    /// Swimlanes whose state the pipeline keeps in memory (hashes and findings, never content). The least recently used goes first.
    pub max_lanes_in_memory: usize,
    /// How often the pipeline compares its memory with the database and with Git, and so finds what an event missed.
    pub reconcile_interval: Duration,
    /// How often it looks for a swimlane whose first inventory is done and that has not been computed yet.
    pub inventory_poll_interval: Duration,
    /// In-process signals (a baseline or a mark changed) waiting for the pipeline. A full queue drops the signal and counts it; the
    /// reconcile finds the change.
    pub signal_queue: usize,
    /// How long one run of the pipeline job goes on before it ends and starts again (always shorter than the job's `max_run`).
    pub pipeline_run: Duration,
    /// Swimlane commits in flight at once (each is one transaction, on its own connection). A push to the base repository concerns every
    /// swimlane, and what a commit mostly waits for is the disk.
    pub persist_concurrency: usize,
    /// How often the leased nightly job runs, and how many swimlanes it recomputes in one batch (a batch holds the content of
    /// the distinct files of its swimlanes in memory).
    pub nightly_interval: Duration,
    pub nightly_batch_lanes: usize,

    // ---------------------------------------------------------------------------------------------- syncs (T4)
    /// How long `record_sync` waits for the full scan it asked for to be applied before it gives up. The scan stays requested (the
    /// swimlane is flagged, so the next heartbeat asks again) and nothing is written: the caller reports the sync again.
    pub sync_scan_timeout: Duration,
    /// How often `record_sync` looks whether the scan has been applied.
    pub sync_poll_interval: Duration,
    /// How long the request for the scan may take to reach the agent's stream (the gateway acknowledges it at once).
    pub sync_request_timeout: Duration,
    /// How long a notification may take (the notifier talks to a webhook): a slow channel is logged and the report goes on (rule 5).
    pub notify_timeout: Duration,
    /// Branches kept in the ranking of a tenant-suffix review (`adoptions.ranking`), best first.
    pub rematch_ranking: usize,
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
            pool_threads: 2,
            index_queue: 4_096,
            index_batch: 32,
            index_sweep_interval: Duration::from_secs(5 * 60),
            index_sweep_max_blobs: 20_000,
            rows_per_statement: 5_000,
            max_affected_paths: 5_000,
            git_hash_cache_bytes: 24 * 1024 * 1024,
            git_timeout: Duration::from_secs(30),
            coalesce_window: Duration::from_millis(200),
            max_coalesced_lanes: 10_000,
            max_lanes_in_memory: 256,
            reconcile_interval: Duration::from_secs(60),
            inventory_poll_interval: Duration::from_secs(5),
            signal_queue: 4_096,
            pipeline_run: Duration::from_secs(50 * 60),
            persist_concurrency: 4,
            nightly_interval: Duration::from_secs(24 * 3600),
            nightly_batch_lanes: 4,
            sync_scan_timeout: Duration::from_secs(120),
            sync_poll_interval: Duration::from_millis(250),
            sync_request_timeout: Duration::from_secs(10),
            notify_timeout: Duration::from_secs(10),
            rematch_ranking: 20,
        }
    }
}

impl RegistryConfig {
    /// Checks every field against what the rest of the system can carry.
    #[allow(clippy::too_many_lines)] // one range check per field, in the order of the fields
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
        within("pool_threads", self.pool_threads as u64, 1, 64)?;
        within("index_queue", self.index_queue as u64, 1, 1_000_000)?;
        within("index_batch", self.index_batch as u64, 1, MAX_GET_MANY as u64)?;
        within(
            "index_sweep_interval",
            millis(self.index_sweep_interval),
            10_000,
            24 * 3600 * 1000,
        )?;
        within(
            "index_sweep_max_blobs",
            self.index_sweep_max_blobs as u64,
            1,
            10_000_000,
        )?;
        within("rows_per_statement", self.rows_per_statement as u64, 100, 50_000)?;
        within("max_affected_paths", self.max_affected_paths as u64, 1, 100_000)?;
        within(
            "git_hash_cache_bytes",
            self.git_hash_cache_bytes,
            1024 * 1024,
            1024 * 1024 * 1024,
        )?;
        within("git_timeout", millis(self.git_timeout), 100, 600_000)?;
        within("coalesce_window", millis(self.coalesce_window), 10, 10_000)?;
        within(
            "max_coalesced_lanes",
            self.max_coalesced_lanes as u64,
            1,
            1_000_000,
        )?;
        within("max_lanes_in_memory", self.max_lanes_in_memory as u64, 1, 10_000)?;
        within(
            "reconcile_interval",
            millis(self.reconcile_interval),
            1_000,
            24 * 3600 * 1000,
        )?;
        within(
            "inventory_poll_interval",
            millis(self.inventory_poll_interval),
            100,
            3600 * 1000,
        )?;
        within("signal_queue", self.signal_queue as u64, 1, 1_000_000)?;
        // The job's own `max_run` is this plus a minute, and the runner allows an hour at most.
        within("pipeline_run", millis(self.pipeline_run), 1_000, 55 * 60 * 1000)?;
        within(
            "nightly_interval",
            millis(self.nightly_interval),
            60_000,
            7 * 24 * 3600 * 1000,
        )?;
        within("persist_concurrency", self.persist_concurrency as u64, 1, 16)?;
        within("nightly_batch_lanes", self.nightly_batch_lanes as u64, 1, 1_000)?;
        within(
            "sync_scan_timeout",
            millis(self.sync_scan_timeout),
            1_000,
            3600 * 1000,
        )?;
        within("sync_poll_interval", millis(self.sync_poll_interval), 10, 10_000)?;
        within(
            "sync_request_timeout",
            millis(self.sync_request_timeout),
            100,
            120_000,
        )?;
        within("notify_timeout", millis(self.notify_timeout), 100, 120_000)?;
        within("rematch_ranking", self.rematch_ranking as u64, 1, 100)?;
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
    fn the_index_defaults_are_pinned() {
        let c = RegistryConfig::default();
        assert_eq!(c.pool_threads, 2, "the plan's registry pool: 2 threads");
        assert_eq!(c.index_queue, 4_096, "a bounded queue of blob hashes");
        assert_eq!(c.index_batch, 32);
        assert_eq!(c.index_sweep_interval, Duration::from_secs(300));
        assert_eq!(c.rows_per_statement, 5_000);
        assert_eq!(
            c.max_affected_paths, 5_000,
            "past this a swimlane is recomputed whole"
        );
        assert_eq!(
            c.git_hash_cache_bytes,
            24 * 1024 * 1024,
            "about 20 MB of normalised hashes (plan, risk 6)"
        );
        assert!(c.index_batch <= MAX_GET_MANY, "one get_many per batch");
    }

    #[test]
    fn the_pipeline_defaults_are_pinned() {
        let c = RegistryConfig::default();
        assert_eq!(
            c.coalesce_window,
            Duration::from_millis(200),
            "the prompt: bursts are coalesced per swimlane in 200 ms windows"
        );
        assert_eq!(c.max_lanes_in_memory, 256);
        assert_eq!(c.reconcile_interval, Duration::from_secs(60));
        assert_eq!(c.nightly_interval, Duration::from_secs(24 * 3600));
        assert!(
            c.pipeline_run < Duration::from_secs(3600),
            "a run ends before the runner's one hour limit on a run"
        );
        assert!(c.nightly_batch_lanes >= 1 && c.persist_concurrency >= 1);
    }

    #[test]
    fn every_pipeline_bound_is_enforced() {
        out_of_range("coalesce_window", |c| {
            c.coalesce_window = Duration::from_millis(1);
        });
        out_of_range("max_coalesced_lanes", |c| {
            c.max_coalesced_lanes = 0;
        });
        out_of_range("max_lanes_in_memory", |c| {
            c.max_lanes_in_memory = 0;
        });
        out_of_range("reconcile_interval", |c| {
            c.reconcile_interval = Duration::from_millis(5);
        });
        out_of_range("inventory_poll_interval", |c| {
            c.inventory_poll_interval = Duration::from_millis(1);
        });
        out_of_range("signal_queue", |c| {
            c.signal_queue = 0;
        });
        out_of_range("pipeline_run", |c| {
            c.pipeline_run = Duration::from_secs(3600);
        });
        out_of_range("persist_concurrency", |c| {
            c.persist_concurrency = 0;
        });
        out_of_range("persist_concurrency", |c| {
            c.persist_concurrency = 17;
        });
        out_of_range("nightly_interval", |c| {
            c.nightly_interval = Duration::from_secs(1);
        });
        out_of_range("nightly_batch_lanes", |c| {
            c.nightly_batch_lanes = 0;
        });
    }

    #[test]
    fn the_sync_defaults_are_pinned_and_their_bounds_enforced() {
        let c = RegistryConfig::default();
        assert_eq!(
            c.sync_scan_timeout,
            Duration::from_secs(120),
            "a full scan of a swimlane takes the agent a walk of 10 s and a quiet period of 3 s"
        );
        assert_eq!(c.sync_poll_interval, Duration::from_millis(250));
        assert_eq!(c.sync_request_timeout, Duration::from_secs(10));
        assert_eq!(c.notify_timeout, Duration::from_secs(10));
        assert_eq!(c.rematch_ranking, 20);
        assert!(c.sync_poll_interval < c.sync_scan_timeout);
        out_of_range("sync_scan_timeout", |c| {
            c.sync_scan_timeout = Duration::from_millis(10);
        });
        out_of_range("sync_scan_timeout", |c| {
            c.sync_scan_timeout = Duration::from_secs(3601);
        });
        out_of_range("sync_poll_interval", |c| {
            c.sync_poll_interval = Duration::from_millis(1);
        });
        out_of_range("sync_poll_interval", |c| {
            c.sync_poll_interval = Duration::from_secs(11);
        });
        out_of_range("sync_request_timeout", |c| {
            c.sync_request_timeout = Duration::from_millis(1);
        });
        out_of_range("sync_request_timeout", |c| {
            c.sync_request_timeout = Duration::from_secs(3600);
        });
        out_of_range("notify_timeout", |c| {
            c.notify_timeout = Duration::from_millis(1);
        });
        out_of_range("notify_timeout", |c| {
            c.notify_timeout = Duration::from_secs(3600);
        });
        out_of_range("rematch_ranking", |c| {
            c.rematch_ranking = 0;
        });
        out_of_range("rematch_ranking", |c| {
            c.rematch_ranking = 101;
        });
    }

    fn out_of_range(name: &'static str, change: impl FnOnce(&mut RegistryConfig)) {
        let mut c = RegistryConfig::default();
        change(&mut c);
        assert_eq!(c.validate(), Err(ConfigError::OutOfRange(name)), "{name}");
    }

    #[test]
    fn every_index_and_git_bound_is_enforced() {
        out_of_range("pool_threads", |c| {
            c.pool_threads = 0;
        });
        out_of_range("pool_threads", |c| {
            c.pool_threads = 65;
        });
        out_of_range("index_queue", |c| {
            c.index_queue = 0;
        });
        out_of_range("index_batch", |c| {
            c.index_batch = 0;
        });
        out_of_range("index_batch", |c| {
            c.index_batch = MAX_GET_MANY + 1;
        });
        out_of_range("index_sweep_interval", |c| {
            c.index_sweep_interval = Duration::from_secs(1);
        });
        out_of_range("index_sweep_max_blobs", |c| {
            c.index_sweep_max_blobs = 0;
        });
        out_of_range("rows_per_statement", |c| {
            c.rows_per_statement = 99;
        });
        out_of_range("rows_per_statement", |c| {
            c.rows_per_statement = 50_001;
        });
        out_of_range("max_affected_paths", |c| {
            c.max_affected_paths = 0;
        });
        out_of_range("git_hash_cache_bytes", |c| {
            c.git_hash_cache_bytes = 1024;
        });
        out_of_range("git_timeout", |c| {
            c.git_timeout = Duration::from_millis(1);
        });
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
