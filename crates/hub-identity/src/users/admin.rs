//! Admin operations on users and access requests (T2; S1, S4, S9). Every operation is one transaction.
//!
//! Lock order, the same everywhere so that transactions cannot deadlock: the admin-set lock, then the rows (the request,
//! then the user), then the audit chain lock inside `AuditLog::record`, which is the last statement before the commit.

use domain::{DomainEvent, Role, ShortText, Timestamp, User, UserId};
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia, Tx};
use sqlx::postgres::{PgConnection, PgRow};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use std::str::FromStr;

use super::cursor::{decode_request, decode_user, encode_request, encode_user};
use super::rules::{self, Change, Outcome, Standing};
use super::{
    AccessError, AccessRequest, AccessRequestStatus, Actor, Page, REQUEST_COOLDOWN_SECS, RequestQuery,
    USER_COLS, UserQuery, UserServices, bounded, db_err, millis, page_size, role_text, sqlstate, standing,
    user_from_row, user_id_from,
};

/// Serialises every change to the set of admins, so that "the last active admin" is decided on a state nobody else is
/// changing. Admin changes are rare; the lock is held for one short transaction.
const ADMIN_SET_LOCK: i64 = 7_160_002;

/// The columns of an access request joined with the requester's email.
pub(super) const REQUEST_COLS: &str = "r.id, r.user_tid, r.user_oid, u.email, r.role, r.reason, r.status, \
     (extract(epoch from r.created_at) * 1000)::bigint AS created_ms, r.decided_by_tid, r.decided_by_oid, \
     (extract(epoch from r.decided_at) * 1000)::bigint AS decided_ms";

#[derive(Clone)]
pub struct UserAdmin {
    svc: UserServices,
}

impl std::fmt::Debug for UserAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserAdmin").finish_non_exhaustive()
    }
}

type Tx_<'a> = Transaction<'a, Postgres>;

impl UserAdmin {
    pub fn new(svc: UserServices) -> Self {
        Self { svc }
    }

    // -----------------------------------------------------------------------------------------------------------
    // Decisions

    /// Approve an open access request with `role`: the user becomes active with that role, the request is closed, and every
    /// session the user holds ends. Nobody approves their own request.
    pub async fn approve(&self, actor: &Actor, request: i64, role: Role) -> Result<User, AccessError> {
        bounded("approve", async {
            let now = self.svc.clock.now();
            let mut tx = self.svc.pool.begin().await.map_err(|e| db_err("begin", &e))?;
            lock_admin_set(&mut tx).await?;
            let target = lock_open_request(&mut tx, request).await?;
            let current = lock_user(&mut tx, &target)
                .await?
                .ok_or(AccessError::Unavailable)?;
            let others = other_active_admins(&mut tx, &target).await?;
            let outcome = rules::plan_approval(actor.user == target, standing(&current), role, others)?;

            let user = if outcome.is_noop() {
                current.clone()
            } else {
                write_standing(&mut tx, &target, outcome.after).await?
            };
            if outcome.ends_sessions() {
                self.end_sessions(&mut tx, &target).await?;
            }
            close_request(&mut tx, request, AccessRequestStatus::Approved, &actor.user, now).await?;

            let diff = format!(
                "request={request}; user={target}; {}",
                describe(&current, outcome)
            );
            self.record(&mut tx, actor, now, AuditAction::AccessApproved, diff)
                .await?;
            tx.commit().await.map_err(|e| db_err("commit", &e))?;
            Ok(user)
        })
        .await
    }

    /// Reject an open access request. The user stays as they are. The reason goes into the audit event.
    pub async fn reject(
        &self,
        actor: &Actor,
        request: i64,
        reason: &ShortText,
    ) -> Result<AccessRequest, AccessError> {
        bounded("reject", async {
            let now = self.svc.clock.now();
            let mut tx = self.svc.pool.begin().await.map_err(|e| db_err("begin", &e))?;
            let target = lock_open_request(&mut tx, request).await?;
            close_request(&mut tx, request, AccessRequestStatus::Rejected, &actor.user, now).await?;
            let diff = format!("request={request}; user={target}; rejected; reason={reason}");
            self.record(&mut tx, actor, now, AuditAction::AccessDenied, diff)
                .await?;
            let done = fetch_request(&mut tx, request)
                .await?
                .ok_or(AccessError::Unavailable)?;
            tx.commit().await.map_err(|e| db_err("commit", &e))?;
            Ok(done)
        })
        .await
    }

