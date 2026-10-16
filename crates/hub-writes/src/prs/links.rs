//! The `pr_links` table (T9, D77): every pull request is linked to the swimlane, the path and the NFS hash it came from.
//!
//! - **The one-open rule.** A second pull request for a change that already has an open one is blocked: [`find_open`] answers
//!   with the link, and the caller answers 409 with it. The unique index `pr_links_one_open` is the hard guard under it.
//! - **State.** A link is created `open`. The `pull_request` webhook moves it (`hub-platform`, prompt 03b), writes its audit event
//!   and publishes `PrStateChanged`. This module only reads the state back, and reacts to the event ([`PrStateWatcher`]).
//! - **"Fix merged; awaiting sync".** A merged pull request means the change is in Git, and the file on NFS still differs from
//!   Git until the config is synced (D19, D20). That label has no column: whoever shows the drift derives it from the link's
//!   state at read time (plan A17). What this module adds is the nudge: when a link changes state, `DriftChanged` is published
//!   for its path, so the screens that show the drift fetch it again and see the label.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use domain::{
    ContentHash, Cursor, DomainEvent, Guid, NfsPath, Page, Paged, PrLinkId, PrState, SwimlaneId, Timestamp,
    UserId,
};
use futures::StreamExt;
use futures::stream::BoxStream;
use ports::{EventFilter, EventKind};
use serde::Serialize;
use sqlx::{PgConnection, Postgres, Row, Transaction};

use crate::db::{ms, ts};
use crate::error::{Internal, Res};
use crate::service::Inner;

/// The most links one page holds.
pub const MAX_PAGE: u32 = 100;

/// A link as the API shows it (the `PrLink` schema of `api/openapi.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrLinkView {
    pub id: i64,
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub nfs_hash: ContentHash,
    pub state: PrState,
    pub pr_url: String,
    pub pr_number: u32,
    pub author: UserId,
    pub created_at: Timestamp,
    /// When the pull request was merged or closed (the time of the last change of state); absent while it is open.
    pub closed_at: Option<Timestamp>,
}

/// What a list of links is asked for.
#[derive(Debug, Clone)]
pub struct LinkQuery {
    pub swimlane: Option<SwimlaneId>,
    pub state: Option<PrState>,
    pub page: Page,
}

/// A link as the table holds it.
#[derive(Debug, Clone)]
pub(crate) struct LinkRow {
    pub id: PrLinkId,
    pub swimlane: SwimlaneId,
    pub path: NfsPath,
    pub nfs_hash: ContentHash,
    pub state: PrState,
    pub url: String,
    pub number: u32,
    pub author: UserId,
    pub created_ms: i64,
    pub updated_ms: i64,
}

impl LinkRow {
    pub fn view(&self) -> PrLinkView {
        PrLinkView {
            id: self.id.get(),
            swimlane: self.swimlane.clone(),
            path: self.path.clone(),
            nfs_hash: self.nfs_hash,
            state: self.state,
            pr_url: self.url.clone(),
            pr_number: self.number,
            author: self.author.clone(),
            created_at: Timestamp::from_unix_millis(self.created_ms),
            closed_at: (self.state != PrState::Open).then(|| Timestamp::from_unix_millis(self.updated_ms)),
        }
    }
}

macro_rules! select_link {
    () => {
        concat!(
            "SELECT l.id, l.swimlane_id, l.path, l.nfs_hash, l.state, l.url, l.pr_number, l.author_tid, l.author_oid, ",
            ms!("l.created_at"),
            " AS created_ms, ",
            ms!("l.updated_at"),
            " AS updated_ms FROM pr_links l"
        )
    };
}

/// The open link of any of the (path, hash) pairs `$2` and `$3` in swimlane `$1`, if there is one. The pairs are joined to the
/// partial unique index `pr_links_one_open`, which is made for it.
pub(crate) const OPEN_SQL: &str = concat!(
    select_link!(),
    " JOIN unnest($2::text[], $3::bytea[]) AS w(path, hash) ON w.path = l.path AND w.hash = l.nfs_hash \
     WHERE l.swimlane_id = $1 AND l.state = 'open' ORDER BY l.id LIMIT 1"
);

/// A page of links, newest first (keyset on the id): `$1` the id to start below (or null), `$2` the swimlane (or null), `$3` the
/// state (or null), `$4` the most rows.
pub(crate) const LIST_SQL: &str = concat!(
    select_link!(),
    " WHERE ($1::bigint IS NULL OR l.id < $1) AND ($2::text IS NULL OR l.swimlane_id = $2) \
     AND ($3::text IS NULL OR l.state = $3) ORDER BY l.id DESC LIMIT $4"
);

