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
    /// Deny globs in addition to the engine's defaults (D79). A write to a matching path is refused.
    pub extra_deny_globs: Vec<String>,
    /// The most rows one run of the reconcile job looks at.
    pub reconcile_batch: usize,
    /// The most validations (parse, secret scan, diff) that run at once on the blocking pool. A request waits for a permit, so a
    /// burst of large files cannot take every thread of the pool (rules 4 and 5).
    pub max_concurrent_checks: usize,
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
            extra_deny_globs: Vec::new(),
            reconcile_batch: 100,
            max_concurrent_checks: 8,
        }
    }
}

/// The most extra deny globs: they are compiled into one set at start.
pub const MAX_EXTRA_DENY_GLOBS: usize = 256;

impl WriteConfig {
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
        nonzero(self.idempotency_wait, "idempotency_wait must be more than zero")?;
        nonzero(self.stale_claim_after, "stale_claim_after must be more than zero")?;
        nonzero(self.proposal_ttl, "proposal_ttl must be more than zero")?;
        if self.reconcile_batch == 0 || self.reconcile_batch > 1_000 {
            return Err(ConfigError::Invalid("reconcile_batch must be 1 to 1000"));
        }
        if self.max_concurrent_checks == 0 || self.max_concurrent_checks > 256 {
            return Err(ConfigError::Invalid("max_concurrent_checks must be 1 to 256"));
        }
        if self.extra_deny_globs.len() > MAX_EXTRA_DENY_GLOBS {
            return Err(ConfigError::Invalid("too many extra deny globs"));
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
        bad(&|c| c.reconcile_batch = 0);
        bad(&|c| c.reconcile_batch = 1_001);
        bad(&|c| c.max_concurrent_checks = 0);
        bad(&|c| c.max_concurrent_checks = 257);
        bad(&|c| c.extra_deny_globs = vec!["*.x".to_owned(); MAX_EXTRA_DENY_GLOBS + 1]);
    }
}
