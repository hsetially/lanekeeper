//! The users component wired to a real Postgres, the test audit chain, the outbox bus, a fake notifier and a manual clock.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use domain::{AuditId, Guid, Role, ShortText, Timestamp, User, UserId, UserStatus};
use hub_identity::oidc::login::SignIn;
use hub_identity::users::{AccessNotifier, Actor, PgUsers, UserAdmin, UserServices};
use hub_identity::{Clock, ManualClock};
use ports::fakes::FakeNotifier;
use ports::{AuditError, AuditEvent, AuditLog, Notification, Notifier, NotifyError, Tx};
use sqlx::{PgPool, Row};

use super::pg::TestDb;
use super::pg_bus::PgTestBus;
use hub_identity::audit::PgAuditLog;

/// 2026-10-11T00:00:00Z.
pub const START_MS: i64 = 1_791_676_800_000;
pub const TID: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";

pub fn oid(n: u32) -> Guid {
    Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap()
}

pub fn uid(n: u32) -> UserId {
    UserId::new(Guid::parse(TID).unwrap(), oid(n))
}

pub fn text(s: &str) -> ShortText {
    ShortText::parse(s).unwrap()
}

pub fn sign_in(n: u32) -> SignIn {
    SignIn {
        user: uid(n),
        email: Some(text(&format!("user{n}@example.com"))),
        display_name: Some(text(&format!("User {n}"))),
        return_to: "/".to_owned(),
    }
}

pub fn actor(n: u32) -> Actor {
    Actor {
        user: uid(n),
        request_id: Some(domain::RequestId::parse(&format!("req-{n}")).unwrap()),
    }
}

/// An audit log that fails every time, to prove that nothing else survives it.
#[derive(Debug)]
pub struct FailingAudit;

#[async_trait]
impl AuditLog for FailingAudit {
    async fn record(&self, _tx: &mut Tx<'_>, _e: AuditEvent) -> Result<AuditId, AuditError> {
        Err(AuditError::Unavailable)
    }
}

/// A notifier that always fails.
#[derive(Debug)]
pub struct FailingNotifier;

#[async_trait]
impl Notifier for FailingNotifier {
    async fn notify(&self, _n: Notification) -> Result<(), NotifyError> {
        Err(NotifyError::Unavailable)
    }
}

pub struct UsersRig {
    pub db: TestDb,
    pub pool: PgPool,
    pub bus: Arc<PgTestBus>,
    pub notifier: FakeNotifier,
    pub clock: Arc<ManualClock>,
    pub svc: UserServices,
    pub users: PgUsers,
    pub admin: UserAdmin,
}

impl UsersRig {
    pub async fn new(db: TestDb) -> Self {
        Self::with(db, &[], Arc::new(PgAuditLog::new()), None).await
    }

    pub async fn with_bootstrap(db: TestDb, bootstrap: &[Guid]) -> Self {
        Self::with(db, bootstrap, Arc::new(PgAuditLog::new()), None).await
    }

    pub async fn with(
        db: TestDb,
        bootstrap: &[Guid],
        audit: Arc<dyn AuditLog>,
        notifier: Option<Arc<dyn Notifier>>,
    ) -> Self {
        let pool = db.app_pool(16).await;
        let bus = Arc::new(PgTestBus::new(pool.clone()));
        let fake = FakeNotifier::new();
        let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(START_MS)));
        let svc = UserServices {
            pool: pool.clone(),
            audit,
            bus: Arc::clone(&bus) as Arc<_>,
            notifier: AccessNotifier::new(notifier.unwrap_or_else(|| Arc::new(fake.clone()))),
            clock: Arc::clone(&clock) as Arc<dyn Clock>,
        };
        let set: BTreeSet<Guid> = bootstrap.iter().cloned().collect();
        let users = PgUsers::new(svc.clone(), set);
        let admin = UserAdmin::new(svc.clone());
        Self {
            db,
            pool,
            bus,
            notifier: fake,
            clock,
            svc,
            users,
            admin,
        }
    }

    /// The same database and clock with other collaborators (a failing audit log, a failing notifier).
    pub fn variant(&self, audit: Arc<dyn AuditLog>, notifier: Arc<dyn Notifier>) -> (PgUsers, UserAdmin) {
        let svc = UserServices {
            audit,
            notifier: AccessNotifier::new(notifier),
            ..self.svc.clone()
        };
        (PgUsers::new(svc.clone(), BTreeSet::new()), UserAdmin::new(svc))
    }

    /// Insert a user directly (a fixture, not a code path under test).
    pub async fn seed(&self, n: u32, role: Option<Role>, status: UserStatus) -> UserId {
        let id = uid(n);
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(format!("user{n}@example.com"))
        .bind(format!("User {n}"))
        .bind(role.map(Role::as_str))
        .bind(status.as_str())
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    pub async fn seed_admin(&self, n: u32) -> UserId {
        self.seed(n, Some(Role::Admin), UserStatus::Active).await
    }

    /// A session row for the user (the session code is T3; the rows are what the revocation rules act on).
    pub async fn seed_session(&self, user: &UserId, tag: u8) {
        sqlx::query(
            "INSERT INTO sessions (session_hash, user_tid, user_oid, csrf_hash, expires_at) \
             VALUES (sha256($1::bytea), $2, $3, sha256($1::bytea), now() + interval '12 hours')",
        )
        .bind(vec![tag; 4])
        .bind(user.tid().as_str())
        .bind(user.oid().as_str())
        .execute(&self.pool)
        .await
        .unwrap();
    }

    pub async fn session_count(&self, user: &UserId) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_tid = $1 AND user_oid = $2")
            .bind(user.tid().as_str())
            .bind(user.oid().as_str())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn audit_actions(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit_events ORDER BY seq LIMIT 1000")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    pub async fn audit_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_events")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The `diff` text of the newest audit event.
    pub async fn last_audit_diff(&self) -> String {
        let json: String =
            sqlx::query_scalar("SELECT event_json FROM audit_events ORDER BY seq DESC LIMIT 1")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["diff"].as_str().unwrap_or_default().to_owned()
    }

    pub async fn outbox_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn request_rows(&self) -> Vec<(i64, String, String)> {
        sqlx::query("SELECT id, user_oid, status FROM access_requests ORDER BY id LIMIT 1000")
            .fetch_all(&self.pool)
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get("id"), r.get("user_oid"), r.get("status")))
            .collect()
    }

    pub async fn stored(&self, id: &UserId) -> User {
        use ports::Users;
        self.users.get(id).await.unwrap().unwrap()
    }
}
