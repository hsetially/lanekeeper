//! Services and what they have not picked up yet (T6, D85).
//!
//! `service_state.pickup_state` is the pipeline's answer (T10 computes it with the engine's `pickup_state` from the file role, each
//! Deployment's client settings, the pods' start times and the config-server's start); a service with no state row has picked up
//! everything. This module only reads. The two client settings a response shows come from the allowlisted environment values the agent
//! reported (D88), read as `CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED=true` and `CONFIG_CLIENT_CACHE_TTL=<seconds>`, with the engine's
//! default of 20 minutes when it is absent or is not a number.

use domain::{NfsPath, Page, Paged, PickupState, ServiceRef, ServiceState, SwimlaneId};
use engine::DEFAULT_CACHE_TTL_SECS;
use ports::ReadError;
use serde_json::Value;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind};
use super::rows;
use super::service::Inner;
use crate::db::ms;
use crate::error::RegistryError;

/// The most services in a swimlane's detail, and the most pending files of one service: the contract's `maxItems`.
pub const MAX_SERVICES: u32 = 500;
pub const MAX_PENDING_FILES: usize = 500;

/// `$1` swimlane, `$2` the service id after which to resume, `$3` true for every service and false for only those that have not picked
/// up everything, `$4` the page size plus one.
pub fn services_sql() -> String {
    format!(
        "SELECT s.id, s.namespace, s.name, s.env_values, COALESCE(st.pickup_state, 'live') AS pickup, \
                {pending} AS pending_ms, {live_by} AS live_by_ms, COALESCE(st.pending_files, '[]'::jsonb) AS pending_files \
         FROM services s LEFT JOIN service_state st ON st.service_id = s.id \
         WHERE s.swimlane_id = $1 AND s.removed_at IS NULL AND s.id > $2 \
           AND ($3 OR COALESCE(st.pickup_state, 'live') <> 'live') \
         ORDER BY s.id LIMIT $4",
        pending = ms("st.pending_since"),
        live_by = ms("st.live_by"),
    )
}

/// The two client settings of a Deployment from its allowlisted environment values.
pub(crate) fn client_settings(env: &Value) -> (bool, u32) {
    let notifications = env
        .get("CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED")
        .and_then(Value::as_str)
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    let ttl = env
        .get("CONFIG_CLIENT_CACHE_TTL")
        .and_then(Value::as_str)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_CACHE_TTL_SECS);
    (notifications, ttl)
}

fn service_state(row: &sqlx::postgres::PgRow) -> Result<ServiceState, RegistryError> {
    let namespace: String = row.get("namespace");
    let name: String = row.get("name");
    let env: Value = row.get("env_values");
    let pickup: String = row.get("pickup");
    let pending: Option<i64> = row.get("pending_ms");
    let live_by: Option<i64> = row.get("live_by_ms");
    let files: Value = row.get("pending_files");
    let (notifications_enabled, cache_ttl_secs) = client_settings(&env);
    let pending_files: Vec<NfsPath> = files
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().and_then(|s| NfsPath::parse(s).ok()))
                .take(MAX_PENDING_FILES)
                .collect()
        })
        .unwrap_or_default();
    Ok(ServiceState {
        service: ServiceRef::new(&namespace, &name).map_err(|_| RegistryError::Corrupt)?,
        pickup: pickup
            .parse::<PickupState>()
            .map_err(|_| RegistryError::Corrupt)?,
        pending_since: rows::timestamp(pending),
        live_by: rows::timestamp(live_by),
        notifications_enabled,
        cache_ttl_secs,
        pending_files,
    })
}

impl Inner {
    /// A page of the services of a swimlane: every service, or only those that have not picked up everything.
    pub async fn services_page(
        &self,
        swimlane: &SwimlaneId,
        only_pending: bool,
        page: &Page,
    ) -> Result<Paged<ServiceState>, ReadError> {
        self.require_swimlane(swimlane).await?;
        let context = Context::of(
            Kind::Restarts,
            &[swimlane.as_str(), if only_pending { "pending" } else { "all" }],
        );
        let after = match cursor::resume(page, Kind::Restarts, &context)? {
            Some(fields) => {
                cursor::arity(&fields, 1).map_err(|_| ReadError::BadRequest)?;
                cursor::int(&fields, 0).map_err(|_| ReadError::BadRequest)?
            }
            None => 0,
        };
        let limit = page.limit() as usize;
        let (items, ids, has_more) = self.services_rows(swimlane, after, !only_pending, limit).await?;
        let next = if has_more {
            ids.last()
                .map(|id| cursor::issue(Kind::Restarts, &context, vec![Field::Int(*id)]))
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }

    /// The services of a swimlane for its detail: every service, up to `limit`. (`after` is not used: the detail is one page.)
    pub async fn services_of(
        &self,
        swimlane: &SwimlaneId,
        _after: Option<i64>,
        limit: u32,
    ) -> Result<Paged<ServiceState>, ReadError> {
        let (items, _, _) = self.services_rows(swimlane, 0, true, limit as usize).await?;
        Ok(Paged { items, next: None })
    }

    async fn services_rows(
        &self,
        swimlane: &SwimlaneId,
        after: i64,
        all: bool,
        limit: usize,
    ) -> Result<(Vec<ServiceState>, Vec<i64>, bool), ReadError> {
        let fetched = self
            .db
            .run(
                "read_services",
                sqlx::query(&services_sql())
                    .bind(swimlane.as_str())
                    .bind(after)
                    .bind(all)
                    .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = fetched.len() > limit;
        let kept = &fetched[..fetched.len().min(limit)];
        let items = kept.iter().map(service_state).collect::<Result<Vec<_>, _>>()?;
        let ids = kept.iter().map(|r| r.get::<i64, _>("id")).collect();
        Ok((items, ids, has_more))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_client_settings_come_from_the_allowlisted_values_with_the_engine_default() {
        assert_eq!(client_settings(&json!({})), (false, DEFAULT_CACHE_TTL_SECS));
        assert_eq!(
            client_settings(
                &json!({"CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED": "TRUE", "CONFIG_CLIENT_CACHE_TTL": "60"})
            ),
            (true, 60)
        );
        assert_eq!(
            client_settings(
                &json!({"CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED": "no", "CONFIG_CLIENT_CACHE_TTL": "20m"})
            ),
            (false, DEFAULT_CACHE_TTL_SECS),
            "a value that is not a number is the default, not an error"
        );
    }
}
