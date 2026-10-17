//! Attribution (T7, D72, D73, D74, P14): who changed a file, from what a sentinel on the NFS VM saw.
//!
//! The registry decides an observation's first attribution when the agent reports it ([`crate::ingest`]): a registered tool write is
//! `tool_write` (certain), a change in a sync window or tagged with a Job is `sync_job` (high), everything else is `unknown`. This module
//! is what makes `unknown` better, in the order of the domain model:
//!
//! | Source | When | Module |
//! |---|---|---|
//! | `sentinel_login` (high) | a record of the sentinel on the same path within ±60 s of the observation, naming the OS Login user (and the application user it maps to) | [`correlate`], [`os_login`] |
//! | `nfs_client` (medium) | nothing explains it after a 30 s grace, and every sentinel of the swimlane has been seen since the change | [`correlate`] |
//!
//! | Piece | Module | What it does |
//! |---|---|---|
//! | `SentinelIngest: SentinelSink` | `sentinel_sink` | `hello` against the admin-registered sentinel; batches deduplicated by `seq`; the records mapped to the swimlane and stored; the new records correlated in the same transaction |
//! | the path mapping (A17) | [`map`] | a sentinel's paths are relative to its export root, a swimlane's to its config root; what is outside is dropped and counted |
//! | `Correlator` | [`correlate`] | which observation a record explains, which is an NFS client's, and the one writer that upgrades an attribution (one audit event, one `AttributionUpdated`) |
//! | `OsLoginMap` | [`os_login`] | the admin's map, then an unambiguous match on email, else the OS Login user alone |
//! | `registry-attribution-grace` | `grace` | the leased job: observations applied after their record, and the ones nothing explains |
//! | `registry-sentinel-partitions` | `partitions` | the leased job that calls `maintain_sentinel_partitions` |
//!
//! # What it holds to (rule 5, rule 10, S5, S10, S25)
//!
//! - **The sentinel's identity** is the certificate's name; `hello` is accepted only for a sentinel an admin registered, with the root the
//!   admin registered, and a record is stored only under the swimlane that sentinel serves.
//! - **Paths are untrusted**: validated as `NfsPath`s at the edge, mapped by component, bounded; a record outside the swimlane's prefix is
//!   dropped and counted, never stored.
//! - **Everything is bounded**: records and bytes in a batch, the sentinels that have said `hello`, the observations a run reads.
//! - **Nothing is lost or doubled**: `last_acked_seq` moves in the transaction that stores the batch, a resend is acknowledged and
//!   stores nothing (and the primary key refuses a duplicate if the counter was ever reset).
//! - **No path, user name, login or content** is in a log line, an error or a metric label (S10): the logs and metrics carry counts and
//!   closed words.
//! - **Attribution only goes up** (D73), and a tool write is never touched.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use domain::ShortText;

use crate::clock::Clock;
use crate::config::RegistryConfig;
use crate::db::Db;
use crate::metrics::RegistryMetrics;
use ports::{AuditLog, EventBus};
use std::sync::Arc;

pub(crate) mod correlate;
mod grace;
pub mod map;
mod os_login;
mod partitions;
mod sentinel_sink;

pub use correlate::WINDOW_MS;
pub use grace::{AttributionGrace, GRACE_JOB, GraceReport};
pub use map::Mapper;
pub use partitions::{PARTITIONS_JOB, PartitionReport, SentinelPartitions};
pub use sentinel_sink::{AttributionDeps, SentinelIngest};

/// SQL of the sink, the correlator and the job, for the `EXPLAIN` snapshots of `tests/explain.rs` (P12).
pub mod statements {
    pub use super::correlate::{FROM_BATCH_SQL, FROM_OBSERVATIONS_SQL, NFS_CLIENT_SQL, UPGRADE_SQL};
    pub use super::grace::LANES_SQL;
    pub use super::os_login::{ADMIN_MAP_SQL, EMAIL_SQL};
    pub use super::sentinel_sink::{ACK_SQL, HELLO_SQL, INSERT_RECORDS_SQL, SENTINEL_SQL};
}

/// What the sink, the correlator and the jobs share.
pub(crate) struct Inner {
    pub db: Db,
    pub bus: Arc<dyn EventBus>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    /// The sentinels that said `hello` on this replica, by the name in their certificate. Bounded by `sentinel_max_streams`.
    streams: Mutex<HashSet<ShortText>>,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    pub(crate) fn new(
        db: Db,
        bus: Arc<dyn EventBus>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        config: RegistryConfig,
        metrics: RegistryMetrics,
    ) -> Self {
        Self {
            db,
            bus,
            audit,
            clock,
            config,
            metrics,
            streams: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn said_hello(&self, name: &ShortText) -> bool {
        locked(&self.streams).contains(name)
    }

    /// Make room for a sentinel that is saying `hello`. `Ok(true)` when it was not in yet (the caller gives the place back with
    /// [`Inner::forget`] if the `hello` then fails), `Ok(false)` when it already was (a reconnecting sentinel is never refused for want
    /// of room). A new one over the bound is refused (rule 5).
    pub(crate) fn admit(&self, name: &ShortText) -> Result<bool, crate::error::RegistryError> {
        let mut streams = locked(&self.streams);
        if streams.contains(name) {
            return Ok(false);
        }
        if streams.len() >= self.config.sentinel_max_streams {
            return Err(crate::error::RegistryError::Capacity);
        }
        streams.insert(name.clone());
        Ok(true)
    }

    /// Take back a place that [`Inner::admit`] gave.
    pub(crate) fn forget(&self, name: &ShortText) {
        locked(&self.streams).remove(name);
    }
}
