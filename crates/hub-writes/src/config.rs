//! Settings of the write path. Every number here bounds something: a wait, a size, a batch.

use std::time::Duration;

use engine::Limits;

use crate::error::ConfigError;

/// The name of the secret that holds the key preview tokens are signed with (D46). At least 32 characters; rotating it makes the
/// tokens issued before the change fail to verify, which costs a person one more preview.
pub const PREVIEW_KEY_SECRET: &str = "write-preview-key";

/// What the write hub is configured with. The defaults are the plan's.
#[derive(Debug, Clone)]
pub struct WriteConfig {
    /// The largest content one write carries, in bytes. At most the engine's file limit (2 MiB, `docs/performance.md`);
    /// lower when the agent's `max_file_bytes` is lower.
    pub max_content_bytes: usize,
    /// How long one command to an agent may take before the hub stops waiting (the write may still have run).
    pub agent_timeout: Duration,
    /// How long one database or port call may take.
    pub db_timeout: Duration,
    /// How long an idempotency row lives (D68).
    pub idempotency_ttl: Duration,
    /// How long a request waits for an earlier request with the same key that is still running, before it answers
    /// `Unavailable`.
    pub idempotency_wait: Duration,
    /// How old an unfinished claim is before the reconcile job looks at it: the first request may still be running before that.
    pub stale_claim_after: Duration,
    /// How long a proposal waits for an admin (D37).
    pub proposal_ttl: Duration,
    /// How long a draft lives before it expires.
    pub draft_ttl: Duration,
    /// The most proposals (and as many drafts) one run of the expiry job marks.
    pub expire_batch: usize,
    /// Deny globs in addition to the engine's defaults (D79). A write to a matching path is refused.
    pub extra_deny_globs: Vec<String>,
    /// The most rows one run of the reconcile job looks at.
    pub reconcile_batch: usize,
    /// The most validations (parse, secret scan, diff) that run at once on the blocking pool. A request waits for a permit, so a
    /// burst of large files cannot take every thread of the pool (rules 4 and 5).
    pub max_concurrent_checks: usize,
    /// The most reveals of masked values one user may make in any 60 seconds, on one replica (D79, T10).
    pub reveals_per_minute: u32,
    /// The most users whose reveal window the replica keeps.
    pub reveal_tracked_users: u64,
    /// How long the batcher waits after the first write for more, before it tells the config-server once about all of them
    /// (D85: "batched within 2 seconds").
    pub auto_notify_window: Duration,
    /// The most written paths the batcher holds at once. A write that finds it full is not announced (the "Notify services"
    /// button is still there), so a burst cannot grow the queue.
    pub auto_notify_queue: usize,
    /// How long `WriteService::raise_pr` drives and waits for its pull request job before it answers `Unavailable` (the job goes on
    /// in the background). The bound holds for a slow step too: a step that is still running when it ends is stopped, and the next
    /// try goes on from where the job was.
    pub pr_wait: Duration,
    /// The name of the config-server's Deployment (D83): the one service a restart for check C11 is offered for. The cluster
    /// report lists the Deployment; when it lists none or several of that name, no restart is offered.
    pub config_server_deployment: String,
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self {
            max_content_bytes: Limits::DEFAULT.input_bytes,
            agent_timeout: Duration::from_secs(10),
            db_timeout: Duration::from_secs(10),
            idempotency_ttl: Duration::from_secs(24 * 3600),
            idempotency_wait: Duration::from_secs(3),
            stale_claim_after: Duration::from_secs(60),
            proposal_ttl: Duration::from_secs(72 * 3600),
            draft_ttl: Duration::from_secs(24 * 3600),
            expire_batch: 100,
            extra_deny_globs: Vec::new(),
            reconcile_batch: 100,
            max_concurrent_checks: 8,
            reveals_per_minute: 30,
            reveal_tracked_users: 10_000,
            auto_notify_window: Duration::from_secs(2),
            auto_notify_queue: 1_024,
            pr_wait: Duration::from_secs(30),
            config_server_deployment: "csp-configuration-server".to_owned(),
        }
    }
}

