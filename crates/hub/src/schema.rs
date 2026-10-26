//! The schema probe (plan 18, T2, G22; S16): does the database hold the schema this hub was built for?
//!
//! The hub runs as `lanekeeper_app`, and `_sqlx_migrations` is invisible to that role (it holds no grant on it, on purpose), so
//! the hub cannot ask "which migrations are applied". It asks the catalog instead, which every role can read: for each
//! migration, one object that the migration creates and no earlier one does (a table, a column, a trigger). A hub started
//! on a database that lacks one refuses to start and names the first migration that is missing; `/readyz` runs the same probe,
//! so a database restored to an older state turns a hub unready instead of serving errors.
//!
//! The markers are tied to the migrations in two places: `tests/migrate.rs` fails if a migration has no marker (or a marker
//! has no migration), and it applies the migrations one at a time and checks that exactly the markers of the ones not yet
//! applied are missing, so a marker that an earlier migration already satisfies is caught.
//!
//! The probe cannot see a database that is *ahead* of the hub. That is safe by construction: migrations are additive (expand,
//! then contract), so a hub built for version N runs on N or later, and `hub migrate` refuses to run an older image's set
//! against a database a newer image already migrated.

use async_trait::async_trait;
use sqlx::PgPool;

use crate::migrate::EMBEDDED;
use crate::ops::ReadyCheck;

/// An object in the `public` schema that a migration creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Object {
    /// A table of this name.
    Table(&'static str),
    /// A column of this name in a table of this name.
    Column {
        table: &'static str,
        column: &'static str,
    },
    /// A user trigger of this name on a table of this name.
    Trigger { table: &'static str, name: &'static str },
}

/// What the probe looks for to know that migration `version` was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub version: i64,
    pub object: Object,
}

/// One marker for each migration in [`EMBEDDED`], in the same order. Each is something that migration creates and no earlier
/// one does; `tests/migrate.rs` proves that against a real database.
pub const MARKERS: [Marker; 8] = [
    Marker {
        version: 1,
        object: Object::Table("users"),
    },
    Marker {
        version: 2,
        object: Object::Table("audit_verify_state"),
    },
    Marker {
        version: 3,
        object: Object::Table("swimlane_folders"),
    },
    Marker {
        version: 4,
        object: Object::Trigger {
            table: "proposals",
            name: "proposals_author_immutable",
        },
    },
    Marker {
        version: 5,
        object: Object::Table("mcp_tokens"),
    },
    Marker {
        version: 6,
        object: Object::Trigger {
            table: "mcp_tokens",
            name: "mcp_tokens_created_not_future",
        },
    },
    Marker {
        version: 7,
        object: Object::Table("tool_write_registrations"),
    },
    Marker {
        version: 8,
        object: Object::Column {
            table: "blobs",
            column: "touched_at",
        },
    },
];

/// Why the schema is not usable. Fixed text and a migration's number and name; no SQL, no driver words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    #[error(
        "migration {version:04} ({description}) has not been applied; run `hub migrate` with the migrator credential first"
    )]
    Missing { version: i64, description: &'static str },
    #[error("the schema could not be checked")]
    Query,
}

/// The catalog query. One round trip for every marker: `kind` is `table`, `column` or `trigger`, `name` is the column or the
/// trigger and empty for a table. It reads `pg_catalog`, which needs no privilege on the tables themselves.
const PROBE: &str = "\
SELECT m.version \
FROM unnest($1::bigint[], $2::text[], $3::text[], $4::text[]) AS m(version, kind, relation, name) \
WHERE NOT EXISTS ( \
    SELECT 1 \
    FROM pg_catalog.pg_class c \
    JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace AND n.nspname = 'public' \
    WHERE c.relname = m.relation AND c.relkind IN ('r', 'p') \
      AND CASE m.kind \
            WHEN 'table' THEN TRUE \
            WHEN 'column' THEN EXISTS ( \
                SELECT 1 FROM pg_catalog.pg_attribute a \
                WHERE a.attrelid = c.oid AND a.attname = m.name AND a.attnum > 0 AND NOT a.attisdropped) \
            WHEN 'trigger' THEN EXISTS ( \
                SELECT 1 FROM pg_catalog.pg_trigger t \
                WHERE t.tgrelid = c.oid AND t.tgname = m.name AND NOT t.tgisinternal) \
            ELSE FALSE \
          END \
) \
ORDER BY m.version";

