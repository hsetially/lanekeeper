//! The rows of the pass (`adoptions`, D22): what is stored, how it is read, and the vocabularies of its CHECKs
//! (`db/migrations/README.md`: reason, status and step). Nothing here talks to Git.
//!
//! Every write that moves an open pass carries `status = 'in_progress'` in its WHERE: a pass that was cancelled or confirmed in
//! the meantime is not touched, and the caller learns it from the number of rows (`NotOpen`).

use std::collections::BTreeMap;

use domain::{CommitId, GitRef, SwimlaneId, TenantId, User, VersionLabel};
use serde_json::Value;

use super::hints::Hint;
use super::secrets::SecretHit;
use super::{AdoptionError, AdoptionView, Inner, RankedBranch};
use crate::db::{Txn, ts};
use crate::error::RegistryError;

/// Why the pass exists (`adoptions.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdoptionReason {
    /// The first pass of a swimlane: no baseline yet.
    Initial,
    /// The tenant suffixes on NFS stopped agreeing with the swimlane's tenant set (T4).
    TenantSuffixChanged,
}

impl AdoptionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::TenantSuffixChanged => "tenant_suffix_changed",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self, RegistryError> {
        match s {
            "initial" => Ok(Self::Initial),
            "tenant_suffix_changed" => Ok(Self::TenantSuffixChanged),
            _ => Err(RegistryError::Corrupt),
        }
    }
}

/// Where the pass is (`adoptions.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdoptionStatus {
    /// An admin is working on it.
    InProgress,
    /// Raised by the registry, waiting for an admin (T4).
    Review,
    Confirmed,
    Cancelled,
}

impl AdoptionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Review => "review",
            Self::Confirmed => "confirmed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self, RegistryError> {
        match s {
            "in_progress" => Ok(Self::InProgress),
            "review" => Ok(Self::Review),
            "confirmed" => Ok(Self::Confirmed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(RegistryError::Corrupt),
        }
    }

    /// An admin can still work on it.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::InProgress | Self::Review)
    }
}

/// The wizard step to resume at (`adoptions.step`). Declared in the order of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdoptionStep {
    Branch,
    BaseVersion,
    Files,
    Secrets,
    Confirm,
}

impl AdoptionStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::BaseVersion => "base_version",
            Self::Files => "files",
            Self::Secrets => "secrets",
            Self::Confirm => "confirm",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self, RegistryError> {
        match s {
            "branch" => Ok(Self::Branch),
            "base_version" => Ok(Self::BaseVersion),
            "files" => Ok(Self::Files),
            "secrets" => Ok(Self::Secrets),
            "confirm" => Ok(Self::Confirm),
            _ => Err(RegistryError::Corrupt),
        }
    }
}

/// A row of `adoptions`, as it is read.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub id: i64,
    pub swimlane: SwimlaneId,
    pub reason: AdoptionReason,
    pub status: AdoptionStatus,
    pub step: AdoptionStep,
    pub ranking: Value,
    pub hints: Value,
    pub tenants: Vec<String>,
    pub tenant_commits: Value,
    pub base_commit: Option<String>,
    pub base_label: Option<Value>,
    pub secret_scan: Value,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

type RowTuple = (
    i64,
    String,
    String,
    String,
    String,
    Value,
    Value,
    Vec<String>,
    Value,
    Option<String>,
    Option<Value>,
    Value,
    i64,
    i64,
);

/// The columns of a row, with the times as Unix milliseconds (`crate::db::ms`). A macro so that the statements below are one literal each
/// (SQL is never built by formatting a string, S11).
macro_rules! columns {
    () => {
        "id, swimlane_id, reason, status, step, ranking, hints, tenants, tenant_commits, base_commit, base_label, secret_scan, \
         (EXTRACT(EPOCH FROM started_at) * 1000)::bigint, (EXTRACT(EPOCH FROM updated_at) * 1000)::bigint"
    };
}

const GET_SQL: &str = concat!("SELECT ", columns!(), " FROM adoptions WHERE id = $1");

const OPEN_SQL: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM adoptions WHERE swimlane_id = $1 AND reason = $2 AND status IN ('in_progress', 'review') \
     ORDER BY id DESC LIMIT 1"
);