/// The swimlane and path of a link, for the watcher, and the version of the row: the microsecond the webhook's transaction stamped on
/// it (`updated_at = now()`), which differs for every change of the link.
pub(crate) const PLACE_SQL: &str = "SELECT swimlane_id, path, (floor(extract(epoch FROM updated_at) * 1000000))::bigint AS version \
     FROM pr_links WHERE id = $1";

const INSERT_SQL: &str = concat!(
    "INSERT INTO pr_links (swimlane_id, path, nfs_hash, repo, branch, pr_number, state, job_id, author_tid, author_oid, \
     github_login, url, created_at, updated_at) \
     SELECT $1, w.path, w.hash, $2, $3, $4, 'open', $5, $6, $7, $8, $9, ",
    ts!("10"),
    ", ",
    ts!("10"),
    " FROM unnest($11::text[], $12::bytea[]) AS w(path, hash)"
);

/// A link keeps the NFS version it came from for as long as it lives (D78): `$1` the swimlane, `$2` and `$3` the (path, hash) pairs
/// just linked, `$4` the job. Only versions the blob store has get a reference. Read through `pr_links_one_open`.
const KEEP_VERSIONS: &str = "INSERT INTO blob_refs (hash, ref_kind, ref_id) \
     SELECT l.nfs_hash, 'pr_link', l.id::text FROM pr_links l \
     JOIN unnest($2::text[], $3::bytea[]) AS w(path, hash) ON w.path = l.path AND w.hash = l.nfs_hash \
     JOIN blobs b ON b.hash = l.nfs_hash \
     WHERE l.swimlane_id = $1 AND l.state = 'open' AND l.job_id = $4 ON CONFLICT DO NOTHING";

fn user_of(tid: &str, oid: &str) -> Option<UserId> {
    Some(UserId::new(Guid::parse(tid).ok()?, Guid::parse(oid).ok()?))
}

fn read(row: &sqlx::postgres::PgRow) -> Option<LinkRow> {
    let id: i64 = row.try_get("id").ok()?;
    let hash: Vec<u8> = row.try_get("nfs_hash").ok()?;
    let state: String = row.try_get("state").ok()?;
    let number: i32 = row.try_get("pr_number").ok()?;
    Some(LinkRow {
        id: PrLinkId::from(id),
        swimlane: SwimlaneId::parse(&row.try_get::<String, _>("swimlane_id").ok()?).ok()?,
        path: NfsPath::parse(&row.try_get::<String, _>("path").ok()?).ok()?,
        nfs_hash: ContentHash::from_bytes(hash.try_into().ok()?),
        state: PrState::from_str(&state).ok()?,
        // A link made by this code always has an address and an author; one without is not shown.
        url: row.try_get::<Option<String>, _>("url").ok()??,
        number: u32::try_from(number).ok()?,
        author: user_of(
            &row.try_get::<Option<String>, _>("author_tid").ok()??,
            &row.try_get::<Option<String>, _>("author_oid").ok()??,
        )?,
        created_ms: row.try_get("created_ms").ok()?,
        updated_ms: row.try_get("updated_ms").ok()?,
    })
}

/// The open link of any of these (path, hash) pairs, if there is one (D77: a second pull request for the change is blocked).
pub(crate) async fn find_open(
    conn: &mut PgConnection,
    swimlane: &SwimlaneId,
    pairs: &[(String, ContentHash)],
) -> Res<Option<LinkRow>> {
    let paths: Vec<&str> = pairs.iter().map(|(p, _)| p.as_str()).collect();
    let hashes: Vec<Vec<u8>> = pairs.iter().map(|(_, h)| h.as_bytes().to_vec()).collect();
    let row = sqlx::query(OPEN_SQL)
        .bind(swimlane.as_str())
        .bind(&paths)
        .bind(&hashes)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_link_open", &e))?;
    row.map(|r| read(&r).ok_or_else(|| Internal::new("pr_link_open", "row")))
        .transpose()
}

/// The links to make for one pull request.
pub(crate) struct NewLinks<'a> {
    pub swimlane: &'a SwimlaneId,
    /// `base` or `tenant`.
    pub repo: &'static str,
    /// The branch the pull request was made from.
    pub branch: &'a str,
    pub number: u32,
    pub job: domain::PrJobId,
    pub author: &'a UserId,
    pub login: Option<&'a str>,
    pub url: &'a str,
    pub files: Vec<(String, ContentHash)>,
    pub now_ms: i64,
}

