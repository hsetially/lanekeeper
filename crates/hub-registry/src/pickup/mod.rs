//! Pickup state (T10, D85, A2, A11, A20, A37): when a change to a file takes effect for each service that reads it.
//!
//! A change is a file whose newest version was detected after the first inventory of its swimlane. For each service that reads it
//! ([`consumers`]) the engine says what has to happen before the service has it ([`engine::pickup_state`]: wait for the client cache,
//! notify or restart, restart the config-server), and [`state`] folds a service's changes into the one state the contract has for it.
//! The result is `service_state` (`pickup_state`, `pending_since`, `live_by`, `pending_files`), which `GET pending-restarts` reads (T6,
//! `read::restarts`): reads do not compute anything.
//!
//! # What the state is made of
//!
//! | Input | From |
//! |---|---|
//! | the changes | `file_state.observed_at` of the files whose newest observation is not part of the first inventory (`file_observations.initial`, A11: auditing 2,000 files and calling every service stale would be false alarms), newest first, at most `pickup_max_files` |
//! | the role of each file | `engine::file_role` against the swimlane's tenants ([`crate::tenants`]) and the application names of its Deployments |
//! | which services read it | [`consumers`] |
//! | a Deployment's client | the two allowlisted variables `CONFIG_CLIENT_CACHE_TTL` and `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED` of `services.env_values` (D88), and `SPRING_APPLICATION_NAME` to find its application. Read one key at a time in SQL (`->>`), so no other value of the object is ever loaded; none of the three is logged, counted or published |
//! | when its pods started | `service_state.oldest_pod_started_at`: every pod started after a change is the oldest one having started after it |
//! | when it was told | `swimlanes.last_notified_at`, written by prompt 06's notify action and **only read** here (A20) |
//! | the config-server | `swimlanes.config_server_started_at` (D88), and `swimlane_folders` for a folder first seen after it |
//!
//! # Who computes it
//!
//! The same function, from the same rows, in four places, so that whichever runs last says what the others would:
//!
//! 1. **a cluster report** ([`crate::ingest::cluster`]), in the transaction that stores the pods, the environment and the config-server's
//!    start, so a restart or a changed cache setting is seen with the facts it comes from;
//! 2. **the pipeline job**, after each batch that brought swimlanes up to date (the files changed), outside the batch's own transaction: the
//!    benchmarked pipeline is not slowed by it and a failure here never fails a batch;
//! 3. **the sweeper** ([`sweeper`], the leased job `registry-pickup-sweeper`, every `pickup_sweep_interval`, A37): the swimlanes that still
//!    have something pending (time passes, a notification was written by another component), and every swimlane with an inventory every
//!    `pickup_full_interval` (what no event announced: an admin's folder mapping, a recompute that failed, a replica that took over);
//! 4. [`Pickups::recompute`] for a caller that knows better.
//!
//! Every computation of a swimlane takes the swimlane's advisory lock first and reads after it, so two of them never write out of order, and
//! only the services whose state **differs** are written (`IS DISTINCT FROM` in the statement), which is what makes a repeat free and a
//! second sweep silent.
//!
//! # `PickupChanged`, and what is audited
//!
//! The services whose stored state changed are named in `PickupChanged { swimlane, services }` (A2: the old `PendingRestartChanged` is
//! never published again), through the outbox, in the transaction that wrote them (D80). It is an invalidation hint and carries ids only:
//! the state, "live by HH:MM" included, is read from `GET pending-restarts`.
//!
//! **No audit event.** Rule 8 and A12 audit decisions and out-of-band changes; the pickup state is derived from facts that are audited where
//! they happen (the file change, the notification by prompt 06), like the other derived columns (D93).
//!
//! # Limits
//!
//! - A **resource file's readers are the application of its first folder and the folders an admin linked**: any other service may ask for it
//!   by name (every non-channel folder is a search location) and is not shown as pending for it ([`consumers`], D105).
//! - **Several tenants in one swimlane (Q34)**: a service is stale for the changes to every tenant's copy, not only its own tenant's. The
//!   profile a service runs is not read.
//! - **A config-server start that is not known** (the agent has not reported it) switches off the two kinds that need its restart: a file in
//!   a new folder or `channels.yml` then counts as an ordinary resource or property source and heals with the cache, a notification or a
//!   restart, instead of staying pending for ever.
//! - **A file in a new folder** is a `needs_config_server_restart` change for as long as the folder was first seen after the config-server
//!   started. Once the config-server has started after it the folder is a search location, and the file is the change to a resource or a
//!   property source that it is: a resource then goes live with the client's cache, not at once, because a client may have asked for it
//!   before.
//! - **The newest `pickup_max_files` changes** are what a swimlane is computed from; `registry_pickup_truncated` counts the times it was more.
//! - A notification written by prompt 06 is seen at the next sweep, at most `pickup_sweep_interval` later.