    /// Change a user's role, status or approval flag. See [`rules::plan_update`] for what is refused. A change that changes
    /// nothing writes nothing. A role or status change ends every session of the user in the same transaction.
    pub async fn update_user(
        &self,
        actor: &Actor,
        target: &UserId,
        change: Change,
    ) -> Result<User, AccessError> {
        bounded("update_user", async {
            let now = self.svc.clock.now();
            let mut tx = self.svc.pool.begin().await.map_err(|e| db_err("begin", &e))?;
            lock_admin_set(&mut tx).await?;
            let current = lock_user(&mut tx, target).await?.ok_or(AccessError::NotFound)?;
            let others = other_active_admins(&mut tx, target).await?;
            let outcome = rules::plan_update(actor.user == *target, standing(&current), change, others)?;
            if outcome.is_noop() {
                return Ok(current);
            }
            let user = write_standing(&mut tx, target, outcome.after).await?;
            if outcome.ends_sessions() {
                self.end_sessions(&mut tx, target).await?;
            }
            // One operation, one event. When the status moved, that is the action that matters most.
            let action = if outcome.status_changed {
                AuditAction::UserStatusChanged
            } else {
                AuditAction::RoleChanged
            };
            let diff = format!("user={target}; {}", describe(&current, outcome));
            self.record(&mut tx, actor, now, action, diff).await?;
            tx.commit().await.map_err(|e| db_err("commit", &e))?;
            Ok(user)
        })
        .await
    }

    // -----------------------------------------------------------------------------------------------------------
    // Requests from an active user

