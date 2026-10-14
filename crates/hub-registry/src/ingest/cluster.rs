//! `cluster`: the agent's report of its Kubernetes cluster (D85, D86, D88).
//!
//! What a report carries and what the hub keeps of it:
//!
//! | Report | Stored |
//! |---|---|
//! | Deployments | `services` (the Deployment, its allowlisted environment values and its other variable *names*) and `service_state.pods` |
//! | release hints | `release_hints`: hints for the adoption pass (Q19), never an authority |
//! | config-server start | `swimlanes.config_server_started_at` (C11) |
//! | sync windows | `sync_windows`, opened and closed once per Job (D75), each with its outbox event |
//!
//! **Environment values (D88, S10).** A value is kept only for a variable on the allowlist, whatever the agent sends: any other
//! value is dropped and counted. A name must look like an environment variable's (`[A-Za-z_][A-Za-z0-9_]*`, 128 bytes at most);
//! anything else is dropped, so a secret cannot ride in as a "name".
//!
//! A change in the Deployments' pods (a Deployment appeared, vanished or restarted) publishes `PendingRestartChanged`, which
//! prompt 05's T10 replaces by `PickupChanged`: until then it is the hint that a client should read the pending restarts again.

use std::collections::{BTreeMap, BTreeSet};

use domain::{ClusterReport, DomainEvent, JobRef, ServiceRef, SwimlaneId, SyncWindowKind};
use ports::{AgentIdentity, Tx};

use super::Inner;
use crate::db::{Txn, ts};
use crate::error::RegistryError;

/// The longest environment variable name kept.
const MAX_ENV_NAME_BYTES: usize = 128;
/// Services named in one `PendingRestartChanged` event; more are sent in further events.
const EVENT_SERVICES: usize = 100;

/// Whether `s` is shaped like an environment variable name.
pub fn is_env_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    s.len() <= MAX_ENV_NAME_BYTES
        && bytes.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// A Deployment as the report describes it, cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Service {
    key: (String, String),
    env_values: BTreeMap<String, String>,
    env_names: Vec<String>,
    /// `(name, started_at_ms)`, by name.
    pods: Vec<(String, i64)>,
    removed: bool,
}

/// What the database has of a Deployment.
#[derive(Debug, Clone)]
struct HeldService {
    removed: bool,
    pods: Vec<(String, i64)>,
    env_values: BTreeMap<String, String>,
    env_names: Vec<String>,
}

/// The Deployments of a swimlane that the hub holds, by namespace and name, with their ids.
type Held = BTreeMap<(String, String), (i64, HeldService)>;
/// A row of `services` joined with its pods, as `held_services` reads it.
type HeldRow = (
    i64,
    String,
    String,
    bool,
    serde_json::Value,
    serde_json::Value,
    Vec<String>,
);

fn pods_json(pods: &[(String, i64)]) -> serde_json::Value {
    serde_json::Value::Array(
        pods.iter()
            .map(|(name, at)| serde_json::json!({ "name": name, "started_at_ms": at }))
            .collect(),
    )
}

