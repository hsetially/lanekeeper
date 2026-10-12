//! Users, roles and access requests (T2; S1, S4, S9).
//!
//! - [`PgUsers`] implements [`ports::Users`] over Postgres and handles the first sign-in: a new user is `pending` with no
//!   role and an access request, unless their object id is a configured bootstrap admin.
//! - [`UserAdmin`] approves and rejects requests, changes roles, status and the approval flag, files requests for active
//!   users, and lists both with keyset pagination.
//! - [`rules`] holds the pure rules (last active admin, no self changes, valid status moves).
//! - [`onboarding`] answers "does this Editor still need to store a GitHub token?" without ever reading a ciphertext.
//!
//! Every state change runs in one transaction that holds the change, the session deletion that must go with it, the
//! `EventBus::publish_in_tx` event, and, as the last statement before commit, the one audit event (D80, rule 8). The audit
//! chain lock is held from `record` to commit, so nothing slow happens after it.
//!
//! Errors carry no input: not an id, an email, a reason or a cursor (S10, S21).

use std::collections::BTreeSet;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use domain::{Guid, RequestId, Role, ShortText, Timestamp, User, UserId, UserStatus};
use ports::{
    AuditAction, AuditActor, AuditError, AuditEvent, AuditLog, AuditVia, AuthError, BusError, EventBus,
    UserError, Users,
};
use serde::Serialize;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use crate::clock::Clock;
use crate::oidc::login::SignIn;

mod admin;
pub mod cursor;
pub mod explain;
pub mod onboarding;
pub mod rules;

pub use crate::notify::AccessNotifier;
pub use admin::UserAdmin;
use rules::RuleError;

/// Every database operation of this module is cut off here, so a stuck connection cannot hold a request for ever.
const DB_TIMEOUT: Duration = Duration::from_secs(10);
/// The widest page a list returns, whatever the caller asks for.
pub const MAX_PAGE: u32 = 500;
/// The page size when the caller names none (the `OpenAPI` default).
pub const DEFAULT_PAGE: u32 = 100;
/// A user may file a new access request at most this often (B21).
pub const REQUEST_COOLDOWN_SECS: i64 = 10 * 60;

/// What the users component needs from the rest of the hub.
#[derive(Clone)]
pub struct UserServices {
    pub pool: PgPool,
    pub audit: Arc<dyn AuditLog>,
    pub bus: Arc<dyn EventBus>,
    pub notifier: AccessNotifier,
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for UserServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserServices").finish_non_exhaustive()
    }
}

/// Who performs an admin operation, from the verified session. The route guard has already checked the role.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user: UserId,
    pub request_id: Option<RequestId>,
}

/// Why an operation failed. Each variant has a status in the web API (T4 maps them); none carries input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    /// No such user or request.
    #[error("not found")]
    NotFound,
    /// The access request is no longer pending: someone decided it first.
    #[error("the access request was already decided")]
    AlreadyDecided,
    /// The user has an open request already.
    #[error("an access request is already pending")]
    RequestPending,
    /// A new request came too soon after the last one.
    #[error("too many access requests, try again later")]
    Cooldown,
    #[error("the cursor is not valid")]
    BadCursor,
    /// The change breaks a rule of [`rules`].
    #[error("{0}")]
    Rule(#[from] RuleError),
    #[error("audit log: {0}")]
    Audit(#[from] AuditError),
    #[error("event bus: {0}")]
    Bus(#[from] BusError),
    /// The database failed or timed out. The cause is logged by SQLSTATE, never returned.
    #[error("user store is unavailable")]
    Unavailable,
}

/// A request for a role, from the first sign-in or from an active user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccessRequest {
    pub id: i64,
    pub user: UserId,
    pub email: ShortText,
    pub requested_role: Option<Role>,
    pub reason: Option<ShortText>,
    pub status: AccessRequestStatus,
    pub created_at: Timestamp,
    pub decided_by: Option<UserId>,
    pub decided_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRequestStatus {
    Pending,
    Approved,
    Rejected,
}

impl AccessRequestStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

impl FromStr for AccessRequestStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            _ => Err(()),
        }
    }
}

/// One page of a keyset-paginated list. `next` is the opaque cursor of the following page, if there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

/// Filters of the user list.
#[derive(Debug, Clone, Default)]
pub struct UserQuery {
    pub status: Option<UserStatus>,
    /// Part of the email or the name, any case. `%`, `_` and `\` are ordinary characters.
    pub q: Option<ShortText>,
    pub cursor: Option<String>,
    /// Clamped to `1..=500`; none means 100.
    pub limit: Option<u32>,
}