    /// An active user asks for another role (`POST /me/access-request`). One open request at a time, and one new request
    /// per [`REQUEST_COOLDOWN_SECS`] (B21). Publishes `AccessRequestCreated` and, after the commit, notifies the admins.
    pub async fn create_request(
        &self,
        actor: &Actor,
        role: Role,
        reason: &ShortText,
    ) -> Result<AccessRequest, AccessError> {
        let created = bounded("create_request", async {
            let now = self.svc.clock.now();
            let mut tx = self.svc.pool.begin().await.map_err(|e| db_err("begin", &e))?;
            // The user's own row serialises their requests.
            let user = lock_user(&mut tx, &actor.user).await?.ok_or(AccessError::NotFound)?;
            rules::check_request(standing(&user), role)?;

            let open: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM access_requests WHERE user_tid = $1 AND user_oid = $2 AND status = 'pending')",
            )
            .bind(actor.user.tid().as_str())
            .bind(actor.user.oid().as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| db_err("check open request", &e))?;
            if open {
                return Err(AccessError::RequestPending);
            }
            let recent: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM access_requests WHERE user_tid = $1 AND user_oid = $2 \
                 AND created_at > to_timestamp($3::bigint / 1000.0))",
            )
            .bind(actor.user.tid().as_str())
            .bind(actor.user.oid().as_str())
            .bind(millis(now) - REQUEST_COOLDOWN_SECS * 1000)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| db_err("check cooldown", &e))?;
            if recent {
                return Err(AccessError::Cooldown);
            }

            let id: i64 = sqlx::query_scalar(
                "INSERT INTO access_requests (user_tid, user_oid, role, reason, created_at) \
                 VALUES ($1, $2, $3, $4, to_timestamp($5::bigint / 1000.0)) RETURNING id",
            )
            .bind(actor.user.tid().as_str())
            .bind(actor.user.oid().as_str())
            .bind(role.as_str())
            .bind(reason.as_str())
            .bind(millis(now))
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                // Two requests at once: the unique index on open requests decides.
                if sqlstate(&e).as_deref() == Some("23505") {
                    AccessError::RequestPending
                } else {
                    db_err("insert access request", &e)
                }
            })?;
            self.svc
                .bus
                .publish_in_tx(
                    &mut Tx::from_pg(&mut tx),
                    DomainEvent::AccessRequestCreated {
                        user: actor.user.clone(),
                    },
                )
                .await?;
            let diff = format!(
                "request={id}; user={}; asked for role {} (now {})",
                actor.user,
                role,
                role_text(user.role)
            );
            self.record(&mut tx, actor, now, AuditAction::AccessRequested, diff).await?;
            let created = fetch_request(&mut tx, id).await?.ok_or(AccessError::Unavailable)?;
            tx.commit().await.map_err(|e| db_err("commit", &e))?;
            Ok((created, user))
        })
        .await?;
        let (request, user) = created;
        self.svc
            .notifier
            .access_requested(&user, request.requested_role)
            .await;
        Ok(request)
    }

    // -----------------------------------------------------------------------------------------------------------
    // Lists (keyset, bounded, indexed; P12)

    /// Users in `(tid, oid)` order. At most 500 per page.
    pub async fn list_users(&self, q: &UserQuery) -> Result<Page<User>, AccessError> {
        let size = page_size(q.limit);
        let mut builder = user_list_builder("", q, size)?;
        bounded("list_users", async {
            let rows = builder
                .build()
                .fetch_all(&self.svc.pool)
                .await
                .map_err(|e| db_err("list users", &e))?;
            let mut items = rows.iter().map(user_from_row).collect::<Result<Vec<_>, _>>()?;
            let next = trim_page(&mut items, size, |u| encode_user(&u.id));
            Ok(Page { items, next })
        })
        .await
    }

    /// Access requests, newest first. At most 500 per page.
    pub async fn list_requests(&self, q: &RequestQuery) -> Result<Page<AccessRequest>, AccessError> {
        let size = page_size(q.limit);
        let mut builder = request_list_builder("", q, size)?;
        bounded("list_requests", async {
            let rows = builder
                .build()
                .fetch_all(&self.svc.pool)
                .await
                .map_err(|e| db_err("list requests", &e))?;
            let mut items = rows.iter().map(request_from_row).collect::<Result<Vec<_>, _>>()?;
            let next = trim_page(&mut items, size, |r| encode_request(r.id));
            Ok(Page { items, next })
        })
        .await
    }

    // -----------------------------------------------------------------------------------------------------------

    /// End every session of the user: delete the rows and publish `UserSessionsRevoked` so that every replica drops its cached
    /// copies. Runs in the caller's transaction, so the deletion, the event and the change that caused them commit or roll
    /// back together (S2, D80). The event is published even when there was no row to delete: a replica may still hold a
    /// cached session whose row a concurrent sign-in or sweep has just changed. Call it before [`UserAdmin::record`], which
    /// must stay the last statement.
    async fn end_sessions(&self, tx: &mut Tx_<'_>, user: &UserId) -> Result<u64, AccessError> {
        let ended = crate::session::store::delete_all_for_user(tx, user)
            .await
            .map_err(|_| AccessError::Unavailable)?;
        crate::session::revocation::publish_revoked(self.svc.bus.as_ref(), &mut Tx::from_pg(tx), user)
            .await?;
        Ok(ended)
    }

    /// The one audit event of the operation. Call it last.
    async fn record(
        &self,
        tx: &mut Tx_<'_>,
        actor: &Actor,
        now: Timestamp,
        action: AuditAction,
        diff: String,
    ) -> Result<(), AccessError> {
        let mut event = AuditEvent::new(
            now,
            AuditActor::User {
                user: actor.user.clone(),
            },
            action,
            AuditVia::Ui,
        );
        event.request_id.clone_from(&actor.request_id);
        event.diff = Some(diff);
        self.svc.audit.record(&mut Tx::from_pg(tx), event).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Statements

async fn lock_admin_set(tx: &mut Tx_<'_>) -> Result<(), AccessError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_SET_LOCK)
        .execute(&mut **tx)
        .await
        .map_err(|e| db_err("lock admin set", &e))?;
    Ok(())
}

async fn lock_user(tx: &mut Tx_<'_>, id: &UserId) -> Result<Option<User>, AccessError> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLS} FROM users WHERE tid = $1 AND oid = $2 FOR UPDATE"
    ))
    .bind(id.tid().as_str())
    .bind(id.oid().as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| db_err("lock user", &e))?;
    row.as_ref().map(user_from_row).transpose()
}

