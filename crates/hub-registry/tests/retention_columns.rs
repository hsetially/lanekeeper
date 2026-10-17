//! Every column that can name a blob is decided (T9, D78, D116): the deleter has to read every column that holds a hash without a foreign
//! key, and a column that is added later must not be forgotten.
//!
//! `hub_registry::retention::plan::DECISIONS` says, for each column of the schema that can hold a hash (the domain `sha256_hash`, any
//! `bytea`, any `jsonb`, and any text column whose name says "hash"), what retention does about it. These tests hold that table to the live
//! catalog both ways, hold the columns it reads to the columns the SQL reads and to the classes the behavioural tests exercise, and prove
//! the check itself by adding and dropping a column in a scratch database.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use hub_registry::retention::plan::{DECISIONS, Decision, NOT_REFERENCED, read_columns};
use sqlx::PgConnection;
use support::retention::{Attach, Gc};

/// The columns of the public schema that can hold a hash or something that names one, as `table.column`. Partitions are left out (they
/// repeat their parent's columns).
const CATALOG_SQL: &str = "\
SELECT c.relname::text || '.' || a.attname::text \
FROM pg_attribute a \
JOIN pg_class c ON c.oid = a.attrelid \
JOIN pg_namespace n ON n.oid = c.relnamespace \
JOIN pg_type t ON t.oid = a.atttypid \
LEFT JOIN pg_type base ON base.oid = t.typbasetype \
LEFT JOIN pg_type elem ON elem.oid = t.typelem AND t.typcategory = 'A' \
WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
  AND a.attnum > 0 AND NOT a.attisdropped \
  AND (t.typname IN ('bytea', 'jsonb', 'sha256_hash') \
       OR base.typname IN ('bytea', 'jsonb') \
       OR elem.typname IN ('bytea', 'jsonb', 'sha256_hash') \
       OR (t.typname IN ('text', 'varchar', 'bpchar') AND a.attname ILIKE '%hash%')) \
ORDER BY 1";

