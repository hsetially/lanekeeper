//! In-cluster agent: Merkle scans, spool, cap-std file operations, cluster reports, config-server notify.
//!
//! Owned by prompt 02. Read `AGENTS.md` and `prompts/02*.md` before changing this crate.
//!
//! The agent reports facts and carries out commands; it never decides anything (the hub does). Every file operation
//! goes through the cap-std [`root::NfsRoot`] handle opened at startup (S17).
//!
//! - [`app`]: the loops put together, started and stopped (T7).
//! - [`config`]: the validated environment, and the limits the hub cannot raise (T1).
//! - [`root`]: the NFS export as a cap-std handle (T1).
//! - [`clock`], [`backoff`]: injectable time, and retry delays with full jitter.
//! - [`http`]: the plain-HTTP client for the metadata server and the config-server (T2, T12).
//! - [`identity`]: the key, the certificate, joining the hub and renewing (T2, S5).
//! - [`transport`]: TLS 1.3 to the hub, the gRPC stream, the bounded outbox, and the session that reconnects (T3, S6).
//! - [`fileops`]: reading, writing and deleting files, byte-exact and never blind (T5, S11, S17).
//! - [`dispatch`]: the hub's commands, and the mapping from their outcomes to answers (T5).
//! - [`kube`]: the Deployment, Pod and Job watchers, cluster reports and restarts (T6, S17).
//! - [`process`]: from settings to a running agent and an exit code (T7).
//! - [`ops`]: health probes, Prometheus metrics and JSON logs (T7, S16).
//! - [`spool`]: the durable spool of observed versions, replayed to the hub in order (T9, D74, P15).
//! - [`quiesce`]: when a change is quiet enough to report (T10, D75).
//! - [`scan`]: the scan loop that keeps the tree current, pushes deltas and answers the hub (T4, D63, P1).
//! - [`windows`]: the spans in which a sync Job ran, and telling the hub about them (T10, D72, D75).
//! - [`tree`]: the Merkle tree of the NFS root, its diffs and the ring of recent roots (T4, D63).
#![forbid(unsafe_code)]
// The error enums document themselves, and a one-line accessor does not need `#[must_use]`.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod app;
pub mod backoff;
pub mod clock;
pub mod config;
pub mod dispatch;
pub mod fileops;
pub mod http;
pub mod identity;
pub mod kube;
pub mod ops;
pub mod process;
pub mod quiesce;
pub mod root;
pub mod scan;
pub mod spool;
pub mod transport;
pub mod tree;
pub mod windows;

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
