//! People with sessions, made the way sign-in makes them (plan 18, T8): a `users` row and a `sessions` row in the database the hub
//! reads, and the two cookies a browser would hold. The hub finds the session in the table on its first request, as it does for one
//! that another replica made. Used by `full.rs` (the hub started in the test) and by the start-up benchmark (a request burst).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use domain::{Guid, Role, UserId, UserStatus};
use hub::edges::IdentityClock;
use hub_identity::session::{SessionPolicy, Sessions};
use hub_platform::clock::SystemClock;
use sqlx::PgPool;

/// The tenant of the rig's Entra settings, which the users of the tests belong to, and the `Origin` of a page (`LK_PUBLIC_ORIGIN`).
pub const TENANT: &str = "11111111-1111-4111-8111-111111111111";
pub const ORIGIN: &str = "https://localhost";

/// A signed-in browser's two cookies.
#[derive(Debug, Clone)]
pub struct Cookies {
    pub session: String,
    pub csrf: String,
}

impl Cookies {
    /// The `Cookie` header of a page.
    pub fn header(&self) -> String {
        format!("__Host-lk_session={}; __Host-lk_csrf={}", self.session, self.csrf)
    }
}

/// Makes users and sessions over the hub's own database role.
pub struct Browser {
    pool: PgPool,
    sessions: Arc<Sessions>,
}

impl Browser {
    pub fn new(pool: PgPool) -> Self {
        let sessions = Arc::new(Sessions::new(
            pool.clone(),
            Arc::new(IdentityClock(Arc::new(SystemClock))),
            &SessionPolicy::default(),
        ));
        Self { pool, sessions }
    }

    /// A user the way sign-in would have made one, and a session for them.
    pub async fn signed_in(&self, n: u32, role: Option<Role>, status: UserStatus) -> (UserId, Cookies) {
        let id = UserId::new(
            Guid::parse(TENANT).unwrap(),
            Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
        );
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT DO NOTHING",
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
        let mut tx = self.pool.begin().await.unwrap();
        let made = self.sessions.store().create_in(&mut tx, &id).await.unwrap();
        tx.commit().await.unwrap();
        (
            id,
            Cookies {
                session: made.session.expose().clone(),
                csrf: made.csrf.expose().clone(),
            },
        )
    }
}