/// Make the links, one for each file. A unique violation (`pr_links_one_open`) is the caller's to tell apart.
pub(crate) async fn insert_all(
    tx: &mut Transaction<'_, Postgres>,
    n: &NewLinks<'_>,
) -> Result<u64, sqlx::Error> {
    let paths: Vec<&str> = n.files.iter().map(|(p, _)| p.as_str()).collect();
    let hashes: Vec<Vec<u8>> = n.files.iter().map(|(_, h)| h.as_bytes().to_vec()).collect();
    let done = sqlx::query(INSERT_SQL)
        .bind(n.swimlane.as_str())
        .bind(n.repo)
        .bind(n.branch)
        .bind(i32::try_from(n.number).unwrap_or(i32::MAX))
        .bind(n.job.get())
        .bind(n.author.tid().as_str())
        .bind(n.author.oid().as_str())
        .bind(n.login)
        .bind(n.url)
        .bind(n.now_ms)
        .bind(&paths)
        .bind(&hashes)
        .execute(&mut **tx)
        .await?;
    sqlx::query(KEEP_VERSIONS)
        .bind(n.swimlane.as_str())
        .bind(&paths)
        .bind(&hashes)
        .bind(n.job.get())
        .execute(&mut **tx)
        .await?;
    Ok(done.rows_affected())
}

/// A page of links, newest first.
pub(crate) async fn list(
    conn: &mut PgConnection,
    below: Option<i64>,
    swimlane: Option<&SwimlaneId>,
    state: Option<PrState>,
    limit: i64,
) -> Res<Vec<LinkRow>> {
    let rows = sqlx::query(LIST_SQL)
        .bind(below)
        .bind(swimlane.map(SwimlaneId::as_str))
        .bind(state.map(PrState::as_str))
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Internal::db("pr_link_list", &e))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        if let Some(l) = read(row) {
            out.push(l);
        } else {
            tracing::warn!("a pull request link could not be read");
        }
    }
    Ok(out)
}

/// A page of the links the caller may see.
pub(crate) async fn page(hub: &Inner, q: &LinkQuery) -> Result<Paged<PrLinkView>, ports::WriteError> {
    let below = match q.page.cursor() {
        None => None,
        Some(c) => Some(
            c.as_str()
                .parse::<i64>()
                .map_err(|_| ports::WriteError::BadRequest)?,
        ),
    };
    let limit = q.page.limit().min(MAX_PAGE);
    let mut conn = hub
        .within("pr_link_list", hub.pool.acquire())
        .await?
        .map_err(|e| Internal::db("pr_link_list", &e))?;
    // One more than the page, to know whether there is a next one.
    let mut rows = hub
        .within(
            "pr_link_list",
            list(
                &mut conn,
                below,
                q.swimlane.as_ref(),
                q.state,
                i64::from(limit) + 1,
            ),
        )
        .await??;
    drop(conn);
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = match (rows.last(), more) {
        (Some(last), true) => Cursor::parse(&last.id.get().to_string()).ok(),
        _ => None,
    };
    Ok(Paged {
        items: rows.iter().map(LinkRow::view).collect(),
        next,
    })
}

/// The statements by name, for the tests that run `EXPLAIN` on them (P12).
pub(crate) fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("pr_link_open", OPEN_SQL),
        ("pr_link_list", LIST_SQL),
        ("pr_link_place", PLACE_SQL),
        ("pr_link_keep_versions", KEEP_VERSIONS),
    ]
}

// ------------------------------------------------------------------------------------------------------------------ watcher

/// How long a replica holds the right to announce one change of one link: the replicas all receive the event within milliseconds.
const ANNOUNCE_LEASE: Duration = Duration::from_secs(5);

/// The most bytes of path text one `DriftChanged` carries, so that an event stays well inside the bus's cap (4 KiB).
pub(crate) const EVENT_PATH_BYTES: usize = 2_500;

/// What the watcher did with one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    /// Not an event about a pull request.
    Ignored,
    /// `DriftChanged` was published for the link's path.
    Announced,
    /// Another replica announced this change.
    Duplicate,
    /// The link is not there (it was deleted with its swimlane).
    Unknown,
    /// The store or the bus did not answer; the screens catch up on their next fetch.
    Unavailable,
}

/// Reacts to `PrStateChanged` (published by the webhook of prompt 03b in the transaction that moves a link): publishes
/// `DriftChanged` for the link's path, so that the drift of the file is read again and shows "fix merged; awaiting sync" once the
/// pull request is merged (D77, plan A17).
#[derive(Clone)]
pub struct PrStateWatcher {
    hub: Arc<Inner>,
}

impl std::fmt::Debug for PrStateWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrStateWatcher").finish_non_exhaustive()
    }
}

impl PrStateWatcher {
    pub(crate) fn new(hub: Arc<Inner>) -> Self {
        Self { hub }
    }

