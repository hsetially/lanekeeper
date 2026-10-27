//! The registry's settings (plan 18, T5, G9, G16; S11, S17, D79, D116, D118): the few knobs of `RegistryConfig` that a deployment
//! sets, over its documented defaults.
//!
//! `RegistryConfig` has well over a hundred fields, each with a default that the registry's tests pin. Exposing them all would be a
//! hundred variables nobody has measured a reason to change, so the hub exposes four and leaves the rest at their defaults:
//!
//! - `LK_EXTRA_DENY_GLOBS`: globs added to the built-in deny list (D79). **One list**: the registry (what an agent is told not to
//!   read, and what the read API withholds), the blob endpoint's policy and the write path are all built from it, so a path denied for
//!   one is denied for all.
//! - `LK_RETENTION_ENABLED` (off) and `LK_RETENTION_DRY_RUN` (on): the retention job deletes blobs, and D116 and D118 are still to be
//!   confirmed at review, so the job is not registered unless a person switches it on, and when it is, it reports and deletes nothing
//!   until a person switches the dry run off.
//! - `LK_CORPUS_CACHE_MIB`: what the text-search corpora may hold in memory on each replica (256 by default).
//!
//! The result is validated here, by the registry's own `validate`, so a value it refuses is a refused setting that names the
//! variable, and not a start that fails after the database was reached.

use hub_registry::{ConfigError as RegistryConfigError, RegistryConfig};

use super::ConfigError;
use super::reader::Reader;

/// The most extra deny globs: the write path compiles the same list into one set, and refuses more than this.
const MAX_DENY_GLOBS: usize = hub_writes::MAX_EXTRA_DENY_GLOBS;
/// A deny glob is a path pattern; `RegistryConfig` refuses one over 256 characters.
const MAX_DENY_GLOB_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RegistrySettings {
    /// The registry's limits and windows: the defaults, and the knobs above.
    pub config: RegistryConfig,
    /// Whether `registry-retention` is registered at all (G16).
    pub retention_enabled: bool,
}

impl RegistrySettings {
    /// The deny globs, for the write path (which keeps its own copy in its own configuration).
    #[must_use]
    pub fn deny_globs(&self) -> Vec<String> {
        self.config.extra_deny_globs.clone()
    }

    pub(super) fn read(reader: &Reader<'_>) -> Result<Self, ConfigError> {
        let mut config = RegistryConfig {
            retention_dry_run: reader.flag("LK_RETENTION_DRY_RUN", true)?,
            corpus_cache_bytes: reader
                .number("LK_CORPUS_CACHE_MIB", 256, 16, 4096)?
                .saturating_mul(1024 * 1024),
            ..RegistryConfig::default()
        };
        if let Some(globs) = reader.list("LK_EXTRA_DENY_GLOBS", MAX_DENY_GLOBS, MAX_DENY_GLOB_LEN)? {
            config.extra_deny_globs = globs;
        }
        let retention_enabled = reader.flag("LK_RETENTION_ENABLED", false)?;
        config.validate().map_err(|error| {
            // The field names the variable; anything else cannot come from these four knobs over the defaults.
            let (RegistryConfigError::OutOfRange(field) | RegistryConfigError::Invalid(field)) = error;
            ConfigError::Invalid(match field {
                "corpus_cache_bytes" => "LK_CORPUS_CACHE_MIB",
                "retention_dry_run" => "LK_RETENTION_DRY_RUN",
                _ => "LK_EXTRA_DENY_GLOBS",
            })
        })?;
        Ok(Self {
            config,
            retention_enabled,
        })
    }
}
