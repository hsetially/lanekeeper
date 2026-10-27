//! Wiring (plan 18, T3): the composition of the platform, the part of the hub that is not a route.
//!
//! - [`grpc`]: the two certificate authorities over Cloud KMS, and the hub's own certificates from them (S5, S6).
//! - [`platform`]: [`Platform::build`], everything `hub-platform` provides, wired in dependency order, with the agent and replica
//!   listeners serving.
//! - [`jobs`]: the platform's four singleton jobs.
//! - [`tasks`]: the tasks the composition spawns, owned in one place, so that the end of one is seen and the end of all is waited
//!   for.
//!
//! Later tasks add the identity module (T4), the registry and the write paths (T5) and MCP (T6) beside this, and fill the places
//! this leaves open: [`Ingest`], the job runner, and the module list.

pub mod grpc;
pub mod jobs;
pub mod platform;
pub mod tasks;

use hub_platform::ca::profile::ProfileError;
use hub_platform::git::mirror::MirrorError;
use hub_platform::tls::TlsError;
use ports::GitError;

pub use grpc::{Ca, CaKind, CaStep, Cas, prepare_cas};
pub use platform::{Ingest, Platform, PlatformInputs};
pub use tasks::{Tasks, TasksAlive};

/// Why the platform could not be built. Every message is fixed text that names a component or a variable, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("the {} CA is not usable ({}): {step}", .kind.name(), .kind.variables())]
    Ca { kind: CaKind, step: CaStep },
    #[error("a TLS configuration of the agent or replica listener was refused: {0}")]
    Tls(TlsError),
    #[error("the Git mirrors could not be opened (LK_MIRROR_DIR): {0}")]
    Mirror(MirrorError),
    #[error("the Git reader could not be built: {0}")]
    Reader(GitError),
    #[error("the {0} gRPC listener has no address")]
    Listener(&'static str),
    #[error("the job {0} could not be registered")]
    Job(&'static str),
    #[error("the replica's certificate profile is not valid: {0}")]
    Profile(ProfileError),
}
