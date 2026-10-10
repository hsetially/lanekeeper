//! In-cluster agent: Merkle scans, spool, cap-std file operations, cluster reports, config-server notify.
//!
//! Owned by prompt 02. Read `AGENTS.md` and `prompts/02*.md` before changing this crate.
//!
//! The agent reports facts and carries out commands; it never decides anything (the hub does). Every file operation
//! goes through the cap-std [`root::NfsRoot`] handle opened at startup (S17).
#![forbid(unsafe_code)]
// The error enums document themselves, and a one-line accessor does not need `#[must_use]`.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod clock;
pub mod config;
pub mod root;

pub use root::{NfsRoot, StartupError};

use config::{EnvSource, Settings};

/// What a successful start leaves the rest of the agent with.
#[derive(Debug)]
pub struct Started {
    pub settings: Settings,
    pub root: NfsRoot,
}

/// Read the environment and open the NFS root. Fails fast, with a clear error, if either is wrong (T1).
pub fn startup<E: EnvSource + ?Sized>(env: &E) -> Result<Started, StartupError> {
    let settings = Settings::from_env(env)?;
    let root = NfsRoot::open(&settings.nfs.root)?;
    Ok(Started { settings, root })
}