impl Row {
    fn from_tuple(t: RowTuple) -> Result<Self, RegistryError> {
        let (
            id,
            swimlane,
            reason,
            status,
            step,
            ranking,
            hints,
            tenants,
            tenant_commits,
            base_commit,
            base_label,
            secret_scan,
            started,
            updated,
        ) = t;
        Ok(Self {
            id,
            swimlane: SwimlaneId::parse(&swimlane).map_err(|_| RegistryError::Corrupt)?,
            reason: AdoptionReason::parse(&reason)?,
            status: AdoptionStatus::parse(&status)?,
            step: AdoptionStep::parse(&step)?,
            ranking,
            hints,
            tenants,
            tenant_commits,
            base_commit,
            base_label,
            secret_scan,
            started_at_ms: started,
            updated_at_ms: updated,
        })
    }

    /// The tenants the admin chose.
    pub(crate) fn chosen(&self) -> Result<Vec<TenantId>, RegistryError> {
        self.tenants
            .iter()
            .map(|t| TenantId::parse(t).map_err(|_| RegistryError::Corrupt))
            .collect()
    }

    /// The commit each chosen tenant was at when it was chosen.
    pub(crate) fn commits(&self) -> Result<BTreeMap<TenantId, CommitId>, RegistryError> {
        let Some(map) = self.tenant_commits.as_object() else {
            return Err(RegistryError::Corrupt);
        };
        map.iter()
            .map(|(tenant, commit)| {
                let tenant = TenantId::parse(tenant).map_err(|_| RegistryError::Corrupt)?;
                let commit = commit
                    .as_str()
                    .and_then(|c| CommitId::parse(c).ok())
                    .ok_or(RegistryError::Corrupt)?;
                Ok((tenant, commit))
            })
            .collect()
    }

    pub(crate) fn base(&self) -> Result<Option<CommitId>, RegistryError> {
        self.base_commit
            .as_deref()
            .map(|c| CommitId::parse(c).map_err(|_| RegistryError::Corrupt))
            .transpose()
    }
}

/// The ranking as the JSON a row stores. The same four keys the tenant-suffix review writes (T4), so that both producers of a ranking
/// agree on what a ranking is.
pub(crate) fn ranking_json(ranked: &[super::rank::Ranked]) -> Value {
    Value::Array(ranked.iter().map(super::rank::Ranked::to_json).collect())
}

fn parse_ranking(value: &Value) -> Result<Vec<RankedBranch>, RegistryError> {
    let Some(list) = value.as_array() else {
        return Err(RegistryError::Corrupt);
    };
    list.iter()
        .map(|b| {
            let text = |key: &str| b.get(key).and_then(Value::as_str);
            let number = |key: &str| b.get(key).and_then(Value::as_u64);
            Ok(RankedBranch {
                branch: text("branch")
                    .and_then(|s| GitRef::parse(s).ok())
                    .ok_or(RegistryError::Corrupt)?,
                tenant: text("tenant")
                    .and_then(|s| TenantId::parse(s).ok())
                    .ok_or(RegistryError::Corrupt)?,
                shared: number("shared").ok_or(RegistryError::Corrupt)?,
                union: number("union").ok_or(RegistryError::Corrupt)?,
            })
        })
        .collect()
}

impl Row {
    pub(crate) fn view(&self, resolutions: usize) -> Result<AdoptionView, RegistryError> {
        let foreign_suffixes = self
            .hints
            .get("foreign_suffixes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().and_then(|s| TenantId::parse(s).ok()))
                    .collect()
            })
            .unwrap_or_default();
        let mut hints: Vec<Hint> = Vec::new();
        for key in ["env", "release"] {
            if let Some(list) = self.hints.get(key).and_then(Value::as_array) {
                hints.extend(list.iter().filter_map(|h| Hint::from_json(key, h)));
            }
        }
        let base_label = match &self.base_label {
            Some(v) => {
                Some(serde_json::from_value::<VersionLabel>(v.clone()).map_err(|_| RegistryError::Corrupt)?)
            }
            None => None,
        };
        Ok(AdoptionView {
            id: self.id,
            swimlane: self.swimlane.clone(),
            reason: self.reason,
            status: self.status,
            step: self.step,
            ranking: parse_ranking(&self.ranking)?,
            hints,
            foreign_suffixes,
            tenants: self.chosen()?,
            tenant_commits: self.commits()?,
            base_commit: self.base()?,
            base_label,
            secret_scan: SecretHit::list_from_json(&self.secret_scan)?,
            resolutions,
            started_at_ms: self.started_at_ms,
            updated_at_ms: self.updated_at_ms,
        })
    }
}