/// Filters of the access-request list (newest first).
#[derive(Debug, Clone, Default)]
pub struct RequestQuery {
    pub status: Option<AccessRequestStatus>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// The page size to use: the caller's, clamped; a zero or missing limit is the default and never "everything".
pub(crate) fn page_size(limit: Option<u32>) -> u32 {
    match limit {
        None | Some(0) => DEFAULT_PAGE,
        Some(n) => n.min(MAX_PAGE),
    }
}

/// Plain text for display: control characters removed, cut at a character boundary to at most `max` bytes.
pub(crate) fn clip_text(raw: &str, max: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max));
    for c in raw.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// A database error, logged by operation and SQLSTATE only: the message can quote values.
pub(crate) fn db_err(op: &'static str, e: &sqlx::Error) -> AccessError {
    let state = e
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
        .unwrap_or_else(|| "none".to_owned());
    tracing::warn!(op, sqlstate = %state, "database error");
    AccessError::Unavailable
}

/// The SQLSTATE of a database error.
pub(crate) fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
}

/// Run `f` with the module's timeout. A dropped future rolls its transaction back, so a timeout leaves nothing half-written.
pub(crate) async fn bounded<T>(
    op: &'static str,
    f: impl Future<Output = Result<T, AccessError>>,
) -> Result<T, AccessError> {
    if let Ok(r) = tokio::time::timeout(DB_TIMEOUT, f).await {
        r
    } else {
        tracing::warn!(op, "database operation timed out");
        Err(AccessError::Unavailable)
    }
}

/// The columns of a user, with the last-seen time as milliseconds.
pub(crate) const USER_COLS: &str = "tid, oid, email, display_name, role, status, requires_approval, github_login, \
                                    (extract(epoch from last_seen_at) * 1000)::bigint AS last_seen_ms";

pub(crate) fn millis(ts: Timestamp) -> i64 {
    ts.unix_millis()
}

pub(crate) fn user_id_from(tid: &str, oid: &str) -> Result<UserId, AccessError> {
    match (Guid::parse(tid), Guid::parse(oid)) {
        (Ok(tid), Ok(oid)) => Ok(UserId::new(tid, oid)),
        _ => Err(AccessError::Unavailable),
    }
}

/// A row of `users` (selected with [`USER_COLS`]). A stored value that is not valid is a damaged database, not user input.
pub(crate) fn user_from_row(row: &PgRow) -> Result<User, AccessError> {
    let get = |name: &str| -> Result<String, AccessError> {
        row.try_get(name).map_err(|e| db_err("read user", &e))
    };
    let tid = get("tid")?;
    let oid = get("oid")?;
    let role: Option<String> = row.try_get("role").map_err(|e| db_err("read user", &e))?;
    let github: Option<String> = row.try_get("github_login").map_err(|e| db_err("read user", &e))?;
    let seen: Option<i64> = row.try_get("last_seen_ms").map_err(|e| db_err("read user", &e))?;
    Ok(User {
        id: user_id_from(&tid, &oid)?,
        email: ShortText::parse(&get("email")?).map_err(|_| AccessError::Unavailable)?,
        display_name: ShortText::parse(&get("display_name")?).map_err(|_| AccessError::Unavailable)?,
        role: match role {
            None => None,
            Some(r) => Some(Role::from_str(&r).map_err(|_| AccessError::Unavailable)?),
        },
        status: UserStatus::from_str(&get("status")?).map_err(|_| AccessError::Unavailable)?,
        requires_approval: row
            .try_get("requires_approval")
            .map_err(|e| db_err("read user", &e))?,
        github_login: match github {
            None => None,
            Some(g) => Some(ShortText::parse(&g).map_err(|_| AccessError::Unavailable)?),
        },
        last_seen_at: seen.map(Timestamp::from_unix_millis),
    })
}

pub(crate) fn standing(u: &User) -> rules::Standing {
    rules::Standing {
        role: u.role,
        status: u.status,
        requires_approval: u.requires_approval,
    }
}

/// `Option<Role>` as the diff text shows it.
pub(crate) fn role_text(r: Option<Role>) -> &'static str {
    r.map_or("none", Role::as_str)
}

/// The result of a sign-in's user step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub user: User,
    /// True when this sign-in created the user.
    pub created: bool,
}