pub(crate) mod consumers;
pub mod state;
pub mod sweeper;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use domain::{
    AppName, DomainEvent, FileClass, FileRole, LogicalFile, NfsPath, PickupState, ServiceRef, SwimlaneId,
};
use engine::{ClientSettings, DenyMatcher, file_role};
use ports::{EventBus, Tx};
use sqlx::{PgPool, Row};
use tracing::debug;

use self::consumers::{Directory, Mapping, Reader};
use self::state::{Assessment, Change, Context, assess, kind_of};
pub use self::state::{MAX_PENDING_FILES, client_settings};
pub use self::sweeper::{JOB_NAME, SweepReport};
use crate::batch::publish_all;
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::{Db, Txn, ms, ts};
use crate::denied::is_denied;
use crate::error::RegistryError;
use crate::metrics::{PickupResult, RegistryMetrics};
use crate::tenants;

/// Services named in one `PickupChanged` event; more are sent in further events.
pub(crate) const EVENT_SERVICES: usize = 100;
/// The most folder mappings and new folders read for one swimlane: both are small, the bounds are guards (rule 5).
const MAX_MAPPINGS: i64 = 10_000;
const MAX_NEW_FOLDERS: i64 = 50_000;

/// The statement that serialises the computations of one swimlane. A key of its own: it is never held with the apply lock of ingestion.
const LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('lanekeeper.registry.pickup.' || $1, 0))";

/// What the pickup computation needs from the rest of the hub.
pub struct PickupDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub bus: Arc<dyn EventBus>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for PickupDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickupDeps").finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub db: Db,
    pub bus: Arc<dyn EventBus>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
    pub deny: DenyMatcher,
    /// When the sweeper last looked at every swimlane (Unix milliseconds); the lowest value before the first look.
    pub last_full: AtomicI64,
}

/// What a computation of several swimlanes did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Swimlanes that were computed.
    pub swimlanes: usize,
    /// Services whose state changed.
    pub changed: usize,
    /// Swimlanes that could not be computed (the database, the outbox). Nothing of them was written.
    pub failed: Vec<SwimlaneId>,
}

/// The pickup state of the swimlanes. Cloning shares it.
#[derive(Clone)]
pub struct Pickups {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Pickups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pickups").finish_non_exhaustive()
    }
}

impl Pickups {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range or a deny glob does not compile.
    pub fn new(deps: PickupDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let deny = DenyMatcher::with_globs(deps.config.extra_deny_globs.iter().map(String::as_str))
            .map_err(|_| ConfigError::Invalid("extra_deny_globs"))?;
        Ok(Self {
            inner: Arc::new(Inner {
                db: Db::new(deps.pool, deps.config.query_timeout),
                bus: deps.bus,
                clock: deps.clock,
                config: deps.config,
                metrics: deps.metrics,
                deny,
                last_full: AtomicI64::new(i64::MIN),
            }),
        })
    }

    /// Compute the pickup state of these swimlanes, one transaction each: the lock, the computation, the services whose state changed and
    /// their `PickupChanged`, the commit. A swimlane that fails is in the report and the others go on; the sweeper looks at it again.
    pub async fn recompute(&self, swimlanes: &[SwimlaneId]) -> Report {
        let mut report = Report::default();
        for swimlane in swimlanes {
            if let Ok(changed) = self.inner.recompute_one(swimlane).await {
                report.swimlanes += 1;
                report.changed += changed;
            } else {
                self.inner.metrics.pickup_lane(PickupResult::Failed);
                report.failed.push(swimlane.clone());
            }
        }
        report
    }
}