impl Inner {
    /// A row by id.
    pub(crate) async fn row(&self, id: i64) -> Result<Row, AdoptionError> {
        let found: Option<RowTuple> = self
            .db
            .run(
                "adoption_row",
                sqlx::query_as(GET_SQL).bind(id).fetch_optional(self.db.pool()),
            )
            .await?;
        match found {
            Some(t) => Ok(Row::from_tuple(t)?),
            None => Err(AdoptionError::NotOpen),
        }
    }

    /// A row an admin is working on: `in_progress`. A review nobody has started is `NotOpen` for the calls after `start`, which is what
    /// takes it.
    pub(crate) async fn working_row(&self, id: i64) -> Result<Row, AdoptionError> {
        let row = self.row(id).await?;
        if row.status == AdoptionStatus::InProgress {
            Ok(row)
        } else {
            Err(AdoptionError::NotOpen)
        }
    }

    /// The open row of a swimlane for a reason: `adoptions_one_open` allows one.
    pub(crate) async fn open_of(
        &self,
        swimlane: &SwimlaneId,
        reason: AdoptionReason,
    ) -> Result<Option<Row>, RegistryError> {
        let found: Option<RowTuple> = self
            .db
            .run(
                "adoption_open",
                sqlx::query_as(OPEN_SQL)
                    .bind(swimlane.as_str())
                    .bind(reason.as_str())
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        found.map(Row::from_tuple).transpose()
    }

    pub(crate) async fn resolution_count(&self, id: i64) -> Result<usize, RegistryError> {
        let n: i64 = self
            .db
            .run(
                "adoption_resolution_count",
                sqlx::query_scalar("SELECT count(*) FROM adoption_resolutions WHERE adoption_id = $1")
                    .bind(id)
                    .fetch_one(self.db.pool()),
            )
            .await?;
        Ok(usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// A pass as the wizard shows it.
    pub(crate) async fn view(&self, id: i64) -> Result<AdoptionView, AdoptionError> {
        let row = self.row(id).await?;
        let resolutions = self.resolution_count(id).await?;
        Ok(row.view(resolutions)?)
    }

    pub(crate) async fn open_view(
        &self,
        swimlane: &SwimlaneId,
        reason: AdoptionReason,
    ) -> Result<Option<AdoptionView>, RegistryError> {
        let Some(row) = self.open_of(swimlane, reason).await? else {
            return Ok(None);
        };
        let resolutions = self.resolution_count(row.id).await?;
        row.view(resolutions).map(Some)
    }

    /// The facts about a swimlane the pass needs: whether an admin has confirmed its baseline, and whether it has an inventory.
    pub(crate) async fn lane(&self, swimlane: &SwimlaneId) -> Result<Lane, AdoptionError> {
        let found: Option<(bool, bool)> = self
            .db
            .run(
                "adoption_lane",
                sqlx::query_as(
                    "SELECT s.baseline_confirmed_at IS NOT NULL, g.first_inventory_at IS NOT NULL \
                     FROM swimlanes s LEFT JOIN swimlane_registry g ON g.swimlane_id = s.id WHERE s.id = $1",
                )
                .bind(swimlane.as_str())
                .fetch_optional(self.db.pool()),
            )
            .await?;
        let (adopted, inventory) = found.ok_or(AdoptionError::Read(ports::ReadError::NotFound))?;
        Ok(Lane { adopted, inventory })
    }

    /// Move an open pass to `step` and stamp it. `false`: it is not `in_progress` any more.
    pub(crate) async fn set_step(&self, id: i64, step: AdoptionStep) -> Result<(), AdoptionError> {
        let now = self.clock.now().unix_millis();
        let sql = format!(
            "UPDATE adoptions SET step = $2, updated_at = {} WHERE id = $1 AND status = 'in_progress'",
            ts("$3")
        );
        let done = self
            .db
            .run(
                "adoption_step",
                sqlx::query(&sql)
                    .bind(id)
                    .bind(step.as_str())
                    .bind(now)
                    .execute(self.db.pool()),
            )
            .await?;
        if done.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        Ok(())
    }

    /// The admin who works on a row, for `started_by` of a review the registry raised.
    pub(crate) fn user_ids(u: &User) -> (String, String) {
        (u.id.tid().as_str().to_owned(), u.id.oid().as_str().to_owned())
    }

    /// Delete every resolution of a pass (the Git side changed, so what the admin looked at is not what is there).
    pub(crate) async fn clear_resolutions(&self, tx: &mut Txn, id: i64) -> Result<(), RegistryError> {
        self.db
            .run(
                "adoption_resolutions_clear",
                sqlx::query("DELETE FROM adoption_resolutions WHERE adoption_id = $1")
                    .bind(id)
                    .execute(&mut **tx),
            )
            .await?;
        Ok(())
    }
}

/// What the pass needs to know about a swimlane.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lane {
    /// An admin has confirmed the baseline (D2, D22).
    pub adopted: bool,
    /// The first full scan has been applied.
    pub inventory: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabularies_are_the_checks_of_the_table() {
        for (reason, name) in [
            (AdoptionReason::Initial, "initial"),
            (AdoptionReason::TenantSuffixChanged, "tenant_suffix_changed"),
        ] {
            assert_eq!(reason.as_str(), name);
            assert_eq!(AdoptionReason::parse(name), Ok(reason));
        }
        for (status, name, open) in [
            (AdoptionStatus::InProgress, "in_progress", true),
            (AdoptionStatus::Review, "review", true),
            (AdoptionStatus::Confirmed, "confirmed", false),
            (AdoptionStatus::Cancelled, "cancelled", false),
        ] {
            assert_eq!(status.as_str(), name);
            assert_eq!(AdoptionStatus::parse(name), Ok(status));
            assert_eq!(status.is_open(), open, "{name}");
        }
        for (step, name) in [
            (AdoptionStep::Branch, "branch"),
            (AdoptionStep::BaseVersion, "base_version"),
            (AdoptionStep::Files, "files"),
            (AdoptionStep::Secrets, "secrets"),
            (AdoptionStep::Confirm, "confirm"),
        ] {
            assert_eq!(step.as_str(), name);
            assert_eq!(AdoptionStep::parse(name), Ok(step));
        }
        assert!(AdoptionStep::Branch < AdoptionStep::BaseVersion);
        assert!(AdoptionStep::Files < AdoptionStep::Secrets && AdoptionStep::Secrets < AdoptionStep::Confirm);
        for bad in ["", "Initial", "done", "IN_PROGRESS"] {
            assert_eq!(AdoptionReason::parse(bad), Err(RegistryError::Corrupt));
            assert_eq!(AdoptionStatus::parse(bad), Err(RegistryError::Corrupt));
            assert_eq!(AdoptionStep::parse(bad), Err(RegistryError::Corrupt));
        }
    }

    #[test]
    fn the_ranking_is_the_four_keys_the_review_writes() {
        let json = serde_json::json!([
            {"branch": "sit2", "tenant": "sit2", "shared": 3, "union": 4},
            {"branch": "sit1", "tenant": "sit1", "shared": 0, "union": 7},
        ]);
        let parsed = parse_ranking(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].branch.as_str(), "sit2");
        assert_eq!((parsed[0].shared, parsed[0].union), (3, 4));
        assert_eq!(parse_ranking(&serde_json::json!({})), Err(RegistryError::Corrupt));
        assert_eq!(
            parse_ranking(&serde_json::json!([{"branch": "sit2"}])),
            Err(RegistryError::Corrupt)
        );
    }
}
