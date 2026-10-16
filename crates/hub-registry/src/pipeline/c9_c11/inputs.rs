//! What the structure checks read (T10, A45, P12): the statements, as constants and functions so that the plan snapshots of `tests/explain.rs`
//! read exactly what runs, and the rows they return, as typed values.
//!
//! Every statement is bounded (`LIMIT`), keyed by the swimlane, and answered from an index (the primary key of `file_state`, `swimlane_folders`,
//! `file_observations_history_idx`, the unique key of `services`). Nothing here reads a file's content, a setting value or an environment
//! *value*: the rows are paths, hashes, classes and times, and the names of environment variables (D88); a service's names are read only
//! by the services a file's readers are, and only when a result has to be evaluated.

use domain::{
    AppName, ContentHash, DriftState, FileClass, FileKind, FileRole, LogicalFile, NfsPath, SwimlaneId,
};
use engine::MAX_LANE_FILES;
use sqlx::Row;

use crate::db::{Db, ms, ts};
use crate::error::RegistryError;
use crate::read::rows;

/// The most folders, services and mappings read for one swimlane. All are small; the bounds are guards (rule 5).
pub(crate) const MAX_FOLDERS: i64 = 100_000;
pub(crate) const MAX_SERVICES: i64 = 100_000;

/// The swimlanes that have an inventory, with what a pass over them compares to decide whether the checks have to be computed: the version
/// the pipeline stamped, the config-server's start, and the fingerprint of the rules the state was computed under (the tenants, the
/// application names, the host rules and the severity rules are in it). `$1` the swimlanes (`NULL`: all of them), `$2` the limit.
pub fn gates_sql() -> String {
    format!(
        "SELECT r.swimlane_id, r.version, {started} AS started, r.rules_fingerprint::bytea AS rules \
         FROM swimlane_registry r JOIN swimlanes s ON s.id = r.swimlane_id \
         WHERE r.first_inventory_at IS NOT NULL AND ($1::text[] IS NULL OR r.swimlane_id = ANY($1)) \
         ORDER BY r.swimlane_id LIMIT $2",
        started = ms("s.config_server_started_at"),
    )
}

/// The files on NFS of a swimlane, in path order: what C9 reads (the names) and C10 (the resources and the property sources). `$1` the
/// swimlane, `$2` the limit (one more than the engine takes, so that a swimlane over the bound is seen as such).
pub const FILES_SQL: &str = "\
SELECT path, logical_file, kind, class, state, role, nfs_hash::bytea AS blob, denied, COALESCE(size, 0) AS size \
FROM file_state WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL ORDER BY path LIMIT $2";

/// The folders of a swimlane's config tree and when each was first seen (C11). `$1` the swimlane, `$2` the limit.
pub fn folders_sql() -> String {
    format!(
        "SELECT dir_path, {seen} AS seen, initial FROM swimlane_folders WHERE swimlane_id = $1 ORDER BY dir_path LIMIT $2",
        seen = ms("first_seen_at"),
    )
}

/// The Deployments of a swimlane that exist, with the application each runs (`SPRING_APPLICATION_NAME`, one of the allowlisted values, D88)
/// and a digest of the names of their environment variables, computed by the database so that the names themselves are not transferred.
/// `$1` the swimlane, `$2` the limit.
pub const SERVICES_SQL: &str = "\
SELECT id, namespace, name, env_values ->> 'SPRING_APPLICATION_NAME' AS app, \
       md5(COALESCE(array_to_string(env_names, ','), '') || '|' \
           || COALESCE((SELECT string_agg(k, ',' ORDER BY k) FROM jsonb_object_keys(env_values) AS k), '')) AS names \
FROM services WHERE swimlane_id = $1 AND removed_at IS NULL ORDER BY id LIMIT $2";

/// The names of the environment variables of some Deployments: the names in `env_names` and the keys of the allowlisted values (never a
/// value). `$1` the swimlane, `$2` the services, `$3` the limit.
pub const ENV_NAMES_SQL: &str = "\
SELECT id, env_names, ARRAY(SELECT jsonb_object_keys(env_values)) AS keys \
FROM services WHERE swimlane_id = $1 AND id = ANY($2::bigint[]) AND removed_at IS NULL ORDER BY id LIMIT $3";

/// The newest observation of a file: when, and whether it was part of the first inventory (A11). `$1` the swimlane, `$2` the path.
pub fn newest_observation_sql() -> String {
    format!(
        "SELECT {at} AS at, initial FROM file_observations WHERE swimlane_id = $1 AND path = $2 \
         ORDER BY observed_at DESC, id DESC LIMIT 1",
        at = ms("observed_at"),
    )
}

