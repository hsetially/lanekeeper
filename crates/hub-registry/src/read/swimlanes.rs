//! The swimlane list and the detail of one (T6, P7: 50 ms).
//!
//! One statement reads a page of swimlanes with everything the summary needs (the tenants as an array, the agent's liveness, the last
//! scan, the confirmed base version, the counts the pipeline keeps in `swimlane_registry`), so a list costs one round trip however
//! many swimlanes it has. Ordered by id, keyset on the id.

use domain::{AgentStatus, DriftState, Page, Paged, SwimlaneId, SwimlaneSummary, TenantId, VersionLabel};
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

/// A page of swimlanes with their summaries: after the id `$1` (`NULL` for the first page), at most `$2`, the agent counting as
/// connected when its connection row was renewed within `$3` seconds.
pub const SWIMLANES_SQL: &str = "\
SELECT s.id, s.display_name, \
       COALESCE((SELECT array_agg(t.tenant_id ORDER BY t.tenant_id) FROM swimlane_tenants t WHERE t.swimlane_id = s.id), '{}') AS tenants, \
       EXISTS (SELECT 1 FROM agent_connections c WHERE c.swimlane_id = s.id AND c.replica_address <> '' \
               AND c.last_seen_at > now() - make_interval(secs => $3::double precision)) AS connected, \
       (a.swimlane_id IS NOT NULL AND (a.joined_at IS NOT NULL OR a.last_hello_at IS NOT NULL)) AS joined, \
       (EXTRACT(EPOCH FROM k.updated_at) * 1000)::bigint AS last_scan_ms, COALESCE(k.acked_seq, 0) AS acked_seq, \
       COALESCE(r.state_counts, '{}'::jsonb) AS state_counts, COALESCE(r.finding_count, 0) AS finding_count, \
       (SELECT d.base_label FROM adoptions d WHERE d.swimlane_id = s.id AND d.status = 'confirmed' AND d.base_label IS NOT NULL \
        ORDER BY d.id DESC LIMIT 1) AS base_label \
FROM swimlanes s \
LEFT JOIN agents a ON a.swimlane_id = s.id \
LEFT JOIN agent_acks k ON k.swimlane_id = s.id \
LEFT JOIN swimlane_registry r ON r.swimlane_id = s.id \
WHERE ($1::text IS NULL OR s.id > $1) \
ORDER BY s.id LIMIT $2";

/// One swimlane by id, in the same shape (`$1` is the id, `$2` the liveness window in seconds).
pub const SWIMLANE_SQL: &str = "\
SELECT s.id, s.display_name, \
       COALESCE((SELECT array_agg(t.tenant_id ORDER BY t.tenant_id) FROM swimlane_tenants t WHERE t.swimlane_id = s.id), '{}') AS tenants, \
       EXISTS (SELECT 1 FROM agent_connections c WHERE c.swimlane_id = s.id AND c.replica_address <> '' \
               AND c.last_seen_at > now() - make_interval(secs => $2::double precision)) AS connected, \
       (a.swimlane_id IS NOT NULL AND (a.joined_at IS NOT NULL OR a.last_hello_at IS NOT NULL)) AS joined, \
       (EXTRACT(EPOCH FROM k.updated_at) * 1000)::bigint AS last_scan_ms, COALESCE(k.acked_seq, 0) AS acked_seq, \
       COALESCE(r.state_counts, '{}'::jsonb) AS state_counts, COALESCE(r.finding_count, 0) AS finding_count, \
       (SELECT d.base_label FROM adoptions d WHERE d.swimlane_id = s.id AND d.status = 'confirmed' AND d.base_label IS NOT NULL \
        ORDER BY d.id DESC LIMIT 1) AS base_label, s.auto_notify \
FROM swimlanes s \
LEFT JOIN agents a ON a.swimlane_id = s.id \
LEFT JOIN agent_acks k ON k.swimlane_id = s.id \
LEFT JOIN swimlane_registry r ON r.swimlane_id = s.id \
WHERE s.id = $1";

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
    })
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
    fn the_counts_are_the_non_zero_states_in_order() {
        let counts = json!({"in_sync": 7, "conflict": 2, "git_ahead": 0, "bogus": 3, "unknown": "x"});
        assert_eq!(
            drift_counts(&counts),
            vec![(DriftState::InSync, 7), (DriftState::Conflict, 2)]
        );
        assert!(drift_counts(&json!([1, 2])).is_empty());
    }
}