/// How many active admins there are besides `target`. Read under the admin-set lock.
async fn other_active_admins(tx: &mut Tx_<'_>, target: &UserId) -> Result<u64, AccessError> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active' AND NOT (tid = $1 AND oid = $2)",
    )
    .bind(target.tid().as_str())
    .bind(target.oid().as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| db_err("count admins", &e))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

async fn write_standing(tx: &mut Tx_<'_>, id: &UserId, to: Standing) -> Result<User, AccessError> {
    let row = sqlx::query(&format!(
        "UPDATE users SET role = $3, status = $4, requires_approval = $5 WHERE tid = $1 AND oid = $2 RETURNING {USER_COLS}"
    ))
    .bind(id.tid().as_str())
    .bind(id.oid().as_str())
    .bind(to.role.map(Role::as_str))
    .bind(to.status.as_str())
    .bind(to.requires_approval)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| db_err("update user", &e))?;
    user_from_row(&row)
}

/// Lock an access request and return its user. Not found is 404; one that was decided already is 409.
async fn lock_open_request(tx: &mut Tx_<'_>, request: i64) -> Result<UserId, AccessError> {
    let row = sqlx::query("SELECT user_tid, user_oid, status FROM access_requests WHERE id = $1 FOR UPDATE")
        .bind(request)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| db_err("lock request", &e))?
        .ok_or(AccessError::NotFound)?;
    let status: String = row.try_get("status").map_err(|e| db_err("read request", &e))?;
    if status != AccessRequestStatus::Pending.as_str() {
        return Err(AccessError::AlreadyDecided);
    }
    let tid: String = row.try_get("user_tid").map_err(|e| db_err("read request", &e))?;
    let oid: String = row.try_get("user_oid").map_err(|e| db_err("read request", &e))?;
    user_id_from(&tid, &oid)
}

async fn close_request(
    tx: &mut Tx_<'_>,
    request: i64,
    status: AccessRequestStatus,
    by: &UserId,
    now: Timestamp,
) -> Result<(), AccessError> {
    sqlx::query(
        "UPDATE access_requests SET status = $2, decided_by_tid = $3, decided_by_oid = $4, \
         decided_at = to_timestamp($5::bigint / 1000.0) WHERE id = $1",
    )
    .bind(request)
    .bind(status.as_str())
    .bind(by.tid().as_str())
    .bind(by.oid().as_str())
    .bind(millis(now))
    .execute(&mut **tx)
    .await
    .map_err(|e| db_err("close request", &e))?;
    Ok(())
}

async fn fetch_request(tx: &mut PgConnection, id: i64) -> Result<Option<AccessRequest>, AccessError> {
    let row = sqlx::query(&format!(
        "SELECT {REQUEST_COLS} FROM access_requests r JOIN users u ON u.tid = r.user_tid AND u.oid = r.user_oid WHERE r.id = $1"
    ))
    .bind(id)
    .fetch_optional(tx)
    .await
    .map_err(|e| db_err("read request", &e))?;
    row.as_ref().map(request_from_row).transpose()
}

fn request_from_row(row: &PgRow) -> Result<AccessRequest, AccessError> {
    let bad = |e: sqlx::Error| db_err("read request", &e);
    let tid: String = row.try_get("user_tid").map_err(bad)?;
    let oid: String = row.try_get("user_oid").map_err(bad)?;
    let email: String = row.try_get("email").map_err(bad)?;
    let role: Option<String> = row.try_get("role").map_err(bad)?;
    let reason: Option<String> = row.try_get("reason").map_err(bad)?;
    let status: String = row.try_get("status").map_err(bad)?;
    let created: i64 = row.try_get("created_ms").map_err(bad)?;
    let decided_by: (Option<String>, Option<String>) = (
        row.try_get("decided_by_tid").map_err(bad)?,
        row.try_get("decided_by_oid").map_err(bad)?,
    );
    let decided_ms: Option<i64> = row.try_get("decided_ms").map_err(bad)?;
    Ok(AccessRequest {
        id: row.try_get("id").map_err(bad)?,
        user: user_id_from(&tid, &oid)?,
        email: ShortText::parse(&email).map_err(|_| AccessError::Unavailable)?,
        requested_role: match role {
            None => None,
            Some(r) => Some(Role::from_str(&r).map_err(|_| AccessError::Unavailable)?),
        },
        reason: match reason {
            None => None,
            Some(r) => Some(ShortText::parse(&r).map_err(|_| AccessError::Unavailable)?),
        },
        status: AccessRequestStatus::from_str(&status).map_err(|()| AccessError::Unavailable)?,
        created_at: Timestamp::from_unix_millis(created),
        decided_by: match decided_by {
            (Some(t), Some(o)) => Some(user_id_from(&t, &o)?),
            _ => None,
        },
        decided_at: decided_ms.map(Timestamp::from_unix_millis),
    })
}

