//! The columns of `audit_events` that copy the event (S9).
//!
//! `event_json` is the evidence: its hash is chained, and the table's `CHECK` covers it. The other columns (`actor_ref`, `action`,
//! `swimlane_id`, `at` and the rest) are copies of fields of that event, written by [`super::log::PgAuditLog`] so that listings and
//! filters need no JSON parsing. Nothing chains them. An owner who switches the append-only trigger off can change `actor_ref`
//! and leave the event, the hash, the links and the checkpoints alone, and everything that reads the columns then shows a false
//! attribution. The nightly verifier therefore compares each column with the field it was copied from, in the stored text.
//!
//! The stored text is read as a [`serde_json::Value`], not as a [`ports::AuditEvent`]: a later change to the event type must not
//! turn old rows into false alarms. Each column is compared the way it was written, so a row written by the real log never
//! differs:
//!
//! - an optional field that is `null` (or absent) in the event is SQL `NULL` in the column, and the other way round;
//! - `at` is a number of milliseconds in the event and a `timestamptz` in the column, compared to the microsecond;
//! - a hash is lowercase hex in the event and 32 bytes in the column;
//! - the attribution columns are `NULL` when the event has no attribution; `attribution_actor` is `NULL` when the attribution
//!   has no actor; the two `jsonb` columns are compared as JSON values, so the order of keys does not matter.
//!
//! - `at` is read as text, from a `CASE` that gives `NULL` for `infinity` and `-infinity`, so that no stored time can make the
//!   query itself fail. A time that is not finite, or whose microseconds do not fit an `i64`, is `None` and is never the copy of
//!   any event's time;
//! - a column whose stored value cannot be decoded (a `jsonb` holding a number or a nesting `serde_json` refuses, a `NULL` in a
//!   column that is `NOT NULL` by the schema) is reported as differing, by name, and the walk goes on. A copy that cannot be read
//!   is not the copy of anything. Only a failure that is not about the value (a missing column, a connection) is an error.
//!
//! - **every value read is bounded in the query**, not after it. A text or `bytea` column is read as
//!   `CASE WHEN octet_length(col) <= N THEN col END`, a `jsonb` column as `... octet_length(col::text) <= N ...`, and a bitmask
//!   column `oversize` says which were left out. A value over its bound is never sent to this process, never buffered, never
//!   decoded: it is reported as that column differing, like any value no event can have written. The bound is derived from the one
//!   the log enforces: a copy is a part of one event, and the log refuses an event over [`MAX_EVENT_JSON_BYTES`] (see
//!   [`TEXT_COLUMN_LIMIT`], [`JSONB_TEXT_LIMIT`]). Without it one row with a few hundred megabytes in a `jsonb` column, which
//!   nothing else limits, ends every nightly run out of memory and silent.
//!
//! A difference is reported by the column's name only, never its value (S10).

use std::fmt;
use std::sync::LazyLock;

use domain::ContentHash;
use ports::AuditActor;
use serde_json::{Map, Value};
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::log::{MAX_EVENT_JSON_BYTES, actor_parts};

/// The most a copied text column can legitimately hold. A copy is a part of one event, and an event is at most
/// [`MAX_EVENT_JSON_BYTES`] of JSON (the log checks this before it takes the lock, and the table's `CHECK` backs it); the text of
/// a JSON string is never longer than the string. In practice every field is far shorter (a `ShortText` has 256 bytes), but this is
/// the bound that cannot be wrong.
pub const TEXT_COLUMN_LIMIT: usize = MAX_EVENT_JSON_BYTES;
/// The most a copied `jsonb` column can legitimately hold, as `octet_length(col::text)`. The column is a part of the event, at most
/// [`MAX_EVENT_JSON_BYTES`] as compact JSON; `jsonb` prints one space after every `,` and `:`, and each is next to an element of at
/// least one byte, so its text is at most one and a half times the compact form. Twice is the bound.
pub const JSONB_TEXT_LIMIT: usize = 2 * MAX_EVENT_JSON_BYTES;
const _: () = assert!(JSONB_TEXT_LIMIT >= MAX_EVENT_JSON_BYTES + MAX_EVENT_JSON_BYTES / 2);
/// A hash column holds 32 bytes.
pub const HASH_COLUMN_BYTES: usize = 32;

/// A denormalised column of `audit_events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    At,
    ActorKind,
    ActorRef,
    Action,
    Via,
    SwimlaneId,
    Path,
    HashBefore,
    HashAfter,
    GithubLogin,
    ApprovalId,
    RequestId,
    AttributionSource,
    AttributionConfidence,
    AttributionActor,
    AttributionEvidence,
}

impl Column {
    /// Every compared column, in table order.
    pub const ALL: [Self; 16] = [
        Self::At,
        Self::ActorKind,
        Self::ActorRef,
        Self::Action,
        Self::Via,
        Self::SwimlaneId,
        Self::Path,
        Self::HashBefore,
        Self::HashAfter,
        Self::GithubLogin,
        Self::ApprovalId,
        Self::RequestId,
        Self::AttributionSource,
        Self::AttributionConfidence,
        Self::AttributionActor,
        Self::AttributionEvidence,
    ];