async fn catalog(conn: &mut PgConnection) -> BTreeSet<String> {
    sqlx::query_scalar::<_, String>(CATALOG_SQL)
        .fetch_all(conn)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

fn decided() -> BTreeSet<String> {
    DECISIONS.iter().map(|(c, _)| (*c).to_owned()).collect()
}

/// What is wrong, as a list of sentences: a column the schema has and the table does not decide, and one the table decides and the schema
/// does not have (a renamed or dropped column leaves a decision that decides nothing).
fn disagreements(catalog: &BTreeSet<String>) -> Vec<String> {
    let decided = decided();
    let mut out: Vec<String> = catalog
        .difference(&decided)
        .map(|c| format!("{c} can hold a hash and retention has not decided what it means"))
        .collect();
    out.extend(
        decided
            .difference(catalog)
            .map(|c| format!("{c} is decided and the schema has no such column")),
    );
    out
}

#[tokio::test]
async fn every_column_that_can_hold_a_hash_is_decided() {
    let Some(gc) = Gc::new().await else { return };
    let mut admin = gc.rig.db.admin().await;
    let found = catalog(&mut admin).await;
    assert!(
        found.len() > 40,
        "the catalog query finds the columns of the schema, not nothing: {}",
        found.len()
    );
    assert_eq!(disagreements(&found), Vec::<String>::new());
    // The ones the brief names, so that a rename of a decision cannot hide one of them.
    for column in [
        "baselines.blob_hash",
        "file_state.nfs_hash",
        "pr_links.nfs_hash",
        "intentional_divergence.nfs_hash",
        "adoption_resolutions.nfs_hash",
    ] {
        assert!(found.contains(column), "{column} is in the schema");
        assert!(
            read_columns().contains(&column),
            "{column} has no foreign key and names a blob: retention reads it"
        );
    }
}

/// The check proves itself: a column added to the schema is reported as undecided, and one dropped is reported as decided without being
/// there. (The scratch database is this test's own.)
#[tokio::test]
async fn a_column_added_later_fails_the_check_until_it_is_decided() {
    let Some(gc) = Gc::new().await else { return };
    let mut admin = gc.rig.db.admin().await;

    // A new hash column on a table that exists.
    sqlx::raw_sql("ALTER TABLE file_state ADD COLUMN extra_hash sha256_hash")
        .execute(&mut admin)
        .await
        .unwrap();
    assert_eq!(
        disagreements(&catalog(&mut admin).await),
        vec!["file_state.extra_hash can hold a hash and retention has not decided what it means".to_owned()]
    );
    // A new table with a bytea column, a jsonb column, an array of hashes, and a text column that says hash.
    sqlx::raw_sql(
        "CREATE TABLE later_feature (id bigint PRIMARY KEY, raw bytea, doc jsonb, many sha256_hash[], content_hash text, note text)",
    )
    .execute(&mut admin)
    .await
    .unwrap();
    let after = disagreements(&catalog(&mut admin).await);
    for column in [
        "later_feature.raw",
        "later_feature.doc",
        "later_feature.many",
        "later_feature.content_hash",
    ] {
        assert!(
            after.iter().any(|d| d.starts_with(column)),
            "{column} is reported: {after:?}"
        );
    }
    assert!(
        !after.iter().any(|d| d.starts_with("later_feature.note")),
        "a plain text column is not one that can hold a hash: {after:?}"
    );
    // A decided column that goes away.
    sqlx::raw_sql("DROP TABLE later_feature; ALTER TABLE file_state DROP COLUMN extra_hash; ALTER TABLE pr_links DROP COLUMN nfs_hash CASCADE")
        .execute(&mut admin)
        .await
        .unwrap();
    assert_eq!(
        disagreements(&catalog(&mut admin).await),
        vec!["pr_links.nfs_hash is decided and the schema has no such column".to_owned()]
    );
}

#[test]
fn the_columns_that_are_read_are_the_columns_the_sql_reads() {
    // The statements name the column each probe reads in a comment, and the table decides which are probed: the two lists are one.
    let mut in_sql: Vec<&str> = NOT_REFERENCED
        .split("/* ref:")
        .skip(1)
        .filter_map(|part| part.find(" */").map(|end| &part[..end]))
        .collect();
    in_sql.sort_unstable();
    let mut decided_read = read_columns();
    decided_read.sort_unstable();
    assert_eq!(in_sql, decided_read);
    assert_eq!(
        in_sql.len(),
        in_sql.iter().collect::<BTreeSet<_>>().len(),
        "each column is probed once"
    );
}

/// Every column retention reads has a behavioural test: a class of `Attach` that puts a blob's hash in exactly that column, run by
/// `retention_gc.rs::every_reference_class_pins_and_every_look_alike_does_not`. A column added to `DECISIONS` as a reference without such a
/// class fails here, and so does a class that names a column the table does not decide.
#[test]
fn every_column_that_is_read_has_a_class_that_proves_it_and_every_look_alike_is_decided_as_not_reading() {
    let proven: BTreeSet<&str> = Attach::all()
        .into_iter()
        .filter(|a| a.pins())
        .map(Attach::column)
        .collect();
    let read: BTreeSet<&str> = read_columns().into_iter().collect();
    assert_eq!(
        proven, read,
        "the columns the tests prove and the columns retention reads"
    );

    let table: BTreeMap<&str, Decision> = DECISIONS.iter().copied().collect();
    for a in Attach::all().into_iter().filter(|a| !a.pins()) {
        let column = a.column();
        let decision = table.get(column).copied();
        assert!(decision.is_some(), "{a:?}: {column} is not decided");
        if a == Attach::StagedElsewhere {
            // The same column as `Staged`: the path lists of a staged part are not hashes of blobs, the entries are.
            assert_eq!(decision, Some(Decision::ReferenceJson("entries[].hash")));
        } else {
            assert!(
                !matches!(decision, Some(Decision::Reference | Decision::ReferenceJson(_))),
                "{a:?}: {column} is a look-alike and is decided as a reference"
            );
        }
    }
}

#[test]
fn every_decision_that_is_not_a_reference_says_why() {
    for (column, decision) in DECISIONS {
        match decision {
            Decision::NotABlob(why) | Decision::DecidedByKeepRules(why) => {
                assert!(
                    why.split_whitespace().count() >= 2,
                    "{column}: the reason is a phrase, not a word: {why:?}"
                );
            }
            Decision::ReferenceJson(path) => assert!(!path.is_empty(), "{column}: where in the document"),
            Decision::Reference | Decision::Subject | Decision::Cascades => {}
        }
    }
    // The two columns D116 is about: an audit event's hashes do not pin a blob.
    for column in ["audit_events.hash_before", "audit_events.hash_after"] {
        let found = DECISIONS.iter().find(|(c, _)| *c == column).map(|(_, d)| *d);
        assert!(
            matches!(found, Some(Decision::NotABlob(why)) if why.contains("D116")),
            "{column}: {found:?}"
        );
    }
}