impl Inner {
    pub(crate) fn compute(&self) -> Compute<'_> {
        Compute {
            db: &self.db,
            clock: &*self.clock,
            config: &self.config,
            deny: &self.deny,
            metrics: &self.metrics,
        }
    }

    /// One swimlane in a transaction of its own. Returns the number of services whose state changed.
    async fn recompute_one(&self, swimlane: &SwimlaneId) -> Result<usize, RegistryError> {
        let mut tx = self.db.begin().await?;
        lock(&self.db, &mut tx, swimlane).await?;
        let outcome = self.compute().lane(&mut tx, swimlane).await?;
        if !outcome.changed.is_empty() {
            let mut handle = Tx::from_pg(&mut tx);
            publish_all(&*self.bus, &mut handle, events(swimlane, &outcome.changed)).await?;
        }
        self.db.commit(tx).await?;
        Ok(outcome.changed.len())
    }
}

/// Take the swimlane's pickup lock for the rest of the transaction. Taken **before** the first row of the swimlane is written, so that a
/// transaction that writes `service_state` and one that is about to never wait for each other in a circle.
pub(crate) async fn lock(db: &Db, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<(), RegistryError> {
    db.run(
        "pickup_lock",
        sqlx::query(LOCK_SQL).bind(swimlane.as_str()).execute(&mut **tx),
    )
    .await?;
    Ok(())
}

/// `PickupChanged` for the services whose state changed, in chunks of [`EVENT_SERVICES`].
pub(crate) fn events(swimlane: &SwimlaneId, changed: &[ServiceRef]) -> Vec<DomainEvent> {
    changed
        .chunks(EVENT_SERVICES)
        .map(|services| DomainEvent::PickupChanged {
            swimlane: swimlane.clone(),
            services: services.to_vec(),
        })
        .collect()
}

/// The statements of the computation, as the code runs them and as the plan snapshots of `tests/explain.rs` read them (P12: every one is
/// bounded, and none reads a large table with a sequential scan).
///
/// The swimlane's own times. `$1` the swimlane.
pub fn times_sql() -> String {
    format!(
        "SELECT {started} AS started, {notified} AS notified FROM swimlanes WHERE id = $1",
        started = ms("config_server_started_at"),
        notified = ms("last_notified_at"),
    )
}

/// The swimlane's Deployments: those that exist, and those that are gone but still have a state to clear. Each environment value is read by
/// its key, so nothing but the three keys of D88 is ever loaded. `$1` the swimlane, `$2` the limit.
pub fn services_sql() -> String {
    format!(
        "SELECT s.id, s.namespace, s.name, s.removed_at IS NOT NULL AS removed, \
                s.env_values ->> 'CONFIG_CLIENT_CACHE_TTL' AS ttl, \
                s.env_values ->> 'CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED' AS notifications, \
                s.env_values ->> 'SPRING_APPLICATION_NAME' AS app, \
                {oldest} AS oldest, COALESCE(st.pickup_state, 'live') AS state \
         FROM services s LEFT JOIN service_state st ON st.service_id = s.id \
         WHERE s.swimlane_id = $1 AND (s.removed_at IS NULL OR COALESCE(st.pickup_state, 'live') <> 'live') \
         ORDER BY s.id LIMIT $2",
        oldest = ms("st.oldest_pod_started_at"),
    )
}

/// The files whose newest version is newer than a bound and is not part of the first inventory, newest first. `$1` the swimlane, `$2` the
/// bound (Unix milliseconds), `$3` the limit: one row more than `pickup_max_files`, so that a swimlane over the bound is seen as such.
pub fn changed_files_sql() -> String {
    format!(
        "SELECT fs.path, fs.logical_file, fs.class, fs.denied, {observed} AS at \
         FROM file_state fs LEFT JOIN file_observations o ON o.id = fs.last_observation_id \
         WHERE fs.swimlane_id = $1 AND fs.observed_at > {bound} AND NOT COALESCE(o.initial, false) \
         ORDER BY fs.observed_at DESC, fs.path LIMIT $3",
        observed = ms("fs.observed_at"),
        bound = ts("$2"),
    )
}

/// The folders first seen after the config-server started, and when. `$1` the swimlane, `$2` the start, `$3` the limit.
pub fn new_folders_sql() -> String {
    format!(
        "SELECT dir_path, {seen} AS seen FROM swimlane_folders \
         WHERE swimlane_id = $1 AND NOT initial AND first_seen_at > {started} ORDER BY dir_path LIMIT $3",
        seen = ms("first_seen_at"),
        started = ts("$2"),
    )
}

/// The folder mappings a person confirmed: the swimlane's own and the global ones. `$1` the swimlane, `$2` the limit.
pub const MAPPINGS_SQL: &str = "SELECT swimlane_id IS NOT NULL AS own, folder_path, namespace, deployment FROM folder_mappings \
     WHERE confirmed AND (swimlane_id = $1 OR swimlane_id IS NULL) ORDER BY id LIMIT $2";

/// Write the services whose row differs from what was computed (or has none) and return their ids: the comparison is the statement's, so a
/// repeat writes nothing and returns nothing. `$1` ids, `$2` states, `$3` `live_by`, `$4` `pending_since`, `$5` the files as JSON texts, `$6`
/// now.
pub fn write_sql() -> String {
    format!(
        "INSERT INTO service_state AS st (service_id, pickup_state, live_by, pending_since, pending_files, updated_at) \
         SELECT x.id, x.state, {live_by}, {pending}, x.files::jsonb, {now} \
         FROM unnest($1::bigint[], $2::text[], $3::bigint[], $4::bigint[], $5::text[]) AS x(id, state, live_by, pending, files) \
         ON CONFLICT (service_id) DO UPDATE SET \
           pickup_state = EXCLUDED.pickup_state, live_by = EXCLUDED.live_by, pending_since = EXCLUDED.pending_since, \
           pending_files = EXCLUDED.pending_files, updated_at = EXCLUDED.updated_at \
         WHERE (st.pickup_state, st.live_by, st.pending_since, st.pending_files) \
               IS DISTINCT FROM (EXCLUDED.pickup_state, EXCLUDED.live_by, EXCLUDED.pending_since, EXCLUDED.pending_files) \
         RETURNING st.service_id",
        live_by = ts("x.live_by"),
        pending = ts("x.pending"),
        now = ts("$6"),
    )
}

/// What a computation of one swimlane found.
#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    /// The services whose stored state it changed, in service order.
    pub changed: Vec<ServiceRef>,
}