    /// The column's name in the table.
    pub const fn name(self) -> &'static str {
        match self {
            Self::At => "at",
            Self::ActorKind => "actor_kind",
            Self::ActorRef => "actor_ref",
            Self::Action => "action",
            Self::Via => "via",
            Self::SwimlaneId => "swimlane_id",
            Self::Path => "path",
            Self::HashBefore => "hash_before",
            Self::HashAfter => "hash_after",
            Self::GithubLogin => "github_login",
            Self::ApprovalId => "approval_id",
            Self::RequestId => "request_id",
            Self::AttributionSource => "attribution_source",
            Self::AttributionConfidence => "attribution_confidence",
            Self::AttributionActor => "attribution_actor",
            Self::AttributionEvidence => "attribution_evidence",
        }
    }

    const fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// What kind of value the column holds, for its bound.
    const fn stored(self) -> Stored {
        match self {
            Self::At | Self::ApprovalId => Stored::Fixed,
            Self::ActorKind
            | Self::ActorRef
            | Self::Action
            | Self::Via
            | Self::SwimlaneId
            | Self::Path
            | Self::GithubLogin
            | Self::RequestId
            | Self::AttributionSource
            | Self::AttributionConfidence => Stored::Text,
            Self::HashBefore | Self::HashAfter => Stored::Hash,
            Self::AttributionActor | Self::AttributionEvidence => Stored::Jsonb,
        }
    }
}

/// How a column is stored, which decides how its size is measured and bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// A value of a fixed size (`timestamptz`, `bigint`): no bound needed.
    Fixed,
    Text,
    Hash,
    Jsonb,
}

/// A set of columns. Small and `Copy`, so a failure that carries one stays `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColumnSet(u32);

impl ColumnSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Every compared column: used when the stored event cannot be read at all, so that none of the copies can be trusted.
    pub const fn all() -> Self {
        Self((1 << Column::ALL.len()) - 1)
    }

    pub fn of(columns: &[Column]) -> Self {
        let mut set = Self::empty();
        for c in columns {
            set.insert(*c);
        }
        set
    }

    /// The set a bitmask of the `oversize` column stands for (bit `n` is the column at index `n` of [`Column::ALL`]).
    pub fn from_mask(mask: i32) -> Self {
        Self(u32::try_from(mask).unwrap_or(0) & Self::all().0)
    }

    pub fn insert(&mut self, c: Column) {
        self.0 |= c.bit();
    }

    /// Add every column of `other`.
    pub fn extend(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub const fn contains(self, c: Column) -> bool {
        self.0 & c.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The columns in the set, in table order.
    pub fn iter(self) -> impl Iterator<Item = Column> {
        Column::ALL.into_iter().filter(move |c| self.contains(*c))
    }

    /// The names of the columns in the set, in table order.
    pub fn names(self) -> Vec<&'static str> {
        self.iter().map(Column::name).collect()
    }
}

impl fmt::Display for ColumnSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, name) in self.names().into_iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(name)?;
        }
        Ok(())
    }
}

/// The select list that reads the columns [`read_row`] needs.
///
/// `at` is read as whole microseconds, which is what a `timestamptz` holds, so that a change of one microsecond shows. It is read
/// as exact text and parsed here: a cast to `bigint` in the query raises an error for `infinity` and for times beyond the last
/// microsecond an `i64` holds, and an error in the query is a failure of the whole walk, where the value is only a finding about
/// one row. The `CASE` is the guard for the first; the parse is the guard for the second.
///
/// Every other column that can hold more than a small value is bounded (see the module docs), and `oversize` is the bitmask of the
/// ones that were left out.
pub fn select_list() -> &'static str {
    static LIST: LazyLock<String> = LazyLock::new(build_select_list);
    LIST.as_str()
}

fn build_select_list() -> String {
    let mut parts = vec![
        "CASE WHEN isfinite(at) THEN (extract(epoch FROM at) * 1000000)::numeric(40,0)::text END AS at_micros".to_owned(),
    ];
    let mut oversize = Vec::new();
    for column in Column::ALL {
        let name = column.name();
        let (length, limit) = match column.stored() {
            Stored::Fixed => {
                if column != Column::At {
                    parts.push(name.to_owned());
                }
                continue;
            }
            Stored::Text => (format!("octet_length({name})"), TEXT_COLUMN_LIMIT),
            Stored::Hash => (format!("octet_length({name})"), HASH_COLUMN_BYTES),
            Stored::Jsonb => (format!("octet_length({name}::text)"), JSONB_TEXT_LIMIT),
        };
        parts.push(format!("CASE WHEN {length} <= {limit} THEN {name} END AS {name}"));
        oversize.push(format!(
            "CASE WHEN {length} > {limit} THEN {} ELSE 0 END",
            column.bit()
        ));
    }
    parts.push(format!("({})::int4 AS oversize", oversize.join(" + ")));
    parts.join(", ")
}

