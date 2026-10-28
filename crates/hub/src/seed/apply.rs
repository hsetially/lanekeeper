//! The database side of `hub seed` (plan 18, T7, G24; rule 8, S9, S10, P12): reading what the file speaks of, and writing the
//! changes of a [`Plan`] with their audit events in one transaction.
//!
//! **Every statement is a constant** in this file, with its values bound, so a test can ask Postgres for the plan of exactly the
//! statement the code runs (`tests/explain.rs`), and nothing is ever built from the file's text.
//!
//! **Reads are batched** (`= ANY($1)`: one round trip for all the ids of a section, by the primary key) and bounded by the number of
//! ids asked for, which the file's limits bound. The lists of rows "not in the file" are `ORDER BY` the key with a `LIMIT`.
//!
//! **A write is a column set to what the file says and no other column.** An update is one statement for all the columns of a
//! row, each `COALESCE($n, column)`: a column the change does not set is bound `NULL` and keeps its value, so a field the file does
//! not carry (and a column the seed never writes: the baseline, the editing lock, the times) is not in the statement's reach.

use sqlx::{Connection, PgConnection};

use super::error::SeedError;
use super::file::SeedFile;
use super::plan::{Absent, Change, Field, Kind, Op, REPORT_LIMIT, SentinelRow, State, SwimlaneRow};

/// The only role `hub seed` runs as: the hub's own. The migrator owns every table and the superuser owns the server, so a seed
/// that was given either credential would be writing with more than the rows it is meant to write; it is refused, as `hub migrate`
/// refuses everything but its own role.
pub const APP_ROLE: &str = "lanekeeper_app";

/// The advisory lock that makes two seeds at once take turns: the second reads what the first committed, and finds nothing to do.
/// A key space of its own (the audit chain's is `0x004C_4B41_5544_4955`). Taken before the audit chain's lock, which nothing else
/// takes before this one, so the two cannot deadlock.
pub const LOCK_KEY: i64 = 0x004C_4B53_4545_4400;

/// How long a seed waits for that lock, at most.
pub const LOCK_WAIT: &str = "15s";

/// The most tenant rows read for the swimlanes of a file: sixty-four tenants each for two hundred swimlanes.
const MAX_TENANT_ROWS: i64 = 12_800;

// ------------------------------------------------------------------------------------------------ the statements

pub const SELECT_PROJECTS: &str = "SELECT id, display_name FROM projects WHERE id = ANY($1)";
pub const SELECT_SWIMLANES: &str = "SELECT id, project_id, display_name, cluster, tier, nfs_server, nfs_export, mount_root, \
     agent_service_account FROM swimlanes WHERE id = ANY($1)";
pub const SELECT_TENANTS: &str = "SELECT swimlane_id, tenant_id FROM swimlane_tenants WHERE swimlane_id = ANY($1) \
     ORDER BY swimlane_id, tenant_id LIMIT $2";
pub const SELECT_SENTINELS: &str =
    "SELECT vm, swimlane_id, export_root, service_account FROM sentinels WHERE vm = ANY($1)";
pub const ABSENT_PROJECTS: &str = "SELECT id FROM projects WHERE id <> ALL($1) ORDER BY id LIMIT $2";
pub const ABSENT_SWIMLANES: &str = "SELECT id FROM swimlanes WHERE id <> ALL($1) ORDER BY id LIMIT $2";
pub const ABSENT_SENTINELS: &str = "SELECT vm FROM sentinels WHERE vm <> ALL($1) ORDER BY vm LIMIT $2";

pub const INSERT_PROJECT: &str = "INSERT INTO projects (id, display_name) VALUES ($1, $2)";
pub const UPDATE_PROJECT: &str = "UPDATE projects SET display_name = $2 WHERE id = $1";
pub const INSERT_SWIMLANE: &str = "INSERT INTO swimlanes (id, project_id, display_name, cluster, tier, nfs_server, nfs_export, \
     mount_root, agent_service_account) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)";
pub const UPDATE_SWIMLANE: &str = "UPDATE swimlanes SET project_id = COALESCE($2, project_id), display_name = COALESCE($3, display_name), \
     cluster = COALESCE($4, cluster), tier = COALESCE($5, tier), nfs_server = COALESCE($6, nfs_server), \
     nfs_export = COALESCE($7, nfs_export), mount_root = COALESCE($8, mount_root), \
     agent_service_account = COALESCE($9, agent_service_account) WHERE id = $1";
pub const INSERT_TENANTS: &str = "INSERT INTO swimlane_tenants (swimlane_id, tenant_id) \
     SELECT $1, tenant FROM unnest($2::text[]) AS tenant ON CONFLICT DO NOTHING";
pub const INSERT_SENTINEL: &str =
    "INSERT INTO sentinels (vm, swimlane_id, export_root, service_account) VALUES ($1, $2, $3, $4)";
