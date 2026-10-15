//! Drafts (MCP `propose_change`, `POST /drafts`): a change prepared and checked, with no effect.
//!
//! - **Creating** a draft validates the content the way an edit does (the form of the file it replaces, the parse, the secret
//!   scan), keeps the new version as a blob, and stores the hunks and the settings it would change, with flagged values masked
//!   (S13, S14b). It writes nothing to NFS, Git or access, so it records no audit event (plan A5); the draft expires after a
//!   day.
//! - **Applying** a draft is an edit by the same user, run through the whole pipeline (the lock, the secret scan, approval for a
//!   user who `requires_approval`, the expected hash at the agent), so a draft never writes anything an edit could not. The
//!   audit event is `DraftApplied`, and the draft is marked with the write in the same transaction.
//! - A draft belongs to its author: another user's draft is as good as no draft (not found).

use std::str::FromStr;

use bytes::Bytes;
use domain::{
    ContentHash, Draft, DraftId, EditRequest, Guid, NfsPath, ProposalStatus, RejectReason, Role, SwimlaneId,
    Timestamp, User, UserId, WriteCtx, WriteOutcome,
};
use ports::WriteError;
use sqlx::{PgConnection, Row};

use crate::db::{ms, ts};
use crate::error::{Internal, Res};
use crate::idempotency::RequestHash;
use crate::ops::edit;
use crate::ops::file::{load_old, prepare, rejected};
use crate::pipeline::authorize::authorize;
use crate::pipeline::claim::{Claim, Claimed, claim, settle};
use crate::pipeline::execute::store_blob;
use crate::pipeline::lock::{LockState, check as check_lock};
use crate::pipeline::{Abort, Done, Step};
use crate::proposals::view::diff_view;
use crate::service::{Inner, millis};

/// The most drafts one user may have open at a time: a draft holds a blob and a row until it expires.
pub const MAX_OPEN_DRAFTS: i64 = 100;

const INSERT: &str = concat!(
    "INSERT INTO drafts (id, author_tid, author_oid, swimlane_id, path, expected, new_hash, hunks, setting_changes, status, \
     created_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft', ",
    ts!("10"),
    ", ",
    ts!("11"),
    ")"
);

const BLOB_REF: &str = "INSERT INTO blob_refs (hash, ref_kind, ref_id) SELECT hash, 'draft', $2 FROM blobs WHERE hash = $1 ON CONFLICT DO NOTHING";

pub(crate) const GET: &str = concat!(
    "SELECT id, author_tid, author_oid, swimlane_id, path, expected, new_hash, hunks, setting_changes, status, ",
    ms!("created_at"),
    " AS created_ms, ",
    ms!("expires_at"),
    " AS expires_ms FROM drafts WHERE id = $1"
);

/// How many drafts the author has open and not run out, at most one more than the cap.
pub(crate) const OPEN_COUNT: &str = concat!(
    "SELECT count(*) FROM (SELECT 1 FROM drafts WHERE author_tid = $1 AND author_oid = $2 \
     AND status IN ('draft', 'pending') AND expires_at > ",
    ts!("3"),
    " LIMIT 101) t"
);

/// Drafts whose time has run out and that are still open: `$1` is now, `$2` the most rows. Read through the partial index.
pub(crate) const EXPIRE: &str = concat!(
    "UPDATE drafts SET status = 'expired' WHERE id IN ( \
       SELECT id FROM drafts WHERE status IN ('draft', 'pending') AND expires_at <= ",
    ts!("1"),
    " ORDER BY expires_at LIMIT $2)"
);

const MARK: &str = "UPDATE drafts SET status = $2 WHERE id = $1 AND status IN ('draft', 'pending')";

/// Move an open draft to `to` (it waits with its proposal, is applied, goes stale, is rejected or expires). A draft that is no
/// longer open is left as it is.
pub(crate) async fn mark(conn: &mut PgConnection, id: &str, to: ProposalStatus) -> Res<()> {
    sqlx::query(MARK)
        .bind(id)
        .bind(to.as_str())
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("draft_mark", &e))?;
    Ok(())
}