fn pods_from_json(v: &serde_json::Value) -> Vec<(String, i64)> {
    let mut pods: Vec<(String, i64)> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((
                        p.get("name")?.as_str()?.to_owned(),
                        p.get("started_at_ms")?.as_i64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    pods.sort();
    pods
}

/// Applies a cluster report. Borrows the ingestion state; one value per message.
pub(crate) struct ClusterApplier<'a> {
    inner: &'a Inner,
}

impl<'a> ClusterApplier<'a> {
    pub(crate) fn new(inner: &'a Inner) -> Self {
        Self { inner }
    }

    pub(crate) async fn apply(&self, id: &AgentIdentity, c: ClusterReport) -> Result<(), RegistryError> {
        let swimlane = id.swimlane();
        if !self.inner.said_hello(swimlane) {
            return Err(RegistryError::HelloRequired);
        }
        let cfg = &self.inner.config;
        if c.deployments.len() > cfg.max_services_per_report
            || c.release_hints.len() > cfg.max_release_hints_per_report
            || c.sync_windows.len() > cfg.max_sync_windows_per_report
            || c.deployments
                .iter()
                .any(|d| d.pods.len() > cfg.max_pods_per_service)
        {
            return Err(RegistryError::TooLarge);
        }

        let mut dropped_env = 0_usize;
        let mut wanted: BTreeMap<(String, String), Service> = BTreeMap::new();
        for d in &c.deployments {
            let key = (d.service.namespace().to_owned(), d.service.name().to_owned());
            let mut env_values = BTreeMap::new();
            for v in &d.env_values {
                if cfg.env_allowlist.iter().any(|n| n == v.name.as_str()) {
                    env_values.insert(v.name.as_str().to_owned(), v.value.as_str().to_owned());
                } else {
                    dropped_env += 1;
                }
            }
            let mut names: BTreeSet<String> = BTreeSet::new();
            for n in &d.env_names {
                if is_env_name(n.as_str()) {
                    names.insert(n.as_str().to_owned());
                } else {
                    dropped_env += 1;
                }
            }
            let mut pods: Vec<(String, i64)> = d
                .pods
                .iter()
                .map(|p| (p.name.as_str().to_owned(), p.started_at.unix_millis()))
                .collect();
            pods.sort();
            wanted.insert(
                key.clone(),
                Service {
                    key,
                    env_values,
                    env_names: names.into_iter().take(cfg.max_env_names_per_service).collect(),
                    pods,
                    removed: d.removed,
                },
            );
        }
        self.inner.metrics.dropped_env(dropped_env);

        let now_ms = self.inner.clock.now().unix_millis();
        let mut tx = self.inner.db.begin().await?;
        let mut changed: Vec<ServiceRef> = Vec::new();
        let held = self.held_services(&mut tx, swimlane).await?;
        self.apply_services(&mut tx, swimlane, &wanted, &held, now_ms, &mut changed)
            .await?;
        if c.full {
            self.retire_services(&mut tx, &wanted, &held, now_ms, &mut changed)
                .await?;
        }
        self.apply_hints(&mut tx, swimlane, &c, now_ms).await?;
        if let Some(started) = c.config_server_started_at {
            self.inner
                .db
                .run(
                    "config_server_start",
                    sqlx::query(&format!(
                        "UPDATE swimlanes SET config_server_started_at = {} WHERE id = $1",
                        ts("$2")
                    ))
                    .bind(swimlane.as_str())
                    .bind(started.unix_millis())
                    .execute(&mut *tx),
                )
                .await?;
        }
        let mut events: Vec<DomainEvent> = Vec::new();
        self.apply_windows(&mut tx, swimlane, &c, &mut events).await?;
        for chunk in changed.chunks(EVENT_SERVICES) {
            events.push(DomainEvent::PendingRestartChanged {
                swimlane: swimlane.clone(),
                services: chunk.to_vec(),
            });
        }
        // The events of the report, in order, as one batch (P15).
        let mut handle = Tx::from_pg(&mut tx);
        crate::batch::publish_all(&*self.inner.bus, &mut handle, events).await?;
        self.inner.db.commit(tx).await
    }

    /// The Deployments the hub holds for a swimlane, by namespace and name, with their ids.
    async fn held_services(&self, tx: &mut Txn, swimlane: &SwimlaneId) -> Result<Held, RegistryError> {
        let limit =
            i64::try_from(self.inner.config.max_services_per_report.saturating_mul(2)).unwrap_or(i64::MAX);
        let rows: Vec<HeldRow> = self
            .inner
            .db
            .run(
                "services_held",
                sqlx::query_as(
                    "SELECT s.id, s.namespace, s.name, s.removed_at IS NOT NULL, \
                            COALESCE(st.pods, '[]'::jsonb), s.env_values, s.env_names \
                     FROM services s LEFT JOIN service_state st ON st.service_id = s.id \
                     WHERE s.swimlane_id = $1 ORDER BY s.id LIMIT $2",
                )
                .bind(swimlane.as_str())
                .bind(limit)
                .fetch_all(&mut **tx),
            )
            .await?;
        let mut held = Held::new();
        for (id, ns, name, removed, pods, env_values, mut env_names) in rows {
            let env_values: BTreeMap<String, String> = env_values
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            env_names.sort();
            held.insert(
                (ns, name),
                (
                    id,
                    HeldService {
                        removed,
                        pods: pods_from_json(&pods),
                        env_values,
                        env_names,
                    },
                ),
            );
        }
        Ok(held)
    }

    /// Insert or update the Deployments of the report that differ from what the hub holds, and note that all of them were seen.
    async fn apply_services(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        wanted: &BTreeMap<(String, String), Service>,
        held: &Held,
        now_ms: i64,
        changed: &mut Vec<ServiceRef>,
    ) -> Result<(), RegistryError> {
        let mut seen_ids: Vec<i64> = Vec::new();
        for (key, svc) in wanted {
            let existing = held.get(key);
            let same = existing.is_some_and(|(_, h)| {
                h.removed == svc.removed
                    && h.pods == svc.pods
                    && h.env_values == svc.env_values
                    && h.env_names == svc.env_names
            });
            if let Some((id, _)) = existing {
                seen_ids.push(*id);
            }
            if same {
                continue;
            }
            let pods_changed = existing.is_none_or(|(_, h)| h.pods != svc.pods || h.removed != svc.removed);
            let id = self.upsert_service(tx, swimlane, svc, now_ms).await?;
            seen_ids.push(id);
            if pods_changed {
                if let Ok(r) = ServiceRef::new(&svc.key.0, &svc.key.1) {
                    changed.push(r);
                }
            }
        }
        // Everything the report names has been seen now; refresh in one statement.
        if !seen_ids.is_empty() {
            self.inner
                .db
                .run(
                    "services_seen",
                    sqlx::query(&format!(
                        "UPDATE services SET last_seen_at = {} WHERE id = ANY($1::bigint[])",
                        ts("$2")
                    ))
                    .bind(&seen_ids)
                    .bind(now_ms)
                    .execute(&mut **tx),
                )
                .await?;
        }
        Ok(())
    }

    /// A full report is the whole list: a Deployment it does not name is gone.
    async fn retire_services(
        &self,
        tx: &mut Txn,
        wanted: &BTreeMap<(String, String), Service>,
        held: &Held,
        now_ms: i64,
        changed: &mut Vec<ServiceRef>,
    ) -> Result<(), RegistryError> {
        for (key, (id, h)) in held {
            if wanted.contains_key(key) || h.removed {
                continue;
            }
            self.inner
                .db
                .run(
                    "services_gone",
                    sqlx::query(&format!(
                        "UPDATE services SET removed_at = {} WHERE id = $1",
                        ts("$2")
                    ))
                    .bind(id)
                    .bind(now_ms)
                    .execute(&mut **tx),
                )
                .await?;
            self.inner
                .db
                .run(
                    "services_gone_pods",
                    sqlx::query(&format!(
                        "UPDATE service_state SET pods = '[]'::jsonb, oldest_pod_started_at = NULL, \
                           newest_pod_started_at = NULL, updated_at = {} WHERE service_id = $1",
                        ts("$2")
                    ))
                    .bind(id)
                    .bind(now_ms)
                    .execute(&mut **tx),
                )
                .await?;
            if let Ok(r) = ServiceRef::new(&key.0, &key.1) {
                changed.push(r);
            }
        }
        Ok(())
    }

    async fn upsert_service(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        svc: &Service,
        now_ms: i64,
    ) -> Result<i64, RegistryError> {
        let env_values = serde_json::to_value(&svc.env_values).map_err(|_| RegistryError::Invalid)?;
        let id: i64 = self.inner
            .db
            .run(
                "service_upsert",
                sqlx::query_scalar(&format!(
                    "INSERT INTO services (swimlane_id, namespace, name, env_values, env_names, first_seen_at, last_seen_at, removed_at) \
                     VALUES ($1, $2, $3, $4, $5, {now}, {now}, CASE WHEN $6 THEN {now} ELSE NULL END) \
                     ON CONFLICT (swimlane_id, namespace, name) DO UPDATE SET \
                       env_values = EXCLUDED.env_values, env_names = EXCLUDED.env_names, last_seen_at = EXCLUDED.last_seen_at, \
                       removed_at = CASE WHEN $6 THEN COALESCE(services.removed_at, EXCLUDED.last_seen_at) ELSE NULL END \
                     RETURNING id",
                    now = ts("$7")
                ))
                .bind(swimlane.as_str())
                .bind(&svc.key.0)
                .bind(&svc.key.1)
                .bind(env_values)
                .bind(&svc.env_names)
                .bind(svc.removed)
                .bind(now_ms)
                .fetch_one(&mut **tx),
            )
            .await?;
        let pods = if svc.removed { Vec::new() } else { svc.pods.clone() };
        let oldest = pods.iter().map(|(_, at)| *at).min();
        let newest = pods.iter().map(|(_, at)| *at).max();
        self.inner.db
            .run(
                "service_state_upsert",
                sqlx::query(&format!(
                    "INSERT INTO service_state (service_id, pods, oldest_pod_started_at, newest_pod_started_at, updated_at) \
                     VALUES ($1, $2, CASE WHEN $3::bigint IS NULL THEN NULL ELSE {oldest} END, \
                             CASE WHEN $4::bigint IS NULL THEN NULL ELSE {newest} END, {now}) \
                     ON CONFLICT (service_id) DO UPDATE SET pods = EXCLUDED.pods, \
                       oldest_pod_started_at = EXCLUDED.oldest_pod_started_at, \
                       newest_pod_started_at = EXCLUDED.newest_pod_started_at, updated_at = EXCLUDED.updated_at",
                    oldest = ts("$3"),
                    newest = ts("$4"),
                    now = ts("$5"),
                ))
                .bind(id)
                .bind(pods_json(&pods))
                .bind(oldest)
                .bind(newest)
                .bind(now_ms)
                .execute(&mut **tx),
            )
            .await?;
        Ok(id)
    }

    async fn apply_hints(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        c: &ClusterReport,
        now_ms: i64,
    ) -> Result<(), RegistryError> {
        if !c.release_hints.is_empty() {
            let mut namespaces = Vec::new();
            let mut services = Vec::new();
            let mut keys = Vec::new();
            let mut values = Vec::new();
            for h in &c.release_hints {
                if h.key.as_str().is_empty() {
                    continue;
                }
                namespaces.push(h.service.namespace().to_owned());
                services.push(h.service.name().to_owned());
                keys.push(h.key.as_str().to_owned());
                values.push(h.value.as_str().to_owned());
            }
            self.inner.db
                .run(
                    "hints_upsert",
                    sqlx::query(&format!(
                        "INSERT INTO release_hints (swimlane_id, namespace, service, key, value, reported_at) \
                         SELECT $1, x.ns, x.svc, x.key, x.value, {now} \
                         FROM unnest($2::text[], $3::text[], $4::text[], $5::text[]) AS x(ns, svc, key, value) \
                         ON CONFLICT (swimlane_id, namespace, service, key) DO UPDATE SET \
                           value = EXCLUDED.value, reported_at = EXCLUDED.reported_at",
                        now = ts("$6")
                    ))
                    .bind(swimlane.as_str())
                    .bind(&namespaces)
                    .bind(&services)
                    .bind(&keys)
                    .bind(&values)
                    .bind(now_ms)
                    .execute(&mut **tx),
                )
                .await?;
        }
        // A full report is the whole picture: a hint it does not repeat is gone.
        if c.full {
            self.inner
                .db
                .run(
                    "hints_prune",
                    sqlx::query(&format!(
                        "DELETE FROM release_hints WHERE swimlane_id = $1 AND reported_at <> {}",
                        ts("$2")
                    ))
                    .bind(swimlane.as_str())
                    .bind(now_ms)
                    .execute(&mut **tx),
                )
                .await?;
        }
        Ok(())
    }

    /// Sync windows (D75): one row per Job, opened once and closed once, each change with its event. A report that repeats a
    /// window changes nothing.
    async fn apply_windows(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        c: &ClusterReport,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), RegistryError> {
        for w in &c.sync_windows {
            let at = w.at.unix_millis();
            match w.kind {
                SyncWindowKind::Opened => {
                    let done = self.inner
                        .db
                        .run(
                            "window_open",
                            sqlx::query(&format!(
                                "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at) VALUES ($1, $2, $3, {}) \
                                 ON CONFLICT (swimlane_id, job_uid) DO NOTHING",
                                ts("$4")
                            ))
                            .bind(swimlane.as_str())
                            .bind(w.job.name())
                            .bind(w.job.uid())
                            .bind(at)
                            .execute(&mut **tx),
                        )
                        .await?;
                    if done.rows_affected() == 1 {
                        events.push(DomainEvent::SyncWindowOpened {
                            swimlane: swimlane.clone(),
                            job: w.job.clone(),
                        });
                    }
                }
                SyncWindowKind::Closed => {
                    if self.close_window(tx, swimlane, &w.job, at).await? {
                        events.push(DomainEvent::SyncWindowClosed {
                            swimlane: swimlane.clone(),
                            job: w.job.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Close a window; a Job the hub never saw open is recorded as open and closed at the same instant. `true` when this call
    /// closed it.
    async fn close_window(
        &self,
        tx: &mut Txn,
        swimlane: &SwimlaneId,
        job: &JobRef,
        at: i64,
    ) -> Result<bool, RegistryError> {
        let closed = self.inner
            .db
            .run(
                "window_close",
                sqlx::query(&format!(
                    "UPDATE sync_windows SET closed_at = {} WHERE swimlane_id = $1 AND job_uid = $2 AND closed_at IS NULL",
                    ts("$3")
                ))
                .bind(swimlane.as_str())
                .bind(job.uid())
                .bind(at)
                .execute(&mut **tx),
            )
            .await?;
        if closed.rows_affected() == 1 {
            return Ok(true);
        }
        let inserted = self
            .inner
            .db
            .run(
                "window_close_unseen",
                sqlx::query(&format!(
                    "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at, closed_at) \
                     VALUES ($1, $2, $3, {t}, {t}) ON CONFLICT (swimlane_id, job_uid) DO NOTHING",
                    t = ts("$4")
                ))
                .bind(swimlane.as_str())
                .bind(job.name())
                .bind(job.uid())
                .bind(at)
                .execute(&mut **tx),
            )
            .await?;
        Ok(inserted.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_have_the_shape_of_environment_variables() {
        for ok in ["PATH", "_X", "JAVA_OPTS", "a1", "CONFIG_CLIENT_CACHE_TTL"] {
            assert!(is_env_name(ok), "{ok}");
        }
        for bad in [
            "",
            "1A",
            "A B",
            "A=b",
            "A-B",
            "password=hunter2",
            "päth",
            &"A".repeat(129),
        ] {
            assert!(!is_env_name(bad), "{bad}");
        }
        assert!(is_env_name(&"A".repeat(128)));
    }

    #[test]
    fn pods_round_trip_in_a_stable_order() {
        let pods = vec![("b".to_owned(), 2), ("a".to_owned(), 1)];
        let json = pods_json(&pods);
        assert_eq!(
            pods_from_json(&json),
            vec![("a".to_owned(), 1), ("b".to_owned(), 2)]
        );
        assert!(pods_from_json(&serde_json::json!("not an array")).is_empty());
        assert!(pods_from_json(&serde_json::json!([{"name": 1}])).is_empty());
    }
}