pub const UPDATE_SENTINEL: &str = "UPDATE sentinels SET swimlane_id = COALESCE($2, swimlane_id), \
     export_root = COALESCE($3, export_root), service_account = COALESCE($4, service_account) WHERE vm = $1";

/// The SQLSTATE of a database error: five characters of `0-9A-Z`, or `?????`. Nothing else of the error is ever shown.
fn sqlstate(error: &sqlx::Error) -> String {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .filter(|code| code.len() == 5 && code.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()))
        .map_or_else(|| "?????".to_owned(), std::borrow::Cow::into_owned)
}

pub(super) fn database(error: &sqlx::Error) -> SeedError {
    SeedError::Database(sqlstate(error))
}

// ------------------------------------------------------------------------------------------------ reading

/// The ids a file speaks of, per table.
struct Wanted {
    projects: Vec<String>,
    swimlanes: Vec<String>,
    tenant_swimlanes: Vec<String>,
    sentinels: Vec<String>,
}

fn wanted(file: &SeedFile) -> Wanted {
    let mut projects: Vec<String> = file.projects.iter().flatten().map(|p| p.id.clone()).collect();
    let mut swimlanes: Vec<String> = file
        .swimlanes
        .iter()
        .flatten()
        .map(|s| s.id.as_str().to_owned())
        .collect();
    projects.extend(file.swimlanes.iter().flatten().filter_map(|s| s.project.clone()));
    swimlanes.extend(
        file.sentinels
            .iter()
            .flatten()
            .filter_map(|s| s.swimlane.as_ref().map(|id| id.as_str().to_owned())),
    );
    projects.sort();
    projects.dedup();
    swimlanes.sort();
    swimlanes.dedup();
    Wanted {
        projects,
        swimlanes,
        tenant_swimlanes: file
            .swimlanes
            .iter()
            .flatten()
            .filter(|s| s.tenants.is_some())
            .map(|s| s.id.as_str().to_owned())
            .collect(),
        sentinels: file.sentinels.iter().flatten().map(|s| s.vm.clone()).collect(),
    }
}

type SwimlaneColumns = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What the database holds that `file` speaks of. Reads only; the caller's transaction decides whether anything else happens.
pub async fn load_state(conn: &mut PgConnection, file: &SeedFile) -> Result<State, SeedError> {
    let want = wanted(file);
    let mut state = State::default();

    if !want.projects.is_empty() {
        let rows: Vec<(String, String)> = sqlx::query_as(SELECT_PROJECTS)
            .bind(&want.projects)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| database(&e))?;
        state.projects = rows.into_iter().collect();
    }
    if !want.swimlanes.is_empty() {
        let rows: Vec<SwimlaneColumns> = sqlx::query_as(SELECT_SWIMLANES)
            .bind(&want.swimlanes)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| database(&e))?;
        for (id, project, display_name, cluster, tier, nfs_server, nfs_export, mount_root, account) in rows {
            state.swimlanes.insert(
                id,
                SwimlaneRow {
                    project,
                    display_name,
                    cluster,
                    tier,
                    nfs_server,
                    nfs_export,
                    mount_root,
                    agent_service_account: account,
                },
            );
        }
    }
    if !want.tenant_swimlanes.is_empty() {
        let rows: Vec<(String, String)> = sqlx::query_as(SELECT_TENANTS)
            .bind(&want.tenant_swimlanes)
            .bind(MAX_TENANT_ROWS + 1)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| database(&e))?;
        if i64::try_from(rows.len()).map_or(true, |n| n > MAX_TENANT_ROWS) {
            // More tenants than any real deployment has: not a state to compare against.
            return Err(SeedError::Database("54000".to_owned()));
        }
        for (swimlane, tenant) in rows {
            state.tenants.entry(swimlane).or_default().insert(tenant);
        }
    }
    if !want.sentinels.is_empty() {
        let rows: Vec<(String, Option<String>, String, Option<String>)> = sqlx::query_as(SELECT_SENTINELS)
            .bind(&want.sentinels)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| database(&e))?;
        for (vm, swimlane, export_root, service_account) in rows {
            state.sentinels.insert(
                vm,
                SentinelRow {
                    swimlane,
                    export_root,
                    service_account,
                },
            );
        }
    }
    state.absent = load_absent(conn, file).await?;
    Ok(state)
}

/// The first [`REPORT_LIMIT`] keys of `sql`'s table that are not in `listed`, in key order, and whether there were more.
async fn absent_keys(
    conn: &mut PgConnection,
    sql: &str,
    listed: &[String],
) -> Result<(Vec<String>, bool), SeedError> {
    let limit = i64::try_from(REPORT_LIMIT).unwrap_or(50) + 1;
    let mut names: Vec<String> = sqlx::query_scalar(sql)
        .bind(listed)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| database(&e))?;
    let more = names.len() > REPORT_LIMIT;
    names.truncate(REPORT_LIMIT);
    Ok((names, more))
}