/// Mark the drafts whose time has run out. Returns how many. A draft is a record, not a change of state anyone depends on, so
/// this writes no audit event (plan A5).
pub(crate) async fn expire(conn: &mut PgConnection, now_ms: i64, limit: i64) -> Res<u64> {
    let done = sqlx::query(EXPIRE)
        .bind(now_ms)
        .bind(limit)
        .execute(&mut *conn)
        .await
        .map_err(|e| Internal::db("draft_expire", &e))?;
    Ok(done.rows_affected())
}

/// A draft as the table holds it, and when it runs out.
struct Stored {
    draft: Draft,
    expires_ms: i64,
}

fn read(row: &sqlx::postgres::PgRow) -> Option<Stored> {
    let id: String = row.try_get("id").ok()?;
    let tid: String = row.try_get("author_tid").ok()?;
    let oid: String = row.try_get("author_oid").ok()?;
    let expected: serde_json::Value = row.try_get("expected").ok()?;
    let new_hash: Vec<u8> = row.try_get("new_hash").ok()?;
    let hunks: serde_json::Value = row.try_get("hunks").ok()?;
    let settings: serde_json::Value = row.try_get("setting_changes").ok()?;
    let status: String = row.try_get("status").ok()?;
    let expires_ms: i64 = row.try_get("expires_ms").ok()?;
    Some(Stored {
        draft: Draft {
            id: DraftId::parse(&id).ok()?,
            author: UserId::new(Guid::parse(&tid).ok()?, Guid::parse(&oid).ok()?),
            swimlane: SwimlaneId::parse(&row.try_get::<String, _>("swimlane_id").ok()?).ok()?,
            path: NfsPath::parse(&row.try_get::<String, _>("path").ok()?).ok()?,
            expected: serde_json::from_value(expected).ok()?,
            new_hash: ContentHash::from_bytes(new_hash.try_into().ok()?),
            hunks: serde_json::from_value(hunks).ok()?,
            setting_changes: serde_json::from_value(settings).ok()?,
            status: ProposalStatus::from_str(&status).ok()?,
            expires_at: Timestamp::from_unix_millis(expires_ms),
        },
        expires_ms,
    })
}

async fn get(hub: &Inner, id: &DraftId) -> Res<Option<Stored>> {
    let row = hub
        .within(
            "draft_get",
            sqlx::query(GET).bind(id.as_str()).fetch_optional(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("draft_get", &e))?;
    match row {
        None => Ok(None),
        Some(row) => read(&row)
            .map(Some)
            .ok_or_else(|| Internal::new("draft_get", "row")),
    }
}

/// A fresh, unguessable draft id: 16 random bytes.
fn new_id() -> Option<DraftId> {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).ok()?;
    DraftId::parse(&format!("d-{}", crate::hex::encode(&bytes))).ok()
}