/// What one row of `audit_events` holds in the copied columns.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredColumns {
    /// Microseconds since the epoch; `None` when the stored time is `infinity`, `-infinity` or beyond what an `i64` of microseconds
    /// holds (and when it is `NULL`, which the schema forbids).
    pub at_micros: Option<i64>,
    pub actor_kind: String,
    pub actor_ref: Option<String>,
    pub action: String,
    pub via: String,
    pub swimlane_id: Option<String>,
    pub path: Option<String>,
    pub hash_before: Option<Vec<u8>>,
    pub hash_after: Option<Vec<u8>>,
    pub github_login: Option<String>,
    pub approval_id: Option<i64>,
    pub request_id: Option<String>,
    pub attribution_source: Option<String>,
    pub attribution_confidence: Option<String>,
    pub attribution_actor: Option<Value>,
    pub attribution_evidence: Option<Value>,
}

/// What [`read_row`] makes of a row: the columns that could be decoded, and the names of those that could not be (or were not read,
/// being over their bound).
#[derive(Debug, Clone, PartialEq)]
pub struct ReadColumns {
    /// The columns; one that could not be decoded holds its empty value (`None`, or an empty string), which no event matches.
    pub columns: StoredColumns,
    /// The columns whose stored value could not be decoded, or was over its bound and not sent.
    pub unreadable: ColumnSet,
}

/// Whether `e` says that a value in a fetched row could not be turned into the type asked for: the value is the problem. Anything
/// else (a column that is not in the row, a connection that broke) says nothing about the value.
pub fn is_decode_failure(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_))
}

/// Decode one column. A value that cannot be decoded marks the column in `unreadable` and gives the empty value.
fn read<'r, T>(
    row: &'r PgRow,
    name: &str,
    column: Column,
    unreadable: &mut ColumnSet,
) -> Result<T, sqlx::Error>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Default,
{
    match row.try_get(name) {
        Ok(value) => Ok(value),
        Err(e) if is_decode_failure(&e) => {
            unreadable.insert(column);
            Ok(T::default())
        }
        Err(e) => Err(e),
    }
}

/// Read the columns named by [`SELECT_LIST`] from a row.
///
/// # Errors
/// A column is missing from the row (the query and this function disagree), which is a fault and not a finding. A value that cannot
/// be decoded is not an error: see [`ReadColumns::unreadable`].
pub fn read_row(row: &PgRow) -> Result<ReadColumns, sqlx::Error> {
    // The columns the query left out for their size. It is computed by the query, so a failure to read it is a fault.
    let mut u = ColumnSet::from_mask(row.try_get::<i32, _>("oversize")?);
    let at: Option<String> = read(row, "at_micros", Column::At, &mut u)?;
    let columns = StoredColumns {
        // Text that is not an `i64` (a time past the last microsecond of one) is not a time any event holds.
        at_micros: at.and_then(|t| t.parse().ok()),
        actor_kind: read(row, "actor_kind", Column::ActorKind, &mut u)?,
        actor_ref: read(row, "actor_ref", Column::ActorRef, &mut u)?,
        action: read(row, "action", Column::Action, &mut u)?,
        via: read(row, "via", Column::Via, &mut u)?,
        swimlane_id: read(row, "swimlane_id", Column::SwimlaneId, &mut u)?,
        path: read(row, "path", Column::Path, &mut u)?,
        hash_before: read(row, "hash_before", Column::HashBefore, &mut u)?,
        hash_after: read(row, "hash_after", Column::HashAfter, &mut u)?,
        github_login: read(row, "github_login", Column::GithubLogin, &mut u)?,
        approval_id: read(row, "approval_id", Column::ApprovalId, &mut u)?,
        request_id: read(row, "request_id", Column::RequestId, &mut u)?,
        attribution_source: read(row, "attribution_source", Column::AttributionSource, &mut u)?,
        attribution_confidence: read(
            row,
            "attribution_confidence",
            Column::AttributionConfidence,
            &mut u,
        )?,
        attribution_actor: read(row, "attribution_actor", Column::AttributionActor, &mut u)?,
        attribution_evidence: read(row, "attribution_evidence", Column::AttributionEvidence, &mut u)?,
    };
    Ok(ReadColumns {
        columns,
        unreadable: u,
    })
}

/// The columns of a fetched row that are not the copy of a field of `event_json`: those that differ, and those that could not be
/// read at all.
///
/// # Errors
/// As [`read_row`].
pub fn differing_in_row(row: &PgRow, event_json: &str) -> Result<ColumnSet, sqlx::Error> {
    let read = read_row(row)?;
    let mut differ = differing_columns(event_json, &read.columns);
    differ.extend(read.unreadable);
    Ok(differ)
}