    /// Handle one event.
    pub async fn on_event(&self, event: &DomainEvent) -> Watched {
        let DomainEvent::PrStateChanged { link_id, state } = event else {
            return Watched::Ignored;
        };
        let hub = &*self.hub;
        let place = async {
            let mut conn = hub
                .pool
                .acquire()
                .await
                .map_err(|e| Internal::db("pr_link_place", &e))?;
            let row = sqlx::query(PLACE_SQL)
                .bind(link_id.get())
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| Internal::db("pr_link_place", &e))?;
            Ok::<_, Internal>(row.and_then(|r| {
                let swimlane = SwimlaneId::parse(&r.try_get::<String, _>("swimlane_id").ok()?).ok()?;
                let path = NfsPath::parse(&r.try_get::<String, _>("path").ok()?).ok()?;
                let version: i64 = r.try_get("version").ok()?;
                Some((swimlane, path, version))
            }))
        };
        let (swimlane, path, version) = match hub.within("pr_link_place", place).await {
            Ok(Ok(Some(found))) => found,
            Ok(Ok(None)) => return Watched::Unknown,
            _ => return Watched::Unavailable,
        };
        // Every replica hears the event. One of them says so: the others find the name taken. The name holds the version of the row
        // the change left, so that the next change of the same link to the same state (a close after a reopen) is a name of its own;
        // the replicas that read the same version agree on it, and one that reads a later version only says it once more, which
        // costs the screens a refetch. The lease runs out by itself.
        let name = format!("pr-state:{}:{}:{version}", link_id.get(), state.as_str());
        match hub
            .within("pr_state_lease", hub.leases.try_acquire(&name, ANNOUNCE_LEASE))
            .await
        {
            Ok(Ok(Some(_guard))) => {}
            Ok(Ok(None)) => return Watched::Duplicate,
            // Without the lease store the change is announced anyway: a repeated hint costs a refetch, a missing one a stale label.
            Ok(Err(_)) | Err(_) => tracing::warn!("the pr-state lease store did not answer"),
        }
        let event = DomainEvent::DriftChanged {
            swimlane,
            paths: vec![path],
        };
        match hub.within("pr_state_publish", hub.bus.publish(event)).await {
            Ok(Ok(())) => Watched::Announced,
            _ => Watched::Unavailable,
        }
    }

    /// The events this watcher reacts to, from now on. Subscribing is separate from following so that a caller can be sure no event
    /// is missed between the two.
    pub fn subscribe(&self) -> BoxStream<'static, DomainEvent> {
        self.hub.bus.subscribe(EventFilter {
            swimlanes: None,
            kinds: std::collections::BTreeSet::from([EventKind::PrStateChanged]),
        })
    }

    /// Follow the bus until it shuts down. Run it as a task of the process.
    pub async fn run(&self) {
        self.follow(self.subscribe()).await;
    }

    /// React to each event of `events` until the stream ends.
    pub async fn follow(&self, mut events: BoxStream<'static, DomainEvent>) {
        while let Some(event) = events.next().await {
            let seen = self.on_event(&event).await;
            if seen == Watched::Unavailable {
                tracing::warn!("a pull request state change could not be announced");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(state: PrState) -> LinkRow {
        LinkRow {
            id: PrLinkId::from(5),
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            path: NfsPath::parse("a.yml").unwrap(),
            nfs_hash: ContentHash::from_bytes([1; 32]),
            state,
            url: "https://github.com/acme/r/pull/3".to_owned(),
            number: 3,
            author: ports::conformance::sample::user_id(1),
            created_ms: 1_000,
            updated_ms: 9_000,
        }
    }

    #[test]
    fn a_link_is_closed_when_it_is_not_open_and_then_at_the_time_of_its_last_change() {
        assert_eq!(link(PrState::Open).view().closed_at, None);
        for state in [PrState::Merged, PrState::Closed] {
            let view = link(state).view();
            assert_eq!(view.closed_at, Some(Timestamp::from_unix_millis(9_000)));
            assert_eq!(view.created_at, Timestamp::from_unix_millis(1_000));
        }
        let json = serde_json::to_value(link(PrState::Merged).view()).unwrap();
        let keys: std::collections::BTreeSet<&str> =
            json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "id",
                "swimlane",
                "path",
                "nfs_hash",
                "state",
                "pr_url",
                "pr_number",
                "author",
                "created_at",
                "closed_at"
            ]),
            "the PrLink schema, and nothing else"
        );
    }

    #[test]
    fn lease_names_of_the_watcher_are_valid_for_every_state() {
        for state in PrState::ALL {
            let name = format!("pr-state:{}:{}", i64::MAX, state.as_str());
            assert!(ports::is_valid_lease_name(&name), "{name}");
        }
    }
}
