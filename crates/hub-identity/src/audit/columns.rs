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
//! A difference is reported by the column's name only, never its value (S10).

use std::fmt;

use domain::ContentHash;
use serde_json::{Map, Value};
use sqlx::Row;
use sqlx::postgres::PgRow;

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

    pub fn insert(&mut self, c: Column) {
        self.0 |= c.bit();
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

/// The select list that reads the columns [`StoredColumns::from_row`] needs. `at` is read as whole microseconds, which is what a
/// `timestamptz` holds, so that a change of one microsecond shows.
pub const SELECT_LIST: &str = "(extract(epoch FROM at) * 1000000)::bigint AS at_micros, actor_kind, actor_ref, action, via, \
     swimlane_id, path, hash_before, hash_after, github_login, approval_id, request_id, attribution_source, \
     attribution_confidence, attribution_actor, attribution_evidence";

/// What one row of `audit_events` holds in the copied columns.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredColumns {
    /// Microseconds since the epoch.
    pub at_micros: i64,
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

impl StoredColumns {
    /// Read the columns named by [`SELECT_LIST`] from a row.
    ///
    /// # Errors
    /// A column is missing from the row or has another type than the table gives it.
    pub fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            at_micros: row.try_get("at_micros")?,
            actor_kind: row.try_get("actor_kind")?,
            actor_ref: row.try_get("actor_ref")?,
            action: row.try_get("action")?,
            via: row.try_get("via")?,
            swimlane_id: row.try_get("swimlane_id")?,
            path: row.try_get("path")?,
            hash_before: row.try_get("hash_before")?,
            hash_after: row.try_get("hash_after")?,
            github_login: row.try_get("github_login")?,
            approval_id: row.try_get("approval_id")?,
            request_id: row.try_get("request_id")?,
            attribution_source: row.try_get("attribution_source")?,
            attribution_confidence: row.try_get("attribution_confidence")?,
            attribution_actor: row.try_get("attribution_actor")?,
            attribution_evidence: row.try_get("attribution_evidence")?,
        })
    }
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
    check(Column::At, at == Some(stored.at_micros));
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
fn actor_columns(event: &Map<String, Value>, stored: &StoredColumns) -> (bool, bool) {
    let actor = event.get("actor").and_then(Value::as_object);
    let kind = actor.and_then(|a| a.get("kind")).and_then(Value::as_str);
    let field = match kind {
        Some("user") => Some("user"),
        Some("agent") => Some("swimlane"),
        Some("sentinel") => Some("name"),
        Some("system") => Some("component"),
        _ => None,
    };
    let reference = field
        .zip(actor)
        .and_then(|(field, a)| a.get(field))
        .and_then(Value::as_str);
    (
        kind == Some(stored.actor_kind.as_str()),
        reference.is_some() && reference == stored.actor_ref.as_deref(),
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
            Column::At => s.at_micros += 1,
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
