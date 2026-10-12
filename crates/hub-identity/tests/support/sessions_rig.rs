//! Hub replicas for the session tests: each has its own pool, event bus connection, session cache and revocation listener,
//! and they share one database and one clock. Two of them are the two app instances of the invalidation test.

use std::sync::Arc;
use std::time::Duration;

use domain::UserId;
use hub_identity::session::revocation::RevocationListener;
use hub_identity::session::store::NewSession;
use hub_identity::session::{SessionPolicy, Sessions};
use hub_identity::{Clock, ManualClock};
use ports::EventBus;
use sqlx::PgPool;

use super::pg::TestDb;
use super::pg_bus::PgTestBus;

/// A policy for tests: the product's limits, and a flush interval so long that only `flush_now` ever writes.
pub fn test_policy() -> SessionPolicy {
    SessionPolicy {
        touch_flush: Duration::from_secs(3600),
        ..SessionPolicy::default()
    }
}

pub struct Replica {
    pub pool: PgPool,
    pub bus: Arc<PgTestBus>,
    pub sessions: Arc<Sessions>,
    /// `None` for a replica that never subscribed to revocations.
    pub listener: Option<RevocationListener>,
}

impl Replica {
    /// A replica that listens for revocations on the shared database.
    pub async fn start(db: &TestDb, clock: &Arc<ManualClock>) -> Self {
        Self::build(db, clock, test_policy(), true).await
    }

    /// A replica that does not: its cache is only as fresh as its 30-second limit.
    pub async fn start_deaf(db: &TestDb, clock: &Arc<ManualClock>) -> Self {
        Self::build(db, clock, test_policy(), false).await
    }

    pub async fn with_policy(db: &TestDb, clock: &Arc<ManualClock>, policy: SessionPolicy) -> Self {
        Self::build(db, clock, policy, true).await
    }

    async fn build(db: &TestDb, clock: &Arc<ManualClock>, policy: SessionPolicy, listen: bool) -> Self {
        let pool = db.app_pool(8).await;
        let bus = Arc::new(PgTestBus::listening(pool.clone()).await);
        let sessions = Arc::new(Sessions::new(
            pool.clone(),
            Arc::clone(clock) as Arc<dyn Clock>,
            &policy,
        ));
        let listener = listen.then(|| {
            RevocationListener::spawn(
                Arc::clone(&bus) as Arc<dyn EventBus>,
                Arc::clone(sessions.cache()),
            )
        });
        Self {
            pool,
            bus,
            sessions,
            listener,
        }
    }

    /// Create a session for `user` in its own transaction, as the sign-in does.
    pub async fn create_session(&self, user: &UserId) -> NewSession {
        let mut tx = self.pool.begin().await.unwrap();
        let session = self.sessions.store().create_in(&mut tx, user).await.unwrap();
        tx.commit().await.unwrap();
        session
    }

    /// Wait until the listener has applied `n` revocation events, or fail after `limit`.
    pub async fn wait_for_events(&self, n: u64, limit: Duration) {
        let listener = self.listener.as_ref().expect("this replica listens");
        tokio::time::timeout(limit, listener.wait_for(|s| s.events >= n))
            .await
            .expect("the revocation event reached this replica in time");
    }
}