/// A field of the stored event, as it is compared with a column.
enum Field<T> {
    /// `null` or absent: the column must be SQL `NULL`.
    Null,
    Value(T),
    /// A value of a type the log never writes: no column can be its copy.
    Malformed,
}

impl<T> Field<T> {
    /// `object[key]`, with `read` turning a present value into a `T`.
    fn of<'a>(object: &'a Map<String, Value>, key: &str, read: impl FnOnce(&'a Value) -> Option<T>) -> Self {
        match object.get(key) {
            None | Some(Value::Null) => Self::Null,
            Some(v) => read(v).map_or(Self::Malformed, Self::Value),
        }
    }

    /// Whether the column holds exactly this field.
    fn is_copied_to(&self, column: Option<&T>) -> bool
    where
        T: PartialEq,
    {
        match (self, column) {
            (Self::Null, None) => true,
            (Self::Value(expected), Some(stored)) => expected == stored,
            _ => false,
        }
    }
}

/// The hash a column holds, if its bytes are a hash at all.
fn hash_column(bytes: Option<&Vec<u8>>) -> Result<Option<ContentHash>, ()> {
    match bytes {
        None => Ok(None),
        Some(b) => <[u8; 32]>::try_from(b.as_slice())
            .map(|h| Some(ContentHash::from_bytes(h)))
            .map_err(|_| ()),
    }
}

fn hash_field(object: &Map<String, Value>, key: &str, column: Option<&Vec<u8>>) -> bool {
    let expected = Field::of(object, key, |v| {
        v.as_str().and_then(|s| ContentHash::parse(s).ok())
    });
    hash_column(column).is_ok_and(|stored| expected.is_copied_to(stored.as_ref()))
}

/// The columns of `stored` that are not the copy of a field of the event in `event_json`.
///
/// `event_json` must be the text whose hash the chain already vouches for. If it is not a JSON object, every column is reported.
pub fn differing_columns(event_json: &str, stored: &StoredColumns) -> ColumnSet {
    let Ok(Value::Object(event)) = serde_json::from_str::<Value>(event_json) else {
        return ColumnSet::all();
    };
    let mut differ = ColumnSet::empty();
    let mut check = |column: Column, same: bool| {
        if !same {
            differ.insert(column);
        }
    };

    // The event holds milliseconds, the column microseconds.
    let at = event
        .get("at")
        .and_then(Value::as_i64)
        .and_then(|ms| ms.checked_mul(1000));
    check(Column::At, at.is_some() && at == stored.at_micros);
    check(
        Column::Action,
        event.get("action").and_then(Value::as_str) == Some(stored.action.as_str()),
    );
    check(
        Column::Via,
        event.get("via").and_then(Value::as_str) == Some(stored.via.as_str()),
    );
    let text = |key: &str| Field::of(&event, key, Value::as_str);
    check(
        Column::SwimlaneId,
        text("swimlane").is_copied_to(stored.swimlane_id.as_deref().as_ref()),
    );
    check(
        Column::Path,
        text("path").is_copied_to(stored.path.as_deref().as_ref()),
    );
    check(
        Column::GithubLogin,
        text("github_login").is_copied_to(stored.github_login.as_deref().as_ref()),
    );
    check(
        Column::RequestId,
        text("request_id").is_copied_to(stored.request_id.as_deref().as_ref()),
    );
    check(
        Column::ApprovalId,
        Field::of(&event, "approval", Value::as_i64).is_copied_to(stored.approval_id.as_ref()),
    );
    check(
        Column::HashBefore,
        hash_field(&event, "hash_before", stored.hash_before.as_ref()),
    );
    check(
        Column::HashAfter,
        hash_field(&event, "hash_after", stored.hash_after.as_ref()),
    );

    // The actor: the kind, and the one field of that kind that names it.
    let (kind_same, ref_same) = actor_columns(&event, stored);
    check(Column::ActorKind, kind_same);
    check(Column::ActorRef, ref_same);

    // The attribution: four columns.
    for (column, same) in attribution_columns(&event, stored) {
        check(column, same);
    }
    differ
}

/// Whether `actor_kind` and `actor_ref` are copies of the event's actor.
///
/// The kind and the reference are derived from the stored actor by [`actor_parts`], the function the log derives them with when it
/// writes, so a new kind of actor is handled in one place and the compiler forces that. A stored actor that is not an
/// [`AuditActor`] (a kind this version does not know) is not the origin of any column.
fn actor_columns(event: &Map<String, Value>, stored: &StoredColumns) -> (bool, bool) {
    let actor = event
        .get("actor")
        .and_then(|a| serde_json::from_value::<AuditActor>(a.clone()).ok());
    let Some((kind, reference)) = actor.as_ref().map(actor_parts) else {
        return (false, false);
    };
    (
        kind == stored.actor_kind,
        stored.actor_ref.as_deref() == Some(reference.as_str()),
    )
}