/// The computation of one swimlane's pickup state, over the caller's transaction. Borrows what it needs, so that ingestion (whose
/// transaction it joins) and the pickup job build it from their own state.
pub(crate) struct Compute<'a> {
    pub db: &'a Db,
    pub clock: &'a dyn Clock,
    pub config: &'a RegistryConfig,
    pub deny: &'a DenyMatcher,
    pub metrics: &'a RegistryMetrics,
}

/// A Deployment of the swimlane, as the computation needs it.
struct Svc {
    id: i64,
    service: ServiceRef,
    app: Option<AppName>,
    client: ClientSettings,
    /// When the oldest running pod started (Unix milliseconds); `None`: no pod runs.
    oldest_pod: Option<i64>,
    removed: bool,
    /// What `service_state` holds now (`live` when there is no row).
    stored: PickupState,
}

impl Svc {
    /// Whether the service can be stale at all: it exists and runs a pod.
    fn runs(&self) -> bool {
        !self.removed && self.oldest_pod.is_some()
    }
}

/// A changed file, as read.
struct FileRow {
    path: String,
    logical: String,
    class: FileClass,
    flagged: bool,
    observed_at: i64,
}

impl Compute<'_> {
    /// Compute the pickup state of `swimlane` and write the services whose state differs. The caller holds the swimlane's lock
    /// ([`lock`]), publishes [`events`] for `changed` and commits.
    #[allow(clippy::too_many_lines)] // the steps are numbered and share their locals
    pub(crate) async fn lane(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<Outcome, RegistryError> {
        let now = self.clock.now().unix_millis();
        // 1. The swimlane's own times. A swimlane that is gone has nothing.
        let Some((config_server, notified)) = self.times(tx, swimlane).await? else {
            return Ok(Outcome::default());
        };
        // 2. Its Deployments, and 3. what the changes since the oldest pod started say about each of them.
        let services = self.services(tx, swimlane).await?;
        let mut desired: Vec<Assessment> = vec![Assessment::live(); services.len()];
        let running: Vec<usize> = (0..services.len()).filter(|&i| services[i].runs()).collect();
        if !running.is_empty() {
            self.assess_running(
                tx,
                swimlane,
                &services,
                &running,
                config_server,
                notified,
                now,
                &mut desired,
            )
            .await?;
        }
        // 4. Write what differs. A service that is live and was live has nothing to write.
        let to_write: Vec<usize> = (0..services.len())
            .filter(|&i| !desired[i].is_live() || services[i].stored != PickupState::Live)
            .collect();
        let written = self.write(tx, &services, &desired, &to_write, now).await?;
        let changed: Vec<ServiceRef> = services
            .iter()
            .filter(|s| written.contains(&s.id))
            .map(|s| s.service.clone())
            .collect();
        self.metrics.pickup_lane(if changed.is_empty() {
            PickupResult::Unchanged
        } else {
            PickupResult::Changed
        });
        self.metrics.pickup_services_changed(changed.len());
        // Ids and counts only (S10): no path, no value.
        debug!(
            swimlane = %swimlane,
            services = services.len(),
            running = running.len(),
            changed = changed.len(),
            "pickup computed"
        );
        Ok(Outcome { changed })
    }

    /// `config_server_started_at` and `last_notified_at`, or `None` for a swimlane that does not exist.
    async fn times(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
    ) -> Result<Option<(Option<i64>, Option<i64>)>, RegistryError> {
        let row = self
            .db
            .run(
                "pickup_swimlane",
                sqlx::query(&times_sql())
                    .bind(swimlane.as_str())
                    .fetch_optional(&mut **tx),
            )
            .await?;
        Ok(row.map(|r| (r.get("started"), r.get("notified"))))
    }

    /// The swimlane's Deployments: those that exist, and those that are gone but still have a state to clear. Each environment value is
    /// read by its key, so nothing but the three keys of D88 is ever loaded.
    async fn services(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<Vec<Svc>, RegistryError> {
        let rows = self
            .db
            .run(
                "pickup_services",
                sqlx::query(&services_sql())
                    .bind(swimlane.as_str())
                    .bind(i64::try_from(self.config.max_services_per_report).unwrap_or(i64::MAX))
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let (namespace, name): (String, String) = (row.get("namespace"), row.get("name"));
            // A row that is not a Deployment's name cannot be named in an event: nothing wrote one (the agent's names are checked).
            let Ok(service) = ServiceRef::new(&namespace, &name) else {
                continue;
            };
            let ttl: Option<String> = row.get("ttl");
            let notifications: Option<String> = row.get("notifications");
            let app: Option<String> = row.get("app");
            let state: String = row.get("state");
            out.push(Svc {
                id: row.get("id"),
                service,
                app: app.and_then(|a| AppName::parse(&a).ok()),
                client: client_settings(notifications.as_deref(), ttl.as_deref()),
                oldest_pod: row.get("oldest"),
                removed: row.get("removed"),
                stored: state.parse().unwrap_or(PickupState::Live),
            });
        }
        Ok(out)
    }

    /// Fill `desired` for the services that run a pod: read the changes newer than anything that could have made them live, find who reads
    /// each, and fold.
    #[allow(clippy::too_many_arguments)] // the inputs of one computation, read once
    async fn assess_running(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        services: &[Svc],
        running: &[usize],
        config_server: Option<i64>,
        notified: Option<i64>,
        now: i64,
        desired: &mut [Assessment],
    ) -> Result<(), RegistryError> {
        // A change at or before this is live for every service whatever kind it is: the pods started after it, or the services were told
        // after it, or the config-server started after it. Only the newer ones are read.
        let mut bound = i64::MAX;
        for &i in running {
            let s = &services[i];
            let mut covered = s.oldest_pod.unwrap_or(i64::MAX);
            if s.client.notifications_enabled {
                covered = covered.max(notified.unwrap_or(i64::MIN));
            }
            bound = bound.min(covered);
        }
        if let Some(started) = config_server {
            bound = bound.min(started);
        }
        let mut rows = self.changed_files(tx, swimlane, bound).await?;
        if rows.len() > self.config.pickup_max_files {
            rows.truncate(self.config.pickup_max_files);
            self.metrics.pickup_truncated();
        }
        if rows.is_empty() {
            return Ok(());
        }
        let tenants = tenants::stored(self.db, tx, swimlane).await?;
        let new_folders = match config_server {
            Some(started) => self.new_folders(tx, swimlane, started).await?,
            None => BTreeMap::new(),
        };
        let mappings = self.mappings(tx, swimlane).await?;

        let apps: Vec<AppName> = running
            .iter()
            .filter_map(|&i| services[i].app.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let readers: Vec<Reader<'_>> = running
            .iter()
            .map(|&i| Reader {
                namespace: services[i].service.namespace(),
                name: services[i].service.name(),
                app: services[i].app.as_ref(),
            })
            .collect();
        let directory = Directory::new(&readers, &mappings);

        let mut changes: Vec<Change> = Vec::new();
        // For each running service, the changes it reads.
        let mut reads: Vec<Vec<usize>> = vec![Vec::new(); running.len()];
        for row in &rows {
            let (Ok(path), Ok(logical)) = (NfsPath::parse(&row.path), LogicalFile::parse(&row.logical))
            else {
                continue;
            };
            // A copy for a tenant the swimlane does not deploy is read by nobody: it changes nothing for anybody.
            if !consumers::is_read(&path, &logical, &tenants) {
                continue;
            }
            let role = file_role(&path, &tenants, &apps);
            let who = directory.readers(&path, role, &tenants);
            if who.is_empty() {
                continue;
            }
            let (kind, changed_at) = kind_of(
                role == FileRole::PropertySource,
                logical.as_path().file_name() == "channels.yml"
                    && tenants::tenant_suffix(&path, &logical).is_none(),
                newest_new_folder(&path, &new_folders),
                row.observed_at,
                config_server.is_some(),
            );
            let at = changes.len();
            changes.push(Change {
                path: path.as_str().to_owned(),
                kind,
                changed_at,
                listed: !is_denied(self.deny, &path, row.flagged, row.class),
            });
            for &reader in who.iter() {
                reads[reader].push(at);
            }
        }
        let ctx = Context {
            now,
            notified_at: notified,
            config_server_started: config_server.unwrap_or(0),
        };
        for (slot, &i) in running.iter().enumerate() {
            let s = &services[i];
            desired[i] = assess(
                s.client,
                s.oldest_pod,
                reads[slot].iter().map(|&c| &changes[c]),
                &ctx,
                MAX_PENDING_FILES,
            );
        }
        Ok(())
    }

    /// The files whose newest version is newer than `bound` and is not part of the first inventory, newest first. One row more than
    /// `pickup_max_files`, so that a swimlane over the bound is seen as such.
    async fn changed_files(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        bound: i64,
    ) -> Result<Vec<FileRow>, RegistryError> {
        let rows = self
            .db
            .run(
                "pickup_changed_files",
                sqlx::query(&changed_files_sql())
                    .bind(swimlane.as_str())
                    .bind(bound)
                    .bind(i64::try_from(self.config.pickup_max_files.saturating_add(1)).unwrap_or(i64::MAX))
                    .fetch_all(&mut **tx),
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let class: String = row.get("class");
            out.push(FileRow {
                path: row.get("path"),
                logical: row.get("logical_file"),
                class: class.parse().map_err(|_| RegistryError::Corrupt)?,
                flagged: row.get("denied"),
                observed_at: row.get("at"),
            });
        }
        Ok(out)
    }

    /// The folders first seen after the config-server started, and when (C11, A11): the folders of the first inventory are never new.
    async fn new_folders(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        started: i64,
    ) -> Result<BTreeMap<String, i64>, RegistryError> {
        let rows = self
            .db
            .run(
                "pickup_new_folders",
                sqlx::query(&new_folders_sql())
                    .bind(swimlane.as_str())
                    .bind(started)
                    .bind(MAX_NEW_FOLDERS)
                    .fetch_all(&mut **tx),
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get("dir_path"), r.get("seen"))).collect())
    }

    /// The folder mappings a person confirmed: the swimlane's own and the global ones.
    async fn mappings(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<Vec<Mapping>, RegistryError> {
        let rows = self
            .db
            .run(
                "pickup_mappings",
                sqlx::query(MAPPINGS_SQL)
                    .bind(swimlane.as_str())
                    .bind(MAX_MAPPINGS)
                    .fetch_all(&mut **tx),
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| Mapping {
                of_swimlane: r.get("own"),
                folder: r.get("folder_path"),
                namespace: r.get("namespace"),
                deployment: r.get("deployment"),
            })
            .collect())
    }

    /// Write the services of `to_write` whose row differs from `desired` (or has none), and say which. The comparison is the statement's, so
    /// a repeat writes nothing and returns nothing.
    async fn write(
        &self,
        tx: &mut Txn,
        services: &[Svc],
        desired: &[Assessment],
        to_write: &[usize],
        now: i64,
    ) -> Result<HashSet<i64>, RegistryError> {
        let mut written = HashSet::new();
        let sql = write_sql();
        for chunk in to_write.chunks(self.config.rows_per_statement) {
            let ids: Vec<i64> = chunk.iter().map(|&i| services[i].id).collect();
            let states: Vec<&str> = chunk.iter().map(|&i| desired[i].state.as_str()).collect();
            let live_by: Vec<Option<i64>> = chunk.iter().map(|&i| desired[i].live_by).collect();
            let pending: Vec<Option<i64>> = chunk.iter().map(|&i| desired[i].pending_since).collect();
            let files: Vec<String> = chunk
                .iter()
                .map(|&i| serde_json::Value::from(desired[i].files.clone()).to_string())
                .collect();
            let done: Vec<i64> = self
                .db
                .run(
                    "pickup_write",
                    sqlx::query_scalar(&sql)
                        .bind(&ids)
                        .bind(&states)
                        .bind(&live_by)
                        .bind(&pending)
                        .bind(&files)
                        .bind(now)
                        .fetch_all(&mut **tx),
                )
                .await?;
            written.extend(done);
        }
        Ok(written)
    }
}