/// The rows the database has in the sections the file carries and the file does not list: the first [`REPORT_LIMIT`] of each, by key.
async fn load_absent(conn: &mut PgConnection, file: &SeedFile) -> Result<Absent, SeedError> {
    let mut absent = Absent::default();
    if let Some(projects) = &file.projects {
        let listed: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
        let (names, more) = absent_keys(conn, ABSENT_PROJECTS, &listed).await?;
        absent.projects = names;
        absent.truncated |= more;
    }
    if let Some(swimlanes) = &file.swimlanes {
        let listed: Vec<String> = swimlanes.iter().map(|s| s.id.as_str().to_owned()).collect();
        let (names, more) = absent_keys(conn, ABSENT_SWIMLANES, &listed).await?;
        absent.swimlanes = names;
        absent.truncated |= more;
    }
    if let Some(sentinels) = &file.sentinels {
        let listed: Vec<String> = sentinels.iter().map(|s| s.vm.clone()).collect();
        let (names, more) = absent_keys(conn, ABSENT_SENTINELS, &listed).await?;
        absent.sentinels = names;
        absent.truncated |= more;
    }
    Ok(absent)
}

// ------------------------------------------------------------------------------------------------ writing

/// The value a change sets for `field`, as the bind for a `COALESCE`d column: `None` leaves the column alone.
fn set(change: &Change, field: Field) -> Option<&str> {
    change.value(field)
}

/// Write one change. Exactly one row is affected (a tenant set: as many as the tenants added); anything else means a row moved under
/// the seed, and the whole run is rolled back.
pub async fn write(conn: &mut PgConnection, change: &Change) -> Result<(), SeedError> {
    let expected = match (change.kind, change.op) {
        (Kind::Tenants, _) => u64::try_from(change.tenants.len()).unwrap_or(u64::MAX),
        _ => 1,
    };
    let done = match (change.kind, change.op) {
        (Kind::Project, Op::Create) => sqlx::query(INSERT_PROJECT)
            .bind(&change.key)
            .bind(set(change, Field::DisplayName)),
        (Kind::Project, _) => sqlx::query(UPDATE_PROJECT)
            .bind(&change.key)
            .bind(set(change, Field::DisplayName)),
        (Kind::Swimlane, Op::Create) => sqlx::query(INSERT_SWIMLANE)
            .bind(&change.key)
            .bind(set(change, Field::Project))
            .bind(set(change, Field::DisplayName))
            .bind(set(change, Field::Cluster))
            .bind(set(change, Field::Tier))
            .bind(set(change, Field::NfsServer))
            .bind(set(change, Field::NfsExport))
            .bind(set(change, Field::MountRoot))
            .bind(set(change, Field::AgentServiceAccount)),
        (Kind::Swimlane, _) => sqlx::query(UPDATE_SWIMLANE)
            .bind(&change.key)
            .bind(set(change, Field::Project))
            .bind(set(change, Field::DisplayName))
            .bind(set(change, Field::Cluster))
            .bind(set(change, Field::Tier))
            .bind(set(change, Field::NfsServer))
            .bind(set(change, Field::NfsExport))
            .bind(set(change, Field::MountRoot))
            .bind(set(change, Field::AgentServiceAccount)),
        (Kind::Tenants, _) => sqlx::query(INSERT_TENANTS)
            .bind(&change.key)
            .bind(&change.tenants),
        (Kind::Sentinel, Op::Create) => sqlx::query(INSERT_SENTINEL)
            .bind(&change.key)
            .bind(set(change, Field::Swimlane))
            .bind(set(change, Field::ExportRoot))
            .bind(set(change, Field::ServiceAccount)),
        (Kind::Sentinel, _) => sqlx::query(UPDATE_SENTINEL)
            .bind(&change.key)
            .bind(set(change, Field::Swimlane))
            .bind(set(change, Field::ExportRoot))
            .bind(set(change, Field::ServiceAccount)),
    }
    .execute(&mut *conn)
    .await
    .map_err(|e| database(&e))?;
    if done.rows_affected() == expected {
        Ok(())
    } else {
        Err(SeedError::Raced)
    }
}

/// `SELECT current_user`: the role the session really has, which a pooler or a default role could make other than the URL's.
pub async fn current_role(conn: &mut PgConnection) -> Result<String, SeedError> {
    sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(conn)
        .await
        .map_err(|e| database(&e))
}

/// Begin the transaction of a run. A dry run's is read-only, so that the server itself refuses a write; a real run's first takes the
/// seed lock, so that two runs at once take turns.
pub async fn begin(
    conn: &mut PgConnection,
    read_only: bool,
) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, SeedError> {
    let mut tx = conn.begin().await.map_err(|e| database(&e))?;
    if read_only {
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(|e| database(&e))?;
    } else {
        // Both statements are ours: a constant and a constant. `SET LOCAL` lasts to the end of this transaction.
        sqlx::query(&format!("SET LOCAL lock_timeout = '{LOCK_WAIT}'"))
            .execute(&mut *tx)
            .await
            .map_err(|e| database(&e))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(|e| database(&e))?;
    }
    Ok(tx)
}