/// Whether each of the four attribution columns is a copy: all `NULL` when the event has no attribution, and `attribution_actor`
/// `NULL` when the attribution has no actor.
fn attribution_columns(event: &Map<String, Value>, stored: &StoredColumns) -> [(Column, bool); 4] {
    match Field::of(event, "attribution", Value::as_object) {
        Field::Null => [
            (Column::AttributionSource, stored.attribution_source.is_none()),
            (
                Column::AttributionConfidence,
                stored.attribution_confidence.is_none(),
            ),
            (Column::AttributionActor, stored.attribution_actor.is_none()),
            (Column::AttributionEvidence, stored.attribution_evidence.is_none()),
        ],
        Field::Value(a) => {
            // The source and the confidence are never `null` when there is an attribution.
            let required = |key: &str, column: &Option<String>| matches!(Field::of(a, key, Value::as_str), Field::Value(v) if column.as_deref() == Some(v));
            [
                (
                    Column::AttributionSource,
                    required("source", &stored.attribution_source),
                ),
                (
                    Column::AttributionConfidence,
                    required("confidence", &stored.attribution_confidence),
                ),
                (
                    Column::AttributionActor,
                    Field::of(a, "actor", Some).is_copied_to(stored.attribution_actor.as_ref().as_ref()),
                ),
                (
                    Column::AttributionEvidence,
                    Field::of(a, "evidence", Some)
                        .is_copied_to(stored.attribution_evidence.as_ref().as_ref()),
                ),
            ]
        }
        // An attribution that is not an object: no attribution column can be its copy.
        Field::Malformed => [
            (Column::AttributionSource, false),
            (Column::AttributionConfidence, false),
            (Column::AttributionActor, false),
            (Column::AttributionEvidence, false),
        ],
    }
}

#[cfg(test)]
mod tests {
    use domain::{Attribution, AttributionSource, Confidence, ProposalId, ShortText, Timestamp};
    use ports::{AuditAction, AuditActor, AuditEvent, AuditVia};

    use super::*;
    use crate::audit::log::prepare;

    fn bare() -> AuditEvent {
        AuditEvent::new(
            Timestamp::from_unix_millis(1_700_000_000_123),
            AuditActor::System {
                component: ShortText::parse("job").unwrap(),
            },
            AuditAction::SettingsChanged,
            AuditVia::System,
        )
    }

    fn full() -> AuditEvent {
        let mut e = bare();
        e.swimlane = Some(domain::SwimlaneId::parse("sit1").unwrap());
        e.path = Some(domain::NfsPath::parse("a/b.yml").unwrap());
        e.hash_before = Some(ContentHash::from_bytes([1; 32]));
        e.hash_after = Some(ContentHash::from_bytes([2; 32]));
        e.diff = Some("-a\n+b\n".to_owned());
        e.github_login = Some(ShortText::parse("octocat").unwrap());
        e.approval = Some(ProposalId::from(7));
        e.request_id = Some(domain::RequestId::parse("req-1").unwrap());
        e.attribution = Some(Attribution {
            source: AttributionSource::SentinelLogin,
            confidence: Confidence::High,
            actor: Some(domain::Actor::OsLogin {
                login: ShortText::parse("jdoe").unwrap(),
            }),
            evidence: domain::Evidence {
                executable: Some(ShortText::parse("/usr/bin/vim").unwrap()),
                login_identity: None,
                observed_at: Some(Timestamp::from_unix_millis(5)),
            },
        });
        e
    }

    /// The columns the log writes for `e`.
    fn written(e: &AuditEvent) -> (String, StoredColumns) {
        let p = prepare(e).unwrap();
        (p.json().to_owned(), p.columns().unwrap())
    }

    /// Change one column to a value it cannot have for this event.
    fn change(c: Column, s: &mut StoredColumns) {
        let text =
            |v: &mut Option<String>| *v = Some(v.take().map_or_else(|| "x".to_owned(), |t| format!("{t}x")));
        let hash = |v: &mut Option<Vec<u8>>| {
            *v = Some(
                v.take()
                    .map_or_else(|| vec![9; 32], |b| b.iter().map(|x| x ^ 1).collect()),
            );
        };
        match c {
            Column::At => s.at_micros = s.at_micros.map(|m| m + 1),
            Column::ActorKind => s.actor_kind.push('x'),
            Column::ActorRef => text(&mut s.actor_ref),
            Column::Action => s.action.push('x'),
            Column::Via => s.via.push('x'),
            Column::SwimlaneId => text(&mut s.swimlane_id),
            Column::Path => text(&mut s.path),
            Column::HashBefore => hash(&mut s.hash_before),
            Column::HashAfter => hash(&mut s.hash_after),
            Column::GithubLogin => text(&mut s.github_login),
            Column::ApprovalId => s.approval_id = Some(s.approval_id.map_or(1, |n| n + 1)),
            Column::RequestId => text(&mut s.request_id),
            Column::AttributionSource => text(&mut s.attribution_source),
            Column::AttributionConfidence => text(&mut s.attribution_confidence),
            Column::AttributionActor => {
                s.attribution_actor = Some(serde_json::json!({"kind": "os_login", "login": "someone-else"}));
            }
            Column::AttributionEvidence => s.attribution_evidence = Some(serde_json::json!({"other": 1})),
        }
    }

