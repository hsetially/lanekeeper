//! The swimlane list and the detail of one (T6, P7: 50 ms).
//!
//! One statement reads a page of swimlanes with everything the summary needs (the tenants as an array, the agent's liveness, the last
//! scan, the confirmed base version, the counts the pipeline keeps in `swimlane_registry`, and, since `contract/08-web` `CG4a`, the tier, the
//! project's name, the baseline, the pickup counts and the open sync window), so a list costs one round trip however many swimlanes it has.
//! Ordered by id, keyset on the id.
//!
//! # The additions of `CG4a`
//!
//! | Field | Source | Rule |
//! |---|---|---|
//! | `tier` | `swimlanes.tier` | as stored; absent when none is set |
//! | `project` | `projects.display_name` through `swimlanes.project_id` | the project's name (D7); a swimlane always has a project |
//! | `baseline` | `swimlanes.editing_unlocked`, `baseline_confirmed_*`, the open `adoptions` | `in_progress` while an adoption is open (`status` `in_progress` or `review`, the partial index `adoptions_one_open`), even on a swimlane whose earlier baseline was confirmed (D17); else `confirmed` when editing is unlocked **and** a confirmation time is recorded (the writes' lock, D2, and the contract's "says when" agree); else `unconfirmed`. `by` is the confirming user's id and display name (Q6: never an email), and `by` and `at` are only given for `confirmed` |
//! | `pickup_counts` | `service_state.pickup_state` of the services that are not removed, grouped | every state but `live`, each once, no zero, in the order of [`PickupState`]; the count of what `pending_restarts` lists, not of the port's first 500 |
//! | `syncing` | the open `sync_windows` row, `sync_windows_open_idx` | the window that has been open longest (earliest `opened_at`, then id), when more than one Job is syncing |
//!
//! Each is a correlated lookup of one swimlane's rows by an index, inside the one statement, so a page costs no more round trips than before.

use domain::{
    AgentStatus, BaselineState, BaselineStatus, DriftState, Guid, JobRef, Page, Paged, PickupState,
    SwimlaneId, SwimlaneSummary, SyncingInfo, TenantId, UserId, UserRef, VersionLabel,
};
use ports::ReadError;
use serde_json::Value;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind};
use super::rows;
use super::service::Inner;
use crate::error::RegistryError;
use crate::http::dto::SwimlaneDetail;

/// The most swimlanes the port's unpaged `swimlanes` returns (the design scale is 40).
pub const MAX_SWIMLANES: u32 = 1_000;

/// The columns the additions of `CG4a` need (see the module documentation), shared by both statements. `s` is the swimlane, `p` its project, `bu`
/// the user who confirmed the baseline and `w` the open sync window.
macro_rules! addition_columns {
    () => {
        "s.tier, p.display_name AS project_name, s.editing_unlocked, \
       (EXTRACT(EPOCH FROM s.baseline_confirmed_at) * 1000)::bigint AS confirmed_ms, \
       s.baseline_confirmed_by_tid AS confirmed_tid, s.baseline_confirmed_by_oid AS confirmed_oid, bu.display_name AS confirmed_name, \
       EXISTS (SELECT 1 FROM adoptions d WHERE d.swimlane_id = s.id AND d.status IN ('in_progress', 'review')) AS adopting, \
       COALESCE((SELECT jsonb_object_agg(q.pickup_state, q.n) FROM ( \
                   SELECT st.pickup_state, count(*) AS n FROM services sv JOIN service_state st ON st.service_id = sv.id \
                   WHERE sv.swimlane_id = s.id AND sv.removed_at IS NULL AND st.pickup_state <> 'live' \
                   GROUP BY st.pickup_state) q), '{}'::jsonb) AS pickup_counts, \
       w.job_name AS sync_job, w.job_uid AS sync_uid, (EXTRACT(EPOCH FROM w.opened_at) * 1000)::bigint AS sync_ms"
    };
}

/// The joins of those columns: the project, the confirming user (both by primary key) and the open sync window (the first by
/// `sync_windows_open_idx`).
macro_rules! addition_joins {
    () => {
        "LEFT JOIN projects p ON p.id = s.project_id \
         LEFT JOIN users bu ON bu.tid = s.baseline_confirmed_by_tid AND bu.oid = s.baseline_confirmed_by_oid \
         LEFT JOIN LATERAL (SELECT w0.job_name, w0.job_uid, w0.opened_at FROM sync_windows w0 \
                            WHERE w0.swimlane_id = s.id AND w0.closed_at IS NULL ORDER BY w0.opened_at, w0.id LIMIT 1) w ON true"
    };
}