/// The versions whose marker is missing, ascending. Empty when the schema is complete.
pub async fn missing(pool: &PgPool) -> Result<Vec<i64>, SchemaError> {
    let mut versions = Vec::with_capacity(MARKERS.len());
    let mut kinds = Vec::with_capacity(MARKERS.len());
    let mut relations = Vec::with_capacity(MARKERS.len());
    let mut names = Vec::with_capacity(MARKERS.len());
    for marker in &MARKERS {
        let (kind, relation, name) = match marker.object {
            Object::Table(table) => ("table", table, ""),
            Object::Column { table, column } => ("column", table, column),
            Object::Trigger { table, name } => ("trigger", table, name),
        };
        versions.push(marker.version);
        kinds.push(kind);
        relations.push(relation);
        names.push(name);
    }
    sqlx::query_scalar::<_, i64>(PROBE)
        .bind(versions)
        .bind(kinds)
        .bind(relations)
        .bind(names)
        .fetch_all(pool)
        .await
        .map_err(|_| SchemaError::Query)
}

/// The schema is complete, or the first migration that is missing is named.
pub async fn verify(pool: &PgPool) -> Result<(), SchemaError> {
    match missing(pool).await?.first() {
        None => Ok(()),
        Some(&version) => Err(SchemaError::Missing {
            version,
            description: EMBEDDED
                .iter()
                .find(|m| m.version == version)
                .map_or("unknown", |m| m.description),
        }),
    }
}

/// Readiness: the schema is complete. One catalog query per probe, which is a few microseconds of work on a handful of rows.
#[derive(Debug, Clone)]
pub struct SchemaReady(PgPool);

impl SchemaReady {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self(pool)
    }
}

#[async_trait]
impl ReadyCheck for SchemaReady {
    async fn ready(&self) -> bool {
        verify(&self.0).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn there_is_one_marker_per_migration_in_order() {
        let markers: Vec<i64> = MARKERS.iter().map(|m| m.version).collect();
        let table: Vec<i64> = EMBEDDED.iter().map(|m| m.version).collect();
        assert_eq!(markers, table);
    }

    #[test]
    fn no_object_is_the_marker_of_two_migrations() {
        let objects: BTreeSet<String> = MARKERS.iter().map(|m| format!("{:?}", m.object)).collect();
        assert_eq!(objects.len(), MARKERS.len());
    }

    #[test]
    fn a_missing_migration_is_named_by_number_and_description_without_any_sql() {
        let shown = SchemaError::Missing {
            version: 4,
            description: "write additions",
        }
        .to_string();
        assert!(
            shown.contains("0004") && shown.contains("write additions"),
            "{shown}"
        );
        assert!(shown.contains("hub migrate"), "{shown}");
        assert!(!shown.to_ascii_lowercase().contains("select"), "{shown}");
    }

    #[test]
    fn the_probe_reads_the_catalog_and_never_the_migration_table() {
        // The app role cannot read `_sqlx_migrations`, so a probe that did would fail on exactly the database it is for.
        assert!(PROBE.contains("pg_catalog.pg_class"));
        assert!(!PROBE.contains("_sqlx_migrations"));
        // One statement, parameters only: nothing from a marker is spliced into SQL.
        assert_eq!(PROBE.matches(';').count(), 0);
        assert!(PROBE.contains("$1") && PROBE.contains("$4"));
    }
}