/// The user directory over Postgres: the [`Users`] port and the first sign-in.
#[derive(Clone)]
pub struct PgUsers {
    svc: UserServices,
    bootstrap_admins: Arc<BTreeSet<Guid>>,
}

impl std::fmt::Debug for PgUsers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgUsers")
            .field("bootstrap_admins", &self.bootstrap_admins.len())
            .finish_non_exhaustive()
    }
}

impl PgUsers {
    /// `bootstrap_admins`: object ids that become active admins when they first sign in (S1). Nobody else is promoted by it.
    pub fn new(svc: UserServices, bootstrap_admins: BTreeSet<Guid>) -> Self {
        Self {
            svc,
            bootstrap_admins: Arc::new(bootstrap_admins),
        }
    }

    /// Record a completed Entra sign-in.
    ///
    /// - A user seen for the first time is created: `pending` with no role and an access request, which publishes
    ///   `AccessRequestCreated`, writes one audit event and, after the commit, notifies the admins. A configured bootstrap
    ///   admin is created `active` with the `admin` role instead, with one audit event and no request.
    /// - A known user keeps their role and status (the bootstrap list never promotes them). Their email and name are
    ///   refreshed from the token, and `last_seen_at` is set. Identity stays `tid` plus `oid`.
    pub async fn first_sign_in(
        &self,
        who: &SignIn,
        request_id: Option<&RequestId>,
    ) -> Result<SignedIn, AccessError> {
        let signed = bounded("first_sign_in", self.sign_in_tx(who, request_id)).await?;
        if signed.notify_admins {
            self.svc.notifier.access_requested(&signed.user, None).await;
        }
        Ok(SignedIn {
            user: signed.user,
            created: signed.created,
        })
    }

    async fn sign_in_tx(
        &self,
        who: &SignIn,
        request_id: Option<&RequestId>,
    ) -> Result<TxSignedIn, AccessError> {
        let now = self.svc.clock.now();
        let id = &who.user;
        let email = who
            .email
            .as_ref()
            .map(ShortText::as_str)
            .filter(|e| !e.is_empty());
        let name = who
            .display_name
            .as_ref()
            .map(ShortText::as_str)
            .filter(|n| !n.is_empty());
        // What a new row shows when the token has no email or name: the object id, which is never empty.
        let new_email = email.map_or_else(|| id.oid().to_string(), str::to_owned);
        let new_name = name.map_or_else(|| new_email.clone(), str::to_owned);
        let bootstrap = self.bootstrap_admins.contains(id.oid());
        let (role, status) = if bootstrap {
            (Some(Role::Admin), UserStatus::Active)
        } else {
            (None, UserStatus::Pending)
        };

        let mut tx = self.svc.pool.begin().await.map_err(|e| db_err("begin", &e))?;
        let inserted = sqlx::query(&format!(
            "INSERT INTO users (tid, oid, email, display_name, role, status, last_seen_at) \
             VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7::bigint / 1000.0)) \
             ON CONFLICT (tid, oid) DO NOTHING RETURNING {USER_COLS}"
        ))
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(&new_email)
        .bind(&new_name)
        .bind(role.map(Role::as_str))
        .bind(status.as_str())
        .bind(millis(now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| db_err("insert user", &e))?;

        let Some(row) = inserted else {
            // A known user: refresh what the identity provider owns, keep what the admins own.
            let user = refresh_known(&mut tx, id, email, name, now).await?;
            tx.commit().await.map_err(|e| db_err("commit", &e))?;
            return Ok(TxSignedIn {
                user,
                created: false,
                notify_admins: false,
            });
        };
        let user = user_from_row(&row)?;
        let event = if bootstrap {
            bootstrap_event(id, request_id, now)?
        } else {
            self.request_event(&mut tx, id, request_id, now).await?
        };
        // The audit event is the last statement: the chain lock it takes is held until the commit below.
        self.svc
            .audit
            .record(&mut ports::Tx::from_pg(&mut tx), event)
            .await?;
        tx.commit().await.map_err(|e| db_err("commit", &e))?;
        Ok(TxSignedIn {
            user,
            created: true,
            notify_admins: !bootstrap,
        })
    }

    /// The first sign-in of a user who needs a decision: the access request row, the outbox event, and the audit event to record.
    async fn request_event(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: &UserId,
        request_id: Option<&RequestId>,
        now: Timestamp,
    ) -> Result<AuditEvent, AccessError> {
        let request: i64 = sqlx::query_scalar(
            "INSERT INTO access_requests (user_tid, user_oid, created_at) \
             VALUES ($1, $2, to_timestamp($3::bigint / 1000.0)) RETURNING id",
        )
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(millis(now))
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| db_err("insert access request", &e))?;
        self.svc
            .bus
            .publish_in_tx(
                &mut ports::Tx::from_pg(tx),
                domain::DomainEvent::AccessRequestCreated { user: id.clone() },
            )
            .await?;
        let mut e = AuditEvent::new(
            now,
            AuditActor::User { user: id.clone() },
            AuditAction::AccessRequested,
            AuditVia::Ui,
        );
        e.request_id = request_id.cloned();
        e.diff = Some(format!("user={id}; first sign-in; request={request}"));
        Ok(e)
    }