/// A page of swimlanes with their summaries: after the id `$1` (`NULL` for the first page), at most `$2`, the agent counting as
/// connected when its connection row was renewed within `$3` seconds.
pub const SWIMLANES_SQL: &str = concat!(
    "SELECT s.id, s.display_name, \
       COALESCE((SELECT array_agg(t.tenant_id ORDER BY t.tenant_id) FROM swimlane_tenants t WHERE t.swimlane_id = s.id), '{}') AS tenants, \
       EXISTS (SELECT 1 FROM agent_connections c WHERE c.swimlane_id = s.id AND c.replica_address <> '' \
               AND c.last_seen_at > now() - make_interval(secs => $3::double precision)) AS connected, \
       (a.swimlane_id IS NOT NULL AND (a.joined_at IS NOT NULL OR a.last_hello_at IS NOT NULL)) AS joined, \
       (EXTRACT(EPOCH FROM k.updated_at) * 1000)::bigint AS last_scan_ms, COALESCE(k.acked_seq, 0) AS acked_seq, \
       COALESCE(r.state_counts, '{}'::jsonb) AS state_counts, COALESCE(r.finding_count, 0) AS finding_count, \
       (SELECT d.base_label FROM adoptions d WHERE d.swimlane_id = s.id AND d.status = 'confirmed' AND d.base_label IS NOT NULL \
        ORDER BY d.id DESC LIMIT 1) AS base_label, ",
    addition_columns!(),
    " FROM swimlanes s \
LEFT JOIN agents a ON a.swimlane_id = s.id \
LEFT JOIN agent_acks k ON k.swimlane_id = s.id \
LEFT JOIN swimlane_registry r ON r.swimlane_id = s.id ",
    addition_joins!(),
    " WHERE ($1::text IS NULL OR s.id > $1) \
ORDER BY s.id LIMIT $2"
);

/// One swimlane by id, in the same shape (`$1` is the id, `$2` the liveness window in seconds).
pub const SWIMLANE_SQL: &str = concat!(
    "SELECT s.id, s.display_name, \
       COALESCE((SELECT array_agg(t.tenant_id ORDER BY t.tenant_id) FROM swimlane_tenants t WHERE t.swimlane_id = s.id), '{}') AS tenants, \
       EXISTS (SELECT 1 FROM agent_connections c WHERE c.swimlane_id = s.id AND c.replica_address <> '' \
               AND c.last_seen_at > now() - make_interval(secs => $2::double precision)) AS connected, \
       (a.swimlane_id IS NOT NULL AND (a.joined_at IS NOT NULL OR a.last_hello_at IS NOT NULL)) AS joined, \
       (EXTRACT(EPOCH FROM k.updated_at) * 1000)::bigint AS last_scan_ms, COALESCE(k.acked_seq, 0) AS acked_seq, \
       COALESCE(r.state_counts, '{}'::jsonb) AS state_counts, COALESCE(r.finding_count, 0) AS finding_count, \
       (SELECT d.base_label FROM adoptions d WHERE d.swimlane_id = s.id AND d.status = 'confirmed' AND d.base_label IS NOT NULL \
        ORDER BY d.id DESC LIMIT 1) AS base_label, s.auto_notify, ",
    addition_columns!(),
    " FROM swimlanes s \
LEFT JOIN agents a ON a.swimlane_id = s.id \
LEFT JOIN agent_acks k ON k.swimlane_id = s.id \
LEFT JOIN swimlane_registry r ON r.swimlane_id = s.id ",
    addition_joins!(),
    " WHERE s.id = $1"
);

/// Whether a swimlane notifies the config-server by itself after a write (D85): `$1` is the id. A swimlane nobody switched on has the
/// column's default, `false`. Read only: the admin operations that write it are not in this crate (A7).
pub const AUTO_NOTIFY_SQL: &str = "SELECT auto_notify FROM swimlanes WHERE id = $1";