    #[test]
    fn a_row_as_the_log_writes_it_differs_in_no_column() {
        for e in [bare(), full()] {
            let (json, stored) = written(&e);
            assert_eq!(differing_columns(&json, &stored), ColumnSet::empty());
        }
    }

    #[test]
    fn each_column_changed_alone_is_the_only_one_reported() {
        for e in [bare(), full()] {
            for c in Column::ALL {
                let (json, mut stored) = written(&e);
                change(c, &mut stored);
                assert_eq!(
                    differing_columns(&json, &stored),
                    ColumnSet::of(&[c]),
                    "{} changed alone",
                    c.name()
                );
            }
        }
    }

    #[test]
    fn a_time_that_is_not_an_i64_of_microseconds_is_never_the_copy_of_a_time() {
        // `infinity`, `-infinity` and a time past the last microsecond of an `i64` are read as `None`, and `NULL` is too.
        let (json, mut stored) = written(&bare());
        stored.at_micros = None;
        assert_eq!(differing_columns(&json, &stored), ColumnSet::of(&[Column::At]));
        // Even for an event whose own time cannot be turned into microseconds: two nothings are not a match.
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["at"] = serde_json::json!(i64::MAX);
        assert_eq!(
            differing_columns(&v.to_string(), &stored),
            ColumnSet::of(&[Column::At])
        );
        v["at"] = serde_json::json!("infinity");
        assert_eq!(
            differing_columns(&v.to_string(), &stored),
            ColumnSet::of(&[Column::At])
        );
    }

    #[test]
    fn every_kind_of_actor_is_a_clean_row_and_a_changed_reference_is_not() {
        use ports::AuditActor;
        // `actor_parts` is an exhaustive match, so a new kind of actor does not compile until it is listed there; this lists the
        // kinds that exist, and holds the verifier to the columns the log writes for each.
        let actors = [
            AuditActor::User {
                user: ports::conformance::sample::user_id(7),
            },
            AuditActor::Agent {
                swimlane: ports::conformance::sample::swimlane("sit3"),
            },
            AuditActor::Sentinel {
                name: ShortText::parse("nfs-vm-2").unwrap(),
            },
            AuditActor::System {
                component: ShortText::parse("sync-job").unwrap(),
            },
        ];
        // Whatever `AuditActor` gains, the match below must be extended, and with it this test.
        for a in &actors {
            match a {
                AuditActor::User { .. }
                | AuditActor::Agent { .. }
                | AuditActor::Sentinel { .. }
                | AuditActor::System { .. } => {}
            }
        }
        for actor in actors {
            let mut e = bare();
            e.actor = actor;
            let (json, stored) = written(&e);
            assert_eq!(differing_columns(&json, &stored), ColumnSet::empty(), "{json}");
            let mut forged = stored.clone();
            forged.actor_ref = Some("someone-else".to_owned());
            assert_eq!(
                differing_columns(&json, &forged),
                ColumnSet::of(&[Column::ActorRef])
            );
            let mut forged = stored;
            forged.actor_kind = "robot".to_owned();
            assert_eq!(
                differing_columns(&json, &forged),
                ColumnSet::of(&[Column::ActorKind])
            );
        }
    }

    #[test]
    fn an_actor_this_version_does_not_know_is_the_origin_of_no_column() {
        let (json, stored) = written(&bare());
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["actor"] = serde_json::json!({"kind": "robot", "robot": "r2"});
        assert_eq!(
            differing_columns(&v.to_string(), &stored),
            ColumnSet::of(&[Column::ActorKind, Column::ActorRef])
        );
    }

    #[test]
    fn only_a_value_that_cannot_be_decoded_is_a_finding_and_anything_else_is_a_fault() {
        // A value in a fetched row that is not the type asked for is the data's problem ...
        let source: Box<dyn std::error::Error + Send + Sync> = "number out of range".into();
        assert!(is_decode_failure(&sqlx::Error::ColumnDecode {
            index: "attribution_actor".to_owned(),
            source,
        }));
        assert!(is_decode_failure(&sqlx::Error::Decode("x".into())));
        // ... and these are not: the query and the reader disagree, or the database is not there.
        assert!(!is_decode_failure(&sqlx::Error::ColumnNotFound("via".to_owned())));
        assert!(!is_decode_failure(&sqlx::Error::PoolClosed));
        assert!(!is_decode_failure(&sqlx::Error::PoolTimedOut));
        assert!(!is_decode_failure(&sqlx::Error::WorkerCrashed));
        assert!(!is_decode_failure(&sqlx::Error::Io(std::io::Error::other(
            "reset"
        ))));
        assert!(!is_decode_failure(&sqlx::Error::RowNotFound));
    }

