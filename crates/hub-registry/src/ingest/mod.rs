//! Ingestion (T1): what the hub does with what agents send.
//!
//! [`AgentIngest`] implements [`ports::ReportSink`]. It is the only writer of the *source facts* of a swimlane's NFS side: the
//! blobs and observations of its files, the NFS columns of `file_state`, the Merkle root, the Deployments with their pods and
//! environment names, the release hints and the sync windows. Everything derived from them (drift, effective config, findings)
//! is the pipeline's (T3) and is recomputed from these facts and Git.
//!
//! | Message | What happens |
//! |---|---|
//! | `hello` | the identity's swimlane must equal the payload's and an admin must have created it; the agent's facts are stored; staged parts of an earlier stream are dropped; the answer is the [`domain::AgentConfig`] |
//! | `heartbeat` | the agent's root is compared with the hub's: unknown (or a scan the hub still needs) asks for a full scan, equal asks for nothing, different asks for a delta since the hub's root |
//! | `delta` | validated before anything is stored; blobs stored (idempotent); a part with `more` is staged durably; the last part applies the whole logical delta in chunks and moves the root last |
//! | `cluster` | Deployments, pods, environment names (values only for the D88 allowlist), release hints, the config-server's start time and the sync windows |
//!
//! Every state change commits with its audit event and its outbox events in one transaction (D80); nothing is fanned out before
//! the commit.
//!
//! # Where the pieces of the plan live
//!
//! | Piece | Module |
//! |---|---|
//! | `AgentIngest: ReportSink` | `sink` (the trait impl), `hello`, `heartbeat` |
//! | `DeltaAssembler` | `delta` (the state machine for multi-part deltas), `staging` (the durable parts), `validate` (the checks and the bounds), `apply` (the chunked apply that moves the root last), `plan` (what becomes of the versions of a path), `rules` (the names a path gets) |
//! | `ClusterApplier` | `cluster` |
//! | `ToolWrites` | `tool_writes` (the seam with prompt 06) |
//! | the fuzz entry points | `fuzzing` |

pub(crate) mod apply;
pub mod cluster;
mod delta;
#[doc(hidden)]
pub mod fuzzing;
mod heartbeat;
mod hello;
pub(crate) mod plan;
pub(crate) mod rules;
mod sink;
pub(crate) mod staging;
pub mod tool_writes;
pub(crate) mod validate;

pub(crate) use sink::Inner;
pub use sink::{AgentIngest, IngestDeps};
pub use tool_writes::{ToolWriteRecord, ToolWrites};