/// The most extra deny globs: they are compiled into one set at start.
pub const MAX_EXTRA_DENY_GLOBS: usize = 256;

/// The longest any row of the write path is kept (an idempotency row, a proposal, a draft): a year. A longer span is a typo, and a
/// span near `Duration::MAX` overflows the `bigint` milliseconds it is added to.
pub const MAX_RETENTION: Duration = Duration::from_secs(366 * 24 * 3600);

/// The longest a request waits for an earlier request with the same key. A wait is held on the request's own task, so it stays
/// short, and `Instant::now() + wait` cannot overflow.
pub const MAX_IDEMPOTENCY_WAIT: Duration = Duration::from_secs(60);

/// The shortest lease on a command to an agent: the plan's minute.
const MIN_COMMAND_LEASE: Duration = Duration::from_secs(60);

impl WriteConfig {
    /// How long a lease is held that guards one command to an agent (a restart, an approval): at least a minute, and longer than
    /// the command can take. The gateway waits `agent_timeout`, the hub's own backstop waits one second more, and the database
    /// calls around the command (the lock check, the recording, the release) take at most `db_timeout` each. A lease that ends
    /// first lets another replica take the same work while the command is still at the agent. A replica that dies in the middle
    /// still frees the work when the lease runs out, so this is no longer than that needs.
    #[must_use]
    pub fn command_lease_ttl(&self) -> Duration {
        self.agent_timeout
            .saturating_add(Duration::from_secs(1))
            .saturating_add(self.db_timeout.saturating_mul(4))
            .max(MIN_COMMAND_LEASE)
    }