    #[test]
    fn a_present_column_set_to_null_is_reported() {
        let (json, stored) = written(&full());
        for (c, nulled) in [
            (
                Column::ActorRef,
                StoredColumns {
                    actor_ref: None,
                    ..stored.clone()
                },
            ),
            (
                Column::SwimlaneId,
                StoredColumns {
                    swimlane_id: None,
                    ..stored.clone()
                },
            ),
            (
                Column::Path,
                StoredColumns {
                    path: None,
                    ..stored.clone()
                },
            ),
            (
                Column::HashBefore,
                StoredColumns {
                    hash_before: None,
                    ..stored.clone()
                },
            ),
            (
                Column::HashAfter,
                StoredColumns {
                    hash_after: None,
                    ..stored.clone()
                },
            ),
            (
                Column::GithubLogin,
                StoredColumns {
                    github_login: None,
                    ..stored.clone()
                },
            ),
            (
                Column::ApprovalId,
                StoredColumns {
                    approval_id: None,
                    ..stored.clone()
                },
            ),
            (
                Column::RequestId,
                StoredColumns {
                    request_id: None,
                    ..stored.clone()
                },
            ),
            (
                Column::AttributionSource,
                StoredColumns {
                    attribution_source: None,
                    ..stored.clone()
                },
            ),
            (
                Column::AttributionConfidence,
                StoredColumns {
                    attribution_confidence: None,
                    ..stored.clone()
                },
            ),
            (
                Column::AttributionActor,
                StoredColumns {
                    attribution_actor: None,
                    ..stored.clone()
                },
            ),
            (
                Column::AttributionEvidence,
                StoredColumns {
                    attribution_evidence: None,
                    ..stored.clone()
                },
            ),
        ] {
            assert_eq!(
                differing_columns(&json, &nulled),
                ColumnSet::of(&[c]),
                "{}",
                c.name()
            );
        }
    }

    #[test]
    fn an_attribution_without_an_actor_has_a_null_actor_column() {
        let mut e = bare();
        e.attribution = Some(Attribution::unknown());
        let (json, stored) = written(&e);
        assert!(stored.attribution_actor.is_none() && stored.attribution_evidence.is_some());
        assert_eq!(differing_columns(&json, &stored), ColumnSet::empty());
    }

    #[test]
    fn json_that_is_not_an_event_distrusts_every_column() {
        let (_, stored) = written(&bare());
        for text in ["", "not json", "[]", "null", "42", "\"x\""] {
            assert_eq!(differing_columns(text, &stored), ColumnSet::all(), "{text:?}");
        }
        // An object of another shape: nothing in it equals the columns.
        let set = differing_columns("{}", &stored);
        assert!(set.contains(Column::At) && set.contains(Column::ActorKind) && set.contains(Column::Action));
    }

    #[test]
    fn fields_of_the_wrong_type_are_a_difference_not_a_panic() {
        let (json, stored) = written(&full());
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["swimlane"] = serde_json::json!(5);
        v["approval"] = serde_json::json!("7");
        v["hash_before"] = serde_json::json!("zz");
        v["attribution"] = serde_json::json!("none");
        let set = differing_columns(&v.to_string(), &stored);
        for c in [
            Column::SwimlaneId,
            Column::ApprovalId,
            Column::HashBefore,
            Column::AttributionSource,
            Column::AttributionConfidence,
            Column::AttributionActor,
            Column::AttributionEvidence,
        ] {
            assert!(set.contains(c), "{}", c.name());
        }
        assert!(!set.contains(Column::Action));
    }

    #[test]
    fn a_future_field_in_the_event_is_not_a_difference() {
        let (json, stored) = written(&full());
        let mut v: Value = serde_json::from_str(&json).unwrap();
        v["a_field_added_later"] = serde_json::json!({"x": [1, 2]});
        v["attribution"]["another_new_field"] = serde_json::json!(true);
        assert_eq!(differing_columns(&v.to_string(), &stored), ColumnSet::empty());
    }