    /// Remember that the user was active now. Not audited: it is maintenance, not an action (B19).
    pub async fn touch_last_seen(&self, id: &UserId) -> Result<(), UserError> {
        let now = self.svc.clock.now();
        let run = async {
            sqlx::query("UPDATE users SET last_seen_at = to_timestamp($3::bigint / 1000.0) WHERE tid = $1 AND oid = $2")
                .bind(id.tid().as_str())
                .bind(id.oid().as_str())
                .bind(millis(now))
                .execute(&self.svc.pool)
                .await
                .map_err(|e| db_err("touch last seen", &e))?;
            Ok(())
        };
        bounded("touch_last_seen", run)
            .await
            .map_err(|_| UserError::Unavailable)
    }

    async fn fetch(&self, id: &UserId) -> Result<Option<User>, AccessError> {
        let run = async {
            let row = sqlx::query(&format!(
                "SELECT {USER_COLS} FROM users WHERE tid = $1 AND oid = $2"
            ))
            .bind(id.tid().as_str())
            .bind(id.oid().as_str())
            .fetch_optional(&self.svc.pool)
            .await
            .map_err(|e| db_err("get user", &e))?;
            row.as_ref().map(user_from_row).transpose()
        };
        bounded("get_user", run).await
    }
}

/// What the transaction of a sign-in decided.
struct TxSignedIn {
    user: User,
    created: bool,
    /// The admins are to be told, after the commit, that someone waits for a decision.
    notify_admins: bool,
}

/// A returning user: email and name come from the identity provider (when the token has them), `last_seen_at` is now.
async fn refresh_known(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &UserId,
    email: Option<&str>,
    name: Option<&str>,
    now: Timestamp,
) -> Result<User, AccessError> {
    let row = sqlx::query(&format!(
        "UPDATE users SET email = COALESCE($3, email), display_name = COALESCE($4, display_name), \
         last_seen_at = to_timestamp($5::bigint / 1000.0) WHERE tid = $1 AND oid = $2 RETURNING {USER_COLS}"
    ))
    .bind(id.tid().as_str())
    .bind(id.oid().as_str())
    .bind(email)
    .bind(name)
    .bind(millis(now))
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| db_err("refresh user", &e))?;
    user_from_row(&row)
}

/// The audit event of a bootstrap admin's creation: the system made them an admin, as the configuration says.
fn bootstrap_event(
    id: &UserId,
    request_id: Option<&RequestId>,
    now: Timestamp,
) -> Result<AuditEvent, AccessError> {
    let component = ShortText::parse("bootstrap").map_err(|_| AccessError::Unavailable)?;
    let mut e = AuditEvent::new(
        now,
        AuditActor::System { component },
        AuditAction::RoleChanged,
        AuditVia::Ui,
    );
    e.request_id = request_id.cloned();
    e.diff = Some(format!(
        "user={id}; role: none -> admin; status: pending -> active; bootstrap admin from configuration"
    ));
    Ok(e)
}

#[async_trait::async_trait]
impl Users for PgUsers {
    async fn get(&self, id: &UserId) -> Result<Option<User>, UserError> {
        self.fetch(id).await.map_err(|_| UserError::Unavailable)
    }

    async fn require_active(&self, id: &UserId, min: Role) -> Result<User, AuthError> {
        let user = self
            .fetch(id)
            .await
            .map_err(|_| AuthError::Unavailable)?
            .ok_or(AuthError::UnknownUser)?;
        match user.status {
            UserStatus::Pending => Err(AuthError::Pending),
            UserStatus::Disabled => Err(AuthError::Disabled),
            UserStatus::Active if user.allows(min) => Ok(user),
            UserStatus::Active => Err(AuthError::InsufficientRole),
        }
    }
}