fn summary_of(row: &sqlx::postgres::PgRow) -> Result<SwimlaneSummary, RegistryError> {
    let id: String = row.get("id");
    let display: String = row.get("display_name");
    let tenants: Vec<String> = row.get("tenants");
    let connected: bool = row.get("connected");
    let joined: bool = row.get("joined");
    let last_scan_ms: Option<i64> = row.get("last_scan_ms");
    let acked_seq: i64 = row.get("acked_seq");
    let counts: Value = row.get("state_counts");
    let finding_count: i32 = row.get("finding_count");
    let label: Option<Value> = row.get("base_label");
    let tier: Option<String> = row.get("tier");
    let project: Option<String> = row.get("project_name");
    let pickup: Value = row.get("pickup_counts");

    let agent = if connected {
        AgentStatus::Connected
    } else if joined {
        AgentStatus::Disconnected
    } else {
        AgentStatus::NeverSeen
    };
    let tenants = tenants
        .iter()
        .map(|t| TenantId::parse(t).map_err(|_| RegistryError::Corrupt))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SwimlaneSummary {
        id: SwimlaneId::parse(&id).map_err(|_| RegistryError::Corrupt)?,
        display_name: rows::short(&display)?,
        tenants,
        agent,
        // A swimlane whose agent has never delivered a scan (nothing acknowledged) has no last scan.
        last_scan_at: if acked_seq > 0 {
            rows::timestamp(last_scan_ms)
        } else {
            None
        },
        base_version: label.and_then(|v| serde_json::from_value::<VersionLabel>(v).ok()),
        drift_counts: drift_counts(&counts),
        finding_count: u32::try_from(finding_count).unwrap_or(0),
        tier: tier.as_deref().map(rows::short).transpose()?,
        project: project.as_deref().map(rows::short).transpose()?,
        baseline: Some(baseline_of(row)?),
        pickup_counts: pickup_counts(&pickup),
        syncing: syncing_of(row)?,
    })
}

/// The baseline of a swimlane from its row (see the module documentation): an open adoption is `in_progress` and names nobody, a baseline is
/// `confirmed` when editing is unlocked and a time is recorded, and anything else is `unconfirmed`.
fn baseline_of(row: &sqlx::postgres::PgRow) -> Result<BaselineStatus, RegistryError> {
    let (adopting, unlocked): (bool, bool) = (row.get("adopting"), row.get("editing_unlocked"));
    let confirmed_ms: Option<i64> = row.get("confirmed_ms");
    let state = |state| BaselineStatus {
        state,
        by: None,
        at: None,
    };
    if adopting {
        return Ok(state(BaselineState::InProgress));
    }
    let Some(at) = confirmed_ms.filter(|_| unlocked) else {
        return Ok(state(BaselineState::Unconfirmed));
    };
    let (tid, oid, name): (Option<String>, Option<String>, Option<String>) = (
        row.get("confirmed_tid"),
        row.get("confirmed_oid"),
        row.get("confirmed_name"),
    );
    // Who confirmed it: the user's id and display name, never the email (Q6). A user the row cannot name (an id that is no Entra guid, which the
    // registry never writes) is left out, and the baseline is still confirmed.
    let by = match (tid, oid, name) {
        (Some(tid), Some(oid), Some(name)) => match (Guid::parse(&tid), Guid::parse(&oid)) {
            (Ok(tid), Ok(oid)) => Some(UserRef {
                id: UserId::new(tid, oid),
                display_name: rows::short(&name)?,
            }),
            _ => None,
        },
        _ => None,
    };
    Ok(BaselineStatus {
        state: BaselineState::Confirmed,
        by,
        at: rows::timestamp(Some(at)),
    })
}

/// The open sync window of a swimlane's row, if it has one.
fn syncing_of(row: &sqlx::postgres::PgRow) -> Result<Option<SyncingInfo>, RegistryError> {
    let (job, uid, opened): (Option<String>, Option<String>, Option<i64>) =
        (row.get("sync_job"), row.get("sync_uid"), row.get("sync_ms"));
    let (Some(job), Some(uid), Some(opened)) = (job, uid, opened) else {
        return Ok(None);
    };
    Ok(Some(SyncingInfo {
        job: JobRef::new(&job, &uid).map_err(|_| RegistryError::Corrupt)?,
        since: domain::Timestamp::from_unix_millis(opened),
    }))
}

/// `{"needs_notify_or_restart": 2, "live_within_ttl": 1}` as the non-zero counts of the states that are not live, in the order of the states. A
/// key that is not a state, or is `live`, is left out.
fn pickup_counts(counts: &Value) -> Vec<(PickupState, u32)> {
    let Some(map) = counts.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<(PickupState, u32)> = map
        .iter()
        .filter_map(|(k, v)| {
            let state: PickupState = k.parse().ok()?;
            let n = u32::try_from(v.as_u64()?).ok()?;
            (state != PickupState::Live && n > 0).then_some((state, n))
        })
        .collect();
    out.sort_by_key(|(s, _)| *s);
    out
}

/// `{"in_sync": 3, "conflict": 1}` as the non-zero counts in the order of the states. A key that is not a state is left out.
fn drift_counts(counts: &Value) -> Vec<(DriftState, u32)> {
    let Some(map) = counts.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<(DriftState, u32)> = map
        .iter()
        .filter_map(|(k, v)| {
            let state: DriftState = k.parse().ok()?;
            let n = u32::try_from(v.as_u64()?).ok()?;
            (n > 0).then_some((state, n))
        })
        .collect();
    out.sort_by_key(|(s, _)| *s);
    out
}