/// Create a draft. See the module documentation.
pub(crate) async fn propose_draft(hub: &Inner, ctx: &WriteCtx, r: EditRequest) -> Result<Draft, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    // The swimlane has to exist. Whether it is unlocked is for the apply to say: a draft changes nothing.
    if matches!(check_lock(hub, &r.swimlane).await?, LockState::UnknownSwimlane) {
        return Err(WriteError::NotFound);
    }
    if r.content.len() > hub.cfg.max_content_bytes || hub.deny.is_denied(&r.path) {
        return Err(WriteError::BadRequest);
    }
    let open: i64 = hub
        .within(
            "draft_count",
            sqlx::query_scalar(OPEN_COUNT)
                .bind(user.id.tid().as_str())
                .bind(user.id.oid().as_str())
                .bind(hub.now_ms())
                .fetch_one(&hub.pool),
        )
        .await?
        .map_err(|e| Internal::db("draft_count", &e))?;
    if open >= MAX_OPEN_DRAFTS {
        return Err(WriteError::BadRequest);
    }

    // A file the registry withholds is not drafted over.
    let Ok(old) = load_old(hub, &user, &r.swimlane, &r.path, &r.expected)
        .await
        .map_err(|a| a.error)?
    else {
        return Err(WriteError::BadRequest);
    };
    let Ok((prepared, checked)) = prepare(
        hub,
        &user,
        &r.swimlane,
        &r.path,
        &r.content,
        r.change_eol,
        old.clone(),
    )
    .await
    .map_err(|a| a.error)?
    else {
        return Err(WriteError::BadRequest);
    };
    // Content that would be refused as an edit is not worth keeping as a draft.
    if checked.reject.is_some() {
        return Err(WriteError::BadRequest);
    }
    let new_hash = store_blob(hub, prepared.clone()).await?;

    let view = hub
        .blocking("draft_view", {
            let (path, deny, limits) = (r.path.clone(), hub.deny.clone(), hub.limits.clone());
            let new = prepared;
            move || diff_view(&path, &old, &new, &deny, &limits)
        })
        .await?;

    let id = new_id().ok_or(WriteError::Unavailable)?;
    let now = hub.now_ms();
    let expires = now.saturating_add(millis(hub.cfg.draft_ttl));
    let mut tx = hub
        .within("draft_insert", hub.pool.begin())
        .await?
        .map_err(|e| Internal::db("draft_insert", &e))?;
    sqlx::query(INSERT)
        .bind(id.as_str())
        .bind(user.id.tid().as_str())
        .bind(user.id.oid().as_str())
        .bind(r.swimlane.as_str())
        .bind(r.path.as_str())
        .bind(serde_json::to_value(r.expected).map_err(|_| Internal::new("draft_insert", "encode"))?)
        .bind(new_hash.as_bytes().as_slice())
        .bind(serde_json::to_value(&view.hunks).map_err(|_| Internal::new("draft_insert", "encode"))?)
        .bind(
            serde_json::to_value(&view.setting_changes)
                .map_err(|_| Internal::new("draft_insert", "encode"))?,
        )
        .bind(now)
        .bind(expires)
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("draft_insert", &e))?;
    sqlx::query(BLOB_REF)
        .bind(new_hash.as_bytes().as_slice())
        .bind(id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| Internal::db("draft_insert", &e))?;
    tx.commit().await.map_err(|e| Internal::db("draft_insert", &e))?;

    Ok(Draft {
        id,
        author: user.id.clone(),
        swimlane: r.swimlane,
        path: r.path,
        expected: r.expected,
        new_hash,
        hunks: view.hunks,
        setting_changes: view.setting_changes,
        status: ProposalStatus::Draft,
        expires_at: Timestamp::from_unix_millis(expires),
    })
}

/// Apply a draft. See the module documentation.
pub(crate) async fn apply_draft(
    hub: &Inner,
    ctx: &WriteCtx,
    id: DraftId,
) -> Result<WriteOutcome, WriteError> {
    let user = authorize(hub, ctx, Role::Editor).await?;
    let claim = match claim(hub, ctx, RequestHash::of_apply_draft(&id)).await? {
        Claimed::Replay(outcome) => return Ok(outcome),
        Claimed::Mine(claim) => claim,
    };
    let result = run(hub, ctx, &user, &claim, &id).await;
    let settled = settle(hub, &claim, result).await;
    // The file is not what the draft was made against: the draft cannot be applied any more.
    if matches!(settled, Ok(WriteOutcome::Conflict { .. })) {
        if let Ok(Ok(mut conn)) = hub.within("draft_stale", hub.pool.acquire()).await {
            let _ = mark(&mut conn, id.as_str(), ProposalStatus::Stale).await;
        }
    }
    settled
}

async fn run(hub: &Inner, ctx: &WriteCtx, user: &User, claim: &Claim, id: &DraftId) -> Step<Done> {
    let Some(stored) = get(hub, id).await? else {
        return Err(Abort::release(WriteError::NotFound));
    };
    let d = stored.draft;
    // Somebody else's draft is not a draft the caller has.
    if d.author != user.id {
        return Err(Abort::release(WriteError::NotFound));
    }
    if d.status != ProposalStatus::Draft || stored.expires_ms <= hub.now_ms() {
        return Ok(Done::Store(rejected(RejectReason::NotAllowed)));
    }
    let content: Bytes = match hub.within("draft_blob", hub.blobs.get(&d.new_hash)).await? {
        Ok(Some(bytes)) if ports::content_hash(&bytes) == d.new_hash => bytes,
        _ => return Err(Abort::release(WriteError::Unavailable)),
    };
    let request = EditRequest {
        swimlane: d.swimlane,
        path: d.path,
        expected: d.expected,
        content,
        change_eol: None,
    };
    edit::run(hub, ctx, user, claim, request, Some(id.as_str())).await
}