    /// Checks the bounds. Called by [`crate::WriteHub::new`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        let nonzero = |d: Duration, what: &'static str| {
            if d.is_zero() {
                Err(ConfigError::Invalid(what))
            } else {
                Ok(())
            }
        };
        if self.max_content_bytes == 0 || self.max_content_bytes > Limits::DEFAULT.input_bytes {
            return Err(ConfigError::Invalid("max_content_bytes must be 1 to 2 MiB"));
        }
        nonzero(self.agent_timeout, "agent_timeout must be more than zero")?;
        nonzero(self.db_timeout, "db_timeout must be more than zero")?;
        nonzero(self.idempotency_ttl, "idempotency_ttl must be more than zero")?;
        if self.idempotency_ttl > MAX_RETENTION {
            return Err(ConfigError::Invalid("idempotency_ttl must be at most 366 days"));
        }
        nonzero(self.idempotency_wait, "idempotency_wait must be more than zero")?;
        if self.idempotency_wait > MAX_IDEMPOTENCY_WAIT {
            return Err(ConfigError::Invalid(
                "idempotency_wait must be at most 60 seconds",
            ));
        }
        nonzero(self.stale_claim_after, "stale_claim_after must be more than zero")?;
        if self.stale_claim_after > self.idempotency_ttl {
            return Err(ConfigError::Invalid(
                "stale_claim_after must not be longer than idempotency_ttl",
            ));
        }
        if self.command_lease_ttl() > ports::MAX_LEASE_TTL {
            return Err(ConfigError::Invalid(
                "agent_timeout and db_timeout are too long for the lease that guards a command",
            ));
        }
        nonzero(self.proposal_ttl, "proposal_ttl must be more than zero")?;
        if self.proposal_ttl > MAX_RETENTION {
            return Err(ConfigError::Invalid("proposal_ttl must be at most 366 days"));
        }
        nonzero(self.draft_ttl, "draft_ttl must be more than zero")?;
        if self.draft_ttl > MAX_RETENTION {
            return Err(ConfigError::Invalid("draft_ttl must be at most 366 days"));
        }
        if self.expire_batch == 0 || self.expire_batch > 1_000 {
            return Err(ConfigError::Invalid("expire_batch must be 1 to 1000"));
        }
        if self.reconcile_batch == 0 || self.reconcile_batch > 1_000 {
            return Err(ConfigError::Invalid("reconcile_batch must be 1 to 1000"));
        }
        if self.max_concurrent_checks == 0 || self.max_concurrent_checks > 256 {
            return Err(ConfigError::Invalid("max_concurrent_checks must be 1 to 256"));
        }
        nonzero(self.pr_wait, "pr_wait must be more than zero")?;
        if self.pr_wait > Duration::from_secs(120) {
            return Err(ConfigError::Invalid("pr_wait must be at most 120 seconds"));
        }
        if self.extra_deny_globs.len() > MAX_EXTRA_DENY_GLOBS {
            return Err(ConfigError::Invalid("too many extra deny globs"));
        }
        if self.reveals_per_minute == 0 || self.reveals_per_minute > 600 {
            return Err(ConfigError::Invalid("reveals_per_minute must be 1 to 600"));
        }
        if self.reveal_tracked_users == 0 || self.reveal_tracked_users > 1_000_000 {
            return Err(ConfigError::Invalid("reveal_tracked_users must be 1 to 1000000"));
        }
        nonzero(
            self.auto_notify_window,
            "auto_notify_window must be more than zero",
        )?;
        if self.auto_notify_window > Duration::from_secs(60) {
            return Err(ConfigError::Invalid(
                "auto_notify_window must be at most 60 seconds",
            ));
        }
        if self.auto_notify_queue == 0 || self.auto_notify_queue > 100_000 {
            return Err(ConfigError::Invalid("auto_notify_queue must be 1 to 100000"));
        }
        if domain::ServiceRef::new("config-server", &self.config_server_deployment).is_err() {
            return Err(ConfigError::Invalid(
                "config_server_deployment must be a Deployment name",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_are_the_plans() {
        let c = WriteConfig::default();
        assert!(c.validate().is_ok());
        assert_eq!(c.max_content_bytes, 2 * 1024 * 1024);
        assert_eq!(c.idempotency_ttl, Duration::from_secs(86_400));
        assert_eq!(c.proposal_ttl, Duration::from_secs(259_200));
        assert_eq!(c.idempotency_wait, Duration::from_secs(3));
        assert_eq!(c.stale_claim_after, Duration::from_secs(60));
        assert_eq!((c.reveals_per_minute, c.reveal_tracked_users), (30, 10_000));
        assert_eq!(c.auto_notify_window, Duration::from_secs(2));
        assert_eq!(c.config_server_deployment, "csp-configuration-server");
    }

    #[test]
    fn a_lease_on_a_command_is_longer_than_the_command() {
        // The default is the minute the plan gave it.
        assert_eq!(
            WriteConfig::default().command_lease_ttl(),
            Duration::from_secs(60)
        );
        // A longer agent timeout makes a longer lease: the gateway's timeout, the hub's extra second and the database calls
        // around the command all fit in it.
        for secs in [30_u64, 59, 60, 90, 300, 600] {
            let c = WriteConfig {
                agent_timeout: Duration::from_secs(secs),
                ..WriteConfig::default()
            };
            assert!(c.validate().is_ok(), "{secs}");
            assert!(
                c.command_lease_ttl() > c.agent_timeout + Duration::from_secs(1) + 2 * c.db_timeout,
                "{secs}: {:?}",
                c.command_lease_ttl()
            );
            assert!(c.command_lease_ttl() <= ports::MAX_LEASE_TTL);
        }
    }

    #[test]
    fn a_timeout_that_no_lease_could_cover_is_refused() {
        let mut c = WriteConfig {
            agent_timeout: Duration::from_secs(3_600),
            ..WriteConfig::default()
        };
        assert!(c.validate().is_err());
        c.agent_timeout = Duration::from_secs(10);
        c.db_timeout = Duration::from_secs(3_600);
        assert!(c.validate().is_err());
    }

    #[test]
    fn a_duration_too_large_to_add_to_a_clock_is_refused() {
        // `Instant::now() + idempotency_wait` and `agent_timeout + 1 s` panic on overflow, and a ttl of `i64::MAX` milliseconds
        // overflows the `bigint` column it is added to; every duration is held to a range the arithmetic around it can take.
        let bad = |f: &dyn Fn(&mut WriteConfig)| {
            let mut c = WriteConfig::default();
            f(&mut c);
            assert!(c.validate().is_err());
        };
        bad(&|c| c.agent_timeout = Duration::MAX);
        bad(&|c| c.db_timeout = Duration::MAX);
        bad(&|c| c.idempotency_wait = Duration::MAX);
        bad(&|c| c.idempotency_wait = Duration::from_secs(61));
        bad(&|c| c.idempotency_ttl = Duration::MAX);
        bad(&|c| c.idempotency_ttl = MAX_RETENTION + Duration::from_secs(1));
        bad(&|c| c.stale_claim_after = Duration::MAX);
        bad(&|c| c.stale_claim_after = c.idempotency_ttl + Duration::from_secs(1));
        bad(&|c| c.proposal_ttl = Duration::MAX);
        bad(&|c| c.proposal_ttl = MAX_RETENTION + Duration::from_secs(1));
        bad(&|c| c.draft_ttl = Duration::MAX);
        bad(&|c| c.draft_ttl = MAX_RETENTION + Duration::from_secs(1));
        bad(&|c| c.auto_notify_window = Duration::MAX);
        bad(&|c| c.pr_wait = Duration::MAX);
        // The edges are still fine.
        let c = WriteConfig {
            idempotency_wait: Duration::from_secs(60),
            idempotency_ttl: MAX_RETENTION,
            stale_claim_after: MAX_RETENTION,
            proposal_ttl: MAX_RETENTION,
            draft_ttl: MAX_RETENTION,
            ..WriteConfig::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn every_bound_is_checked() {
        let bad = |f: &dyn Fn(&mut WriteConfig)| {
            let mut c = WriteConfig::default();
            f(&mut c);
            assert!(c.validate().is_err());
        };
        bad(&|c| c.max_content_bytes = 0);
        bad(&|c| c.max_content_bytes = 2 * 1024 * 1024 + 1);
        bad(&|c| c.agent_timeout = Duration::ZERO);
        bad(&|c| c.db_timeout = Duration::ZERO);
        bad(&|c| c.idempotency_ttl = Duration::ZERO);
        bad(&|c| c.idempotency_wait = Duration::ZERO);
        bad(&|c| c.stale_claim_after = Duration::ZERO);
        bad(&|c| c.proposal_ttl = Duration::ZERO);
        bad(&|c| c.draft_ttl = Duration::ZERO);
        bad(&|c| c.expire_batch = 0);
        bad(&|c| c.expire_batch = 1_001);
        bad(&|c| c.reconcile_batch = 0);
        bad(&|c| c.reconcile_batch = 1_001);
        bad(&|c| c.max_concurrent_checks = 0);
        bad(&|c| c.max_concurrent_checks = 257);
        bad(&|c| c.extra_deny_globs = vec!["*.x".to_owned(); MAX_EXTRA_DENY_GLOBS + 1]);
        bad(&|c| c.reveals_per_minute = 0);
        bad(&|c| c.reveals_per_minute = 601);
        bad(&|c| c.reveal_tracked_users = 0);
        bad(&|c| c.reveal_tracked_users = 1_000_001);
        bad(&|c| c.auto_notify_window = Duration::ZERO);
        bad(&|c| c.auto_notify_window = Duration::from_secs(61));
        bad(&|c| c.auto_notify_queue = 0);
        bad(&|c| c.auto_notify_queue = 100_001);
        bad(&|c| c.pr_wait = Duration::ZERO);
        bad(&|c| c.pr_wait = Duration::from_secs(121));
        bad(&|c| c.config_server_deployment = String::new());
        bad(&|c| c.config_server_deployment = "Not A Name".to_owned());
    }
}
