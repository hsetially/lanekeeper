//! Findings, newest first (T6, P12).
//!
//! Open findings only (`resolved_at IS NULL`), by id descending, keyset on the id. A swimlane filter uses the partial indexes the table
//! has for open findings; the kinds and the minimum severity are filters on the page. Severity ordering of the list is T9's.

use domain::{
    Finding, FindingId, FindingKind, FindingsQuery, NfsPath, Paged, SettingLocation, Severity, SwimlaneId,
    Timestamp,
};
use ports::ReadError;
use sqlx::Row;

use super::cursor::{self, Context, Field, Kind};
use super::rows;
use super::service::Inner;
use crate::db::ms;
use crate::error::RegistryError;

/// `$1` a swimlane or `NULL`, `$2` kinds (an empty array means every kind), `$3` the lowest severity rank (0 for all), `$4` the id after
/// which to resume (`NULL` for the first page), `$5` the page size plus one.
pub fn findings_sql() -> String {
    format!(
        "SELECT id, kind, swimlane_id, path, severity, rule_ids, message, locations, {detected} AS detected_ms \
         FROM findings \
         WHERE resolved_at IS NULL \
           AND ($1::text IS NULL OR swimlane_id = $1) \
           AND (cardinality($2::text[]) = 0 OR kind = ANY($2)) \
           AND (CASE severity WHEN 'low' THEN 0 WHEN 'medium' THEN 1 WHEN 'high' THEN 2 ELSE 3 END) >= $3 \
           AND ($4::bigint IS NULL OR id < $4) \
         ORDER BY id DESC LIMIT $5",
        detected = ms("detected_at"),
    )
}

const fn severity_rank(s: Severity) -> i32 {
    match s {
        Severity::Low => 0,
        Severity::Medium => 1,
        Severity::High => 2,
        Severity::Critical => 3,
    }
}

fn finding_of(row: &sqlx::postgres::PgRow) -> Result<Finding, RegistryError> {
    let id: i64 = row.get("id");
    let kind: String = row.get("kind");
    let swimlane: String = row.get("swimlane_id");
    let path: Option<String> = row.get("path");
    let severity: String = row.get("severity");
    let rule_ids: Vec<String> = row.get("rule_ids");
    let message: String = row.get("message");
    let locations: serde_json::Value = row.get("locations");
    let detected_ms: i64 = row.get("detected_ms");
    Ok(Finding {
        id: FindingId::from(id),
        kind: kind.parse::<FindingKind>().map_err(|_| RegistryError::Corrupt)?,
        swimlane: SwimlaneId::parse(&swimlane).map_err(|_| RegistryError::Corrupt)?,
        path: path
            .map(|p| NfsPath::parse(&p).map_err(|_| RegistryError::Corrupt))
            .transpose()?,
        severity: severity.parse().map_err(|_| RegistryError::Corrupt)?,
        rule_ids: rule_ids
            .iter()
            .map(|r| rows::short(r))
            .collect::<Result<_, _>>()?,
        message: rows::short(&message)?,
        // The engine wrote them as `SettingLocation`s; a value that does not read back is shown as no location, not as an error.
        locations: serde_json::from_value::<Vec<SettingLocation>>(locations).unwrap_or_default(),
        detected_at: Timestamp::from_unix_millis(detected_ms),
    })
}

impl Inner {
    pub async fn findings_page(&self, q: &FindingsQuery) -> Result<Paged<Finding>, ReadError> {
        if let Some(s) = &q.swimlane {
            self.require_swimlane(s).await?;
        }
        let mut kinds: Vec<&str> = q.kinds.iter().map(|k| k.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let joined = kinds.join(",");
        let context = Context::of(
            Kind::Findings,
            &[
                q.swimlane.as_ref().map_or("", SwimlaneId::as_str),
                &joined,
                q.min_severity.map_or("", Severity::as_str),
            ],
        );
        let after = match cursor::resume(&q.page, Kind::Findings, &context)? {
            Some(fields) => {
                cursor::arity(&fields, 1).map_err(|_| ReadError::BadRequest)?;
                Some(cursor::int(&fields, 0).map_err(|_| ReadError::BadRequest)?)
            }
            None => None,
        };
        let limit = q.page.limit() as usize;
        let fetched = self
            .db
            .run(
                "read_findings",
                sqlx::query(&findings_sql())
                    .bind(q.swimlane.as_ref().map(SwimlaneId::as_str))
                    .bind(&kinds)
                    .bind(q.min_severity.map_or(0, severity_rank))
                    .bind(after)
                    .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let has_more = fetched.len() > limit;
        let kept = &fetched[..fetched.len().min(limit)];
        let items = kept.iter().map(finding_of).collect::<Result<Vec<_>, _>>()?;
        let next = if has_more {
            items
                .last()
                .map(|f| cursor::issue(Kind::Findings, &context, vec![Field::Int(f.id.get())]))
                .transpose()?
        } else {
            None
        };
        Ok(Paged { items, next })
    }
}