/// "role: a -> b; status: a -> b; `requires_approval`: a -> b", only the parts that changed.
fn describe(current: &User, outcome: Outcome) -> String {
    let mut parts = Vec::new();
    if outcome.role_changed {
        parts.push(format!(
            "role: {} -> {}",
            role_text(current.role),
            role_text(outcome.after.role)
        ));
    }
    if outcome.status_changed {
        parts.push(format!("status: {} -> {}", current.status, outcome.after.status));
    }
    if outcome.flag_changed {
        parts.push(format!(
            "requires_approval: {} -> {}",
            current.requires_approval, outcome.after.requires_approval
        ));
    }
    if parts.is_empty() {
        "no change to the user".to_owned()
    } else {
        parts.join("; ")
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Keyset pagination

/// Keep `size` rows (the query asked for one more) and return the cursor of the last kept row if there was an extra one.
fn trim_page<T>(items: &mut Vec<T>, size: u32, cursor: impl Fn(&T) -> String) -> Option<String> {
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    if items.len() <= size {
        return None;
    }
    items.truncate(size);
    items.last().map(cursor)
}

/// The user list statement. Built from fixed fragments with every value bound, so no input reaches the SQL text. `prefix` is
/// empty for the real query and `EXPLAIN (FORMAT JSON) ` for the plan test, which therefore explains the same statement.
pub(super) fn user_list_builder(
    prefix: &str,
    q: &UserQuery,
    size: u32,
) -> Result<QueryBuilder<'static, Postgres>, AccessError> {
    let after = q.cursor.as_deref().map(decode_user).transpose()?;
    let mut b = QueryBuilder::new(prefix);
    b.push("SELECT ").push(USER_COLS).push(" FROM users WHERE TRUE");
    if let Some(status) = q.status {
        b.push(" AND status = ").push_bind(status.as_str());
    }
    if let Some(needle) = &q.q {
        // `position` instead of LIKE: no pattern characters to escape.
        let needle = needle.as_str().to_owned();
        b.push(" AND (position(lower(").push_bind(needle.clone());
        b.push(") in lower(email)) > 0 OR position(lower(")
            .push_bind(needle);
        b.push(") in lower(display_name)) > 0)");
    }
    if let Some(after) = after {
        b.push(" AND (tid, oid) > (")
            .push_bind(after.tid().as_str().to_owned());
        b.push(", ").push_bind(after.oid().as_str().to_owned()).push(")");
    }
    b.push(" ORDER BY tid, oid LIMIT ").push_bind(i64::from(size) + 1);
    Ok(b)
}

/// The access-request list statement, newest first. See [`user_list_builder`].
pub(super) fn request_list_builder(
    prefix: &str,
    q: &RequestQuery,
    size: u32,
) -> Result<QueryBuilder<'static, Postgres>, AccessError> {
    let before = q.cursor.as_deref().map(decode_request).transpose()?;
    let mut b = QueryBuilder::new(prefix);
    b.push("SELECT ")
        .push(REQUEST_COLS)
        .push(" FROM access_requests r JOIN users u ON u.tid = r.user_tid AND u.oid = r.user_oid WHERE TRUE");
    if let Some(status) = q.status {
        b.push(" AND r.status = ").push_bind(status.as_str());
    }
    if let Some(before) = before {
        b.push(" AND r.id < ").push_bind(before);
    }
    b.push(" ORDER BY r.id DESC LIMIT ")
        .push_bind(i64::from(size) + 1);
    Ok(b)
}