    #[test]
    fn jsonb_key_order_does_not_matter() {
        let (json, mut stored) = written(&full());
        // `jsonb` returns keys in its own order.
        stored.attribution_evidence = Some(
            serde_json::from_str(r#"{"observed_at":5,"login_identity":null,"executable":"/usr/bin/vim"}"#)
                .unwrap(),
        );
        assert_eq!(differing_columns(&json, &stored), ColumnSet::empty());
    }

    #[test]
    fn every_column_that_can_be_large_is_bounded_in_the_select_and_flagged_when_left_out() {
        let list = select_list();
        // Written out, not derived from `Column::stored`, so that a column moved to the wrong kind (or out of the bounded ones)
        // fails here.
        let text = [
            Column::ActorKind,
            Column::ActorRef,
            Column::Action,
            Column::Via,
            Column::SwimlaneId,
            Column::Path,
            Column::GithubLogin,
            Column::RequestId,
            Column::AttributionSource,
            Column::AttributionConfidence,
        ];
        let hashes = [Column::HashBefore, Column::HashAfter];
        let jsonb = [Column::AttributionActor, Column::AttributionEvidence];
        let fixed = [Column::At, Column::ApprovalId];
        assert_eq!(
            text.len() + hashes.len() + jsonb.len() + fixed.len(),
            Column::ALL.len()
        );
        for column in Column::ALL {
            let name = column.name();
            let (length, limit) = if text.contains(&column) {
                (format!("octet_length({name})"), TEXT_COLUMN_LIMIT)
            } else if hashes.contains(&column) {
                (format!("octet_length({name})"), HASH_COLUMN_BYTES)
            } else if jsonb.contains(&column) {
                (format!("octet_length({name}::text)"), JSONB_TEXT_LIMIT)
            } else {
                // Nothing to bound: `approval_id` is selected as it is, and `at` through its own `CASE`.
                assert!(fixed.contains(&column), "{name}");
                assert!(!list.contains(&format!("octet_length({name})")), "{name}");
                continue;
            };
            // Sent only under its bound ...
            let guard = format!("CASE WHEN {length} <= {limit} THEN {name} END AS {name}");
            assert_eq!(list.matches(&guard).count(), 1, "{name}: {list}");
            // ... and its bit is in the mask when it is not.
            let flag = format!("CASE WHEN {length} > {limit} THEN {} ELSE 0 END", column.bit());
            assert_eq!(list.matches(&flag).count(), 1, "{name}: {list}");
        }
        assert!(list.ends_with("::int4 AS oversize"), "{list}");
        assert_eq!(ColumnSet::from_mask(i32::MAX), ColumnSet::all());
        assert_eq!(ColumnSet::from_mask(-1), ColumnSet::empty());
        assert_eq!(
            ColumnSet::from_mask(i32::try_from(Column::Path.bit() | Column::Via.bit()).unwrap()),
            ColumnSet::of(&[Column::Path, Column::Via])
        );
        // The limits themselves, by value.
        assert_eq!(TEXT_COLUMN_LIMIT, 1_048_576);
        assert_eq!(JSONB_TEXT_LIMIT, 2_097_152);
        assert_eq!(HASH_COLUMN_BYTES, 32);
    }

    #[test]
    fn the_bounds_are_derived_from_the_size_an_event_can_have() {
        assert_eq!(TEXT_COLUMN_LIMIT, MAX_EVENT_JSON_BYTES);
        // `jsonb` text is at most one and a half times the compact JSON (a space after each `,` and `:`).
        assert_eq!(HASH_COLUMN_BYTES, 32);
    }

    #[test]
    fn what_the_log_writes_for_a_largest_event_is_under_every_bound() {
        // Each copy is a part of an event the log accepted, so it is no larger than the event; the bound is that event's size.
        let mut e = full();
        e.actor = AuditActor::System {
            component: ShortText::parse(&"é".repeat(128)).unwrap(),
        };
        e.github_login = Some(ShortText::parse(&"é".repeat(128)).unwrap());
        e.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES));
        let (json, stored) = written(&e);
        assert!(json.len() <= MAX_EVENT_JSON_BYTES);
        let text = [
            Some(stored.actor_kind.as_str()),
            stored.actor_ref.as_deref(),
            Some(stored.action.as_str()),
            Some(stored.via.as_str()),
            stored.swimlane_id.as_deref(),
            stored.path.as_deref(),
            stored.github_login.as_deref(),
            stored.request_id.as_deref(),
            stored.attribution_source.as_deref(),
            stored.attribution_confidence.as_deref(),
        ];
        for t in text.into_iter().flatten() {
            assert!(t.len() <= TEXT_COLUMN_LIMIT);
        }
        for h in [&stored.hash_before, &stored.hash_after].into_iter().flatten() {
            assert_eq!(h.len(), HASH_COLUMN_BYTES);
        }
        for v in [&stored.attribution_actor, &stored.attribution_evidence]
            .into_iter()
            .flatten()
        {
            // Compact length times one and a half is the most `jsonb` prints for it.
            assert!(v.to_string().len() * 3 / 2 <= JSONB_TEXT_LIMIT);
        }
    }

    #[test]
    fn a_column_set_lists_names_in_table_order_and_never_a_value() {
        let set = ColumnSet::of(&[Column::Action, Column::At]);
        assert_eq!(set.names(), ["at", "action"]);
        assert_eq!(set.to_string(), "at, action");
        assert_eq!(set.len(), 2);
        assert_eq!(ColumnSet::all().len(), Column::ALL.len());
        assert!(ColumnSet::empty().is_empty());
        // Each column has its own bit.
        let mut seen = ColumnSet::empty();
        for c in Column::ALL {
            assert!(!seen.contains(c));
            seen.insert(c);
        }
        assert_eq!(seen, ColumnSet::all());
    }
}