/// The version of a file that was current at a time: the newest observation at or before it. `hash` is `NULL` when that observation was a
/// deletion. `$1` the swimlane, `$2` the path, `$3` the time (Unix milliseconds).
pub fn observation_at_sql() -> String {
    format!(
        "SELECT hash::bytea AS hash FROM file_observations WHERE swimlane_id = $1 AND path = $2 AND observed_at <= {bound} \
         ORDER BY observed_at DESC, id DESC LIMIT 1",
        bound = ts("$3"),
    )
}

/// The first observation of a file, and whether it was part of the first inventory. `$1` the swimlane, `$2` the path.
pub const FIRST_OBSERVATION_SQL: &str = "\
SELECT hash::bytea AS hash, initial FROM file_observations WHERE swimlane_id = $1 AND path = $2 ORDER BY observed_at, id LIMIT 1";

/// A file of the swimlane, as the checks read it.
#[derive(Debug, Clone)]
pub(crate) struct FileRow {
    pub path: NfsPath,
    pub logical: LogicalFile,
    pub kind: FileKind,
    pub class: FileClass,
    pub state: DriftState,
    pub role: FileRole,
    pub blob: ContentHash,
    /// The stored flag; whether the file is denied is [`crate::denied::is_denied`].
    pub denied: bool,
    pub size: u64,
}

/// A Deployment, as C10 needs it: who it is, which application it runs, and a digest of the names of its environment.
#[derive(Debug, Clone)]
pub(crate) struct ServiceRow {
    pub id: i64,
    pub namespace: String,
    pub name: String,
    pub app: Option<AppName>,
    pub names: String,
}

/// The gate row of a swimlane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Gate {
    pub swimlane: SwimlaneId,
    pub version: i64,
    pub started: Option<i64>,
    /// `None` until the pipeline has computed the swimlane for the first time: there is nothing to check yet.
    pub rules: Option<Vec<u8>>,
}

impl Gate {
    /// The part of the row that says whether anything the checks read has moved.
    pub(crate) fn key(&self) -> (i64, Option<i64>, Option<&[u8]>) {
        (self.version, self.started, self.rules.as_deref())
    }

    pub(crate) fn ready(&self) -> bool {
        self.rules.is_some()
    }
}

/// The gates of `swimlanes` (all of them with an inventory when `None`), in swimlane order, at most `limit`.
pub(crate) async fn gates(
    db: &Db,
    swimlanes: Option<&[SwimlaneId]>,
    limit: usize,
) -> Result<Vec<Gate>, RegistryError> {
    let ids: Option<Vec<&str>> = swimlanes.map(|s| s.iter().map(SwimlaneId::as_str).collect());
    let fetched = db
        .run(
            "structure_gates",
            sqlx::query(&gates_sql())
                .bind(ids)
                .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
                .fetch_all(db.pool()),
        )
        .await?;
    fetched
        .iter()
        .map(|r| {
            let id: String = r.get("swimlane_id");
            Ok(Gate {
                swimlane: SwimlaneId::parse(&id).map_err(|_| RegistryError::Corrupt)?,
                version: r.get("version"),
                started: r.get("started"),
                rules: r.get("rules"),
            })
        })
        .collect()
}

/// The files of a swimlane on NFS. A swimlane over the engine's bound is not computed (the engine would refuse it too).
pub(crate) async fn files(db: &Db, swimlane: &SwimlaneId) -> Result<Vec<FileRow>, RegistryError> {
    let limit = i64::try_from(MAX_LANE_FILES.saturating_add(1)).unwrap_or(i64::MAX);
    let fetched = db
        .run(
            "structure_files",
            sqlx::query(FILES_SQL)
                .bind(swimlane.as_str())
                .bind(limit)
                .fetch_all(db.pool()),
        )
        .await?;
    if fetched.len() > MAX_LANE_FILES {
        return Err(RegistryError::TooLarge);
    }
    let mut out = Vec::with_capacity(fetched.len());
    for r in &fetched {
        let path: String = r.get("path");
        let logical: String = r.get("logical_file");
        let kind: String = r.get("kind");
        let class: String = r.get("class");
        let state: String = r.get("state");
        let role: Option<String> = r.get("role");
        let blob: Vec<u8> = r.get("blob");
        let size: i64 = r.get("size");
        out.push(FileRow {
            path: NfsPath::parse(&path).map_err(|_| RegistryError::Corrupt)?,
            logical: LogicalFile::parse(&logical).map_err(|_| RegistryError::Corrupt)?,
            kind: rows::kind(&kind)?,
            class: rows::class(&class)?,
            state: rows::state(&state)?,
            role: rows::role(role.as_deref())?,
            blob: rows::hash(&blob)?,
            denied: r.get("denied"),
            size: u64::try_from(size).unwrap_or(0),
        });
    }
    in_engine_order(&mut out);
    Ok(out)
}