/// The newest first-seen time among the new folders above `path`: when the config-server was last made to miss one of its folders.
fn newest_new_folder(path: &NfsPath, new_folders: &BTreeMap<String, i64>) -> Option<i64> {
    if new_folders.is_empty() {
        return None;
    }
    let mut newest: Option<i64> = None;
    let mut dir = path.parent();
    while !dir.is_root() {
        if let Some(&seen) = new_folders.get(dir.as_str()) {
            newest = Some(newest.map_or(seen, |n| n.max(seen)));
        }
        dir = dir.parent();
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> NfsPath {
        NfsPath::parse(s).unwrap()
    }

    #[test]
    fn the_newest_new_folder_above_a_file_decides_when_it_was_missed() {
        let folders: BTreeMap<String, i64> =
            [("a".to_owned(), 10), ("a/b".to_owned(), 30), ("c".to_owned(), 99)].into();
        assert_eq!(newest_new_folder(&path("a/b/x.yml"), &folders), Some(30));
        assert_eq!(newest_new_folder(&path("a/x.yml"), &folders), Some(10));
        assert_eq!(
            newest_new_folder(&path("a/old/x.yml"), &folders),
            Some(10),
            "a folder below a new one is behind it"
        );
        assert_eq!(newest_new_folder(&path("d/x.yml"), &folders), None);
        assert_eq!(
            newest_new_folder(&path("x.yml"), &folders),
            None,
            "the root is a search location from the start"
        );
        assert_eq!(newest_new_folder(&path("a/x.yml"), &BTreeMap::new()), None);
    }

    #[test]
    fn the_events_name_the_services_in_chunks() {
        let lane = SwimlaneId::parse("sit1").unwrap();
        let many: Vec<ServiceRef> = (0..=EVENT_SERVICES)
            .map(|i| ServiceRef::new("csp", &format!("svc-{i}")).unwrap())
            .collect();
        let sent = events(&lane, &many);
        assert_eq!(sent.len(), 2);
        assert!(events(&lane, &[]).is_empty());
        assert!(matches!(
            &sent[0],
            DomainEvent::PickupChanged { swimlane, services } if *swimlane == lane && services.len() == EVENT_SERVICES
        ));
    }
}