impl Inner {
    /// The page of swimlanes after the cursor.
    pub async fn swimlanes_page(&self, page: &Page) -> Result<Paged<SwimlaneSummary>, ReadError> {
        let context = Context::of(Kind::Swimlanes, &[]);
        let after = match cursor::resume(page, Kind::Swimlanes, &context)? {
            Some(fields) => {
                cursor::arity(&fields, 1).map_err(|_| ReadError::BadRequest)?;
                Some(cursor::text(&fields, 0).map_err(|_| ReadError::BadRequest)?)
            }
            None => None,
        };
        let limit = i64::from(page.limit());
        let fetched = self
            .db
            .run(
                "read_swimlanes",
                sqlx::query(SWIMLANES_SQL)
                    .bind(after)
                    .bind(limit + 1)
                    .bind(self.config.agent_live_window.as_secs_f64())
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut items = fetched.iter().map(summary_of).collect::<Result<Vec<_>, _>>()?;
        let next = if items.len() > page.limit() as usize {
            items.truncate(page.limit() as usize);
            items
                .last()
                .map(|s| cursor::issue(Kind::Swimlanes, &context, vec![Field::text(s.id.as_str())]))
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }

    /// Every swimlane, for the port's unpaged list (at most [`MAX_SWIMLANES`]).
    pub async fn swimlanes_all(&self) -> Result<Vec<SwimlaneSummary>, ReadError> {
        let mut out = Vec::new();
        let mut page = Page::new(500, None).map_err(|_| ReadError::BadRequest)?;
        loop {
            let got = self.swimlanes_page(&page).await?;
            out.extend(got.items);
            match got.next {
                Some(c) if u32::try_from(out.len()).is_ok_and(|n| n < MAX_SWIMLANES) => {
                    page = Page::new(500, Some(c)).map_err(|_| ReadError::BadRequest)?;
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// One swimlane with its services (a swimlane the reader may not know is `NotFound`).
    pub async fn swimlane_detail(&self, id: &SwimlaneId) -> Result<SwimlaneDetail, ReadError> {
        let row = self
            .db
            .run(
                "read_swimlane",
                sqlx::query(SWIMLANE_SQL)
                    .bind(id.as_str())
                    .bind(self.config.agent_live_window.as_secs_f64())
                    .fetch_optional(self.db.pool()),
            )
            .await?
            .ok_or(ReadError::NotFound)?;
        let summary = summary_of(&row)?;
        let auto_notify: bool = row.get("auto_notify");
        let services = self
            .services_of(id, None, super::restarts::MAX_SERVICES)
            .await?
            .items;
        Ok(SwimlaneDetail {
            summary,
            services,
            auto_notify,
        })
    }

    /// The swimlane's auto-notify setting (the port's `auto_notify`); a swimlane the reader may not know is `NotFound`.
    pub async fn auto_notify_of(&self, id: &SwimlaneId) -> Result<bool, ReadError> {
        self.db
            .run(
                "read_auto_notify",
                sqlx::query_scalar(AUTO_NOTIFY_SQL)
                    .bind(id.as_str())
                    .fetch_optional(self.db.pool()),
            )
            .await?
            .ok_or(ReadError::NotFound)
    }

    /// Whether a swimlane exists. Every read that names a swimlane checks it first, so an unknown one is `NotFound` and not an empty page.
    pub async fn require_swimlane(&self, id: &SwimlaneId) -> Result<(), ReadError> {
        let exists: bool = self
            .db
            .run(
                "read_swimlane_exists",
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM swimlanes WHERE id = $1)")
                    .bind(id.as_str())
                    .fetch_one(self.db.pool()),
            )
            .await?;
        if exists { Ok(()) } else { Err(ReadError::NotFound) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_pickup_counts_are_the_non_zero_states_that_are_not_live_in_order() {
        let counts = json!({
            "needs_notify_or_restart": 2, "live_within_ttl": 1, "live": 9, "needs_config_server_restart": 0, "bogus": 3
        });
        assert_eq!(
            pickup_counts(&counts),
            vec![
                (PickupState::LiveWithinTtl, 1),
                (PickupState::NeedsNotifyOrRestart, 2)
            ]
        );
        assert!(pickup_counts(&json!([1])).is_empty());
    }

    #[test]
    fn the_counts_are_the_non_zero_states_in_order() {
        let counts = json!({"in_sync": 7, "conflict": 2, "git_ahead": 0, "bogus": 3, "unknown": "x"});
        assert_eq!(
            drift_counts(&counts),
            vec![(DriftState::InSync, 7), (DriftState::Conflict, 2)]
        );
        assert!(drift_counts(&json!([1, 2])).is_empty());
    }
}