/// The order the engine takes (`NfsPath`'s, bytewise): the database's collation may order paths differently (under `en_US.UTF-8` a hyphen
/// is ignored), and the engine's checks report in the order they are given, so the result of a check must not depend on a setting of the
/// database.
pub(crate) fn in_engine_order(rows: &mut [FileRow]) {
    rows.sort_by(|a, b| a.path.cmp(&b.path));
}

/// The folders of a swimlane with the time each was first seen (`None` for a folder of the first inventory: C11 takes it as seen at the
/// epoch, A11).
pub(crate) async fn folders(
    db: &Db,
    swimlane: &SwimlaneId,
) -> Result<Vec<(NfsPath, Option<i64>)>, RegistryError> {
    let fetched = db
        .run(
            "structure_folders",
            sqlx::query(&folders_sql())
                .bind(swimlane.as_str())
                .bind(MAX_FOLDERS)
                .fetch_all(db.pool()),
        )
        .await?;
    fetched
        .iter()
        .map(|r| {
            let dir: String = r.get("dir_path");
            let initial: bool = r.get("initial");
            let seen: i64 = r.get("seen");
            Ok((
                NfsPath::parse(&dir).map_err(|_| RegistryError::Corrupt)?,
                (!initial).then_some(seen),
            ))
        })
        .collect()
}

/// The Deployments of a swimlane that exist.
pub(crate) async fn services(db: &Db, swimlane: &SwimlaneId) -> Result<Vec<ServiceRow>, RegistryError> {
    let fetched = db
        .run(
            "structure_services",
            sqlx::query(SERVICES_SQL)
                .bind(swimlane.as_str())
                .bind(MAX_SERVICES)
                .fetch_all(db.pool()),
        )
        .await?;
    Ok(fetched
        .iter()
        .map(|r| {
            let app: Option<String> = r.get("app");
            ServiceRow {
                id: r.get("id"),
                namespace: r.get("namespace"),
                name: r.get("name"),
                app: app.and_then(|a| AppName::parse(&a).ok()),
                names: r.get("names"),
            }
        })
        .collect())
}

/// The names of the environment variables of these Deployments (names only), one list for each, in service order.
pub(crate) async fn env_names(
    db: &Db,
    swimlane: &SwimlaneId,
    services: &[i64],
) -> Result<Vec<Vec<String>>, RegistryError> {
    if services.is_empty() {
        return Ok(Vec::new());
    }
    let fetched = db
        .run(
            "structure_env_names",
            sqlx::query(ENV_NAMES_SQL)
                .bind(swimlane.as_str())
                .bind(services)
                .bind(MAX_SERVICES)
                .fetch_all(db.pool()),
        )
        .await?;
    Ok(fetched
        .iter()
        .map(|r| {
            let mut names: Vec<String> = r.get("env_names");
            names.extend(r.get::<Vec<String>, _>("keys"));
            names
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use domain::FileRole;

    use super::*;

    fn row(path: &str) -> FileRow {
        let p = NfsPath::parse(path).unwrap();
        FileRow {
            logical: LogicalFile::from_nfs(p.clone()).unwrap(),
            path: p,
            kind: FileKind::Base,
            class: FileClass::Text,
            state: DriftState::InSync,
            role: FileRole::Resource,
            blob: ContentHash::from_bytes([1; 32]),
            denied: false,
            size: 1,
        }
    }

    #[test]
    fn the_rows_come_in_the_engines_byte_order_whatever_order_the_database_gave_them() {
        // `a-b` before `a/b` and after `a.b` in bytes (`-` 0x2d, `.` 0x2e, `/` 0x2f), which a collation that ignores punctuation would not.
        let mut rows = vec![row("a/b.yml"), row("a-b.yml"), row("a.b.yml"), row("A.yml")];
        in_engine_order(&mut rows);
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["A.yml", "a-b.yml", "a.b.yml", "a/b.yml"]);
    }
}
