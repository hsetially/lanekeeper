//! T3 (S2, P7, P12): server-side sessions against a real Postgres as `lanekeeper_app`: what is stored, expiry, the
//! `last_seen_at` batches, the sweep, the cache, and a revocation reaching a second app instance.
//!
//! The audit chain is the test-only `SimpleChainAudit` (plan decision B7); the event bus is the outbox-backed `PgTestBus`
//! with a `LISTEN` side, so two replicas on one database behave like two hub pods.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{DomainEvent, Role, UserStatus};
use hub_identity::session::cookie::{RandomToken, TokenHash};
use hub_identity::users::rules::Change;
use sha2::{Digest, Sha256};
use support::sessions_rig::{Replica, test_policy};
use support::users_rig::{START_MS, UsersRig, actor, uid};

const HOUR: i64 = 3600;

/// The hash of a cookie value, the way the server computes it.
fn hash_of(cookie: &hub_identity::session::store::NewSession) -> TokenHash {
    RandomToken::from_text(cookie.session.expose()).unwrap().hash()
}

/// A rig with one active viewer (user 1) and one replica, or return from the test when there is no database.
macro_rules! viewer_rig {
    () => {{
        let rig = UsersRig::new(db!()).await;
        rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
        let replica = Replica::start(&rig.db, &rig.clock).await;
        (rig, replica)
    }};
}

// ---------------------------------------------------------------------------------------------------------------
// What is stored (S2)

#[tokio::test]
async fn only_the_hash_of_the_session_id_is_stored() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let raw_session = RandomToken::from_text(session.session.expose()).unwrap();
    let raw_csrf = RandomToken::from_text(session.csrf.expose()).unwrap();
    let rows: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as("SELECT session_hash, csrf_hash FROM sessions")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (stored_session, stored_csrf) = &rows[0];
    assert_eq!(
        stored_session.as_slice(),
        raw_session.hash().as_bytes().as_slice()
    );
    assert_eq!(stored_csrf.as_slice(), raw_csrf.hash().as_bytes().as_slice());
    // Neither the cookie text nor the raw bytes appear in any column of the row.
    let raw_bytes = base64_decode(session.session.expose());
    assert_ne!(stored_session, &raw_bytes);
    assert_eq!(stored_session.as_slice(), Sha256::digest(&raw_bytes).as_slice());
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions s WHERE s::text LIKE '%' || $1 || '%' OR s::text LIKE '%' || $2 || '%'",
    )
    .bind(session.session.expose())
    .bind(session.csrf.expose())
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "the cookie text is not in the table");
    assert_ne!(
        session.session.expose(),
        session.csrf.expose(),
        "the CSRF token is a second random value"
    );
    assert_eq!(session.expires_at.unix_millis(), START_MS + 12 * HOUR * 1000);
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

#[tokio::test]
async fn two_sessions_of_one_user_are_independent() {
    let (_rig, replica) = viewer_rig!();
    let a = replica.create_session(&uid(1)).await;
    let b = replica.create_session(&uid(1)).await;
    assert_ne!(a.session.expose(), b.session.expose());
    assert!(
        replica
            .sessions
            .authenticate(a.session.expose())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        replica
            .sessions
            .authenticate(b.session.expose())
            .await
            .unwrap()
            .is_some()
    );
    assert!(replica.sessions.store().delete(&hash_of(&a)).await.unwrap());
    replica.sessions.cache().flush();
    assert!(
        replica
            .sessions
            .authenticate(a.session.expose())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        replica
            .sessions
            .authenticate(b.session.expose())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_cookie_that_is_not_a_session_is_none() {
    let (rig, replica) = viewer_rig!();
    for text in ["", "not a token", &"A".repeat(43), &"A".repeat(44)] {
        assert!(
            replica.sessions.authenticate(text).await.unwrap().is_none(),
            "{text:?}"
        );
    }
    // A well-formed token nobody issued.
    let stranger = RandomToken::generate().unwrap().to_text();
    assert!(
        replica
            .sessions
            .authenticate(stranger.expose())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(rig.session_count(&uid(1)).await, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// Expiry: 8 hours idle, 12 hours absolute (S2)

#[tokio::test]
async fn session_expires_after_8h_idle() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let cookie = session.session.expose();
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_some());
    rig.clock.advance_secs(8 * HOUR - 1);
    assert!(
        replica.sessions.authenticate(cookie).await.unwrap().is_some(),
        "one second short of 8 hours idle"
    );
    rig.clock.advance_secs(1);
    // A fresh replica has no cache; the one under test must also refuse because the cache entry is past its 30 seconds.
    assert!(
        replica.sessions.authenticate(cookie).await.unwrap().is_none(),
        "8 hours without activity ends the session"
    );
    let other = Replica::start_deaf(&rig.db, &rig.clock).await;
    assert!(other.sessions.authenticate(cookie).await.unwrap().is_none());
}

#[tokio::test]
async fn session_expires_after_12h_absolute() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let cookie = session.session.expose();
    // Busy all day: a request every 4 hours, each flushed to the database, so the idle limit is never near.
    for _ in 0..2 {
        rig.clock.advance_secs(4 * HOUR);
        assert!(replica.sessions.authenticate(cookie).await.unwrap().is_some());
        replica.sessions.touch().flush_now().await.unwrap();
    }
    rig.clock.advance_secs(4 * HOUR - 1);
    assert!(
        replica.sessions.authenticate(cookie).await.unwrap().is_some(),
        "one second short of 12 hours"
    );
    rig.clock.advance_secs(1);
    assert!(
        replica.sessions.authenticate(cookie).await.unwrap().is_none(),
        "12 hours after sign-in the session is over, however recently it was used"
    );
}

#[tokio::test]
async fn activity_extends_idle_but_not_absolute() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let cookie = session.session.expose();
    // Used after 7 hours: the idle clock restarts there.
    rig.clock.advance_secs(7 * HOUR);
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_some());
    replica.sessions.touch().flush_now().await.unwrap();
    // 11 hours after sign-in is 4 hours after the last use: alive, though 11 hours of idle-since-creation would not be.
    rig.clock.advance_secs(4 * HOUR);
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_some());
    // Past 12 hours: over, whatever the activity.
    rig.clock.advance_secs(HOUR + 1);
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_none());
}

#[tokio::test]
async fn a_session_whose_row_is_gone_is_none_once_the_cache_entry_is_old() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let cookie = session.session.expose();
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_some());
    replica.sessions.store().delete(&hash_of(&session)).await.unwrap();
    rig.clock.advance_secs(31);
    assert!(replica.sessions.authenticate(cookie).await.unwrap().is_none());
}

// ---------------------------------------------------------------------------------------------------------------
// last_seen_at is written in batches, off the request path

#[tokio::test]
async fn touch_is_batched_and_bounded() {
    let rig = UsersRig::new(db!()).await;
    let policy = hub_identity::session::SessionPolicy {
        touch_queue: 8,
        touch_batch: 5,
        ..test_policy()
    };
    let replica = Replica::with_policy(&rig.db, &rig.clock, policy).await;
    let touch = replica.sessions.touch();
    let hash = |n: u8| TokenHash::from_bytes([n; 32]);

    // 1,000 asks for one session inside the minute: one entry in the queue.
    for _ in 0..1_000 {
        touch.touch(hash(1));
    }
    let report = touch.flush_now().await.unwrap();
    assert_eq!(
        report.sessions, 1,
        "the minute-long throttle keeps the queue to one entry per session"
    );

    // After a minute the same session may be touched again.
    rig.clock.advance_secs(59);
    touch.touch(hash(1));
    assert_eq!(
        touch.flush_now().await.unwrap().sessions,
        0,
        "59 s: still inside the minute"
    );
    rig.clock.advance_secs(1);
    touch.touch(hash(1));
    assert_eq!(touch.flush_now().await.unwrap().sessions, 1, "60 s: accepted");

    // The queue holds 8: a burst of distinct sessions is cut off there, and counted.
    let before = touch.dropped();
    for n in 10..60u8 {
        touch.touch(hash(n));
    }
    assert!(
        touch.dropped() - before >= 42,
        "dropped {}",
        touch.dropped() - before
    );
    // Written in statements of at most 5 rows.
    let report = touch.flush_now().await.unwrap();
    assert!(report.sessions <= 8, "{report:?}");
    assert_eq!(report.statements, report.sessions.div_ceil(5), "{report:?}");
}

#[tokio::test]
async fn a_flush_sets_last_seen_to_the_time_of_the_touch_and_never_backwards() {
    let (rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let hash = hash_of(&session);
    let last_seen = || async {
        let ms: i64 = sqlx::query_scalar(
            "SELECT (extract(epoch from last_seen_at) * 1000)::bigint FROM sessions WHERE session_hash = $1",
        )
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&rig.pool)
        .await
        .unwrap();
        ms
    };
    assert_eq!(last_seen().await, START_MS);
    rig.clock.advance_secs(120);
    replica.sessions.touch().touch(hash);
    // The clock moves on before the flush: the row gets the time of the touch.
    rig.clock.advance_secs(30);
    replica.sessions.touch().flush_now().await.unwrap();
    assert_eq!(last_seen().await, START_MS + 120_000);
    // A late write of an older touch cannot move it back.
    rig.clock
        .set(domain::Timestamp::from_unix_millis(START_MS + 60_000));
    replica.sessions.touch().touch(hash);
    replica.sessions.touch().flush_now().await.unwrap();
    assert_eq!(last_seen().await, START_MS + 120_000);
}

// ---------------------------------------------------------------------------------------------------------------
// Sweep and logout

#[tokio::test]
async fn expired_sessions_are_swept_in_batches() {
    let (rig, replica) = viewer_rig!();
    for _ in 0..25 {
        replica.create_session(&uid(1)).await;
    }
    let live = replica.create_session(&uid(1)).await;
    // Nothing is expired yet.
    let none = replica.sessions.store().sweep_expired(10).await.unwrap();
    assert_eq!((none.deleted, none.batches), (0, 1));
    // The 26 made so far expire; the one made after the jump does not.
    let _ = live;
    rig.clock.advance_secs(12 * HOUR + 1);
    let live = replica.create_session(&uid(1)).await;
    let report = replica.sessions.store().sweep_expired(10).await.unwrap();
    assert_eq!(report.deleted, 26, "25 + the one from before the jump");
    assert_eq!(
        report.batches, 3,
        "10 + 10 + 6, the last short batch ends the sweep"
    );
    assert_eq!(rig.session_count(&uid(1)).await, 1);
    assert!(
        replica
            .sessions
            .authenticate(live.session.expose())
            .await
            .unwrap()
            .is_some()
    );
    let again = replica.sessions.store().sweep_expired(10).await.unwrap();
    assert_eq!(again.deleted, 0);
}

#[tokio::test]
async fn idempotent_logout() {
    let (_rig, replica) = viewer_rig!();
    let session = replica.create_session(&uid(1)).await;
    let hash = hash_of(&session);
    assert!(replica.sessions.store().delete(&hash).await.unwrap());
    assert!(
        !replica.sessions.store().delete(&hash).await.unwrap(),
        "a second logout finds nothing and still succeeds"
    );
    let stranger = RandomToken::generate().unwrap().hash();
    assert!(!replica.sessions.store().delete(&stranger).await.unwrap());
}

#[tokio::test]
async fn deleting_a_user_deletes_their_sessions() {
    let (rig, replica) = viewer_rig!();
    replica.create_session(&uid(1)).await;
    assert_eq!(rig.session_count(&uid(1)).await, 1);
    sqlx::query("DELETE FROM users WHERE oid = $1")
        .bind(uid(1).oid().as_str())
        .execute(&rig.pool)
        .await
        .unwrap();
    assert_eq!(rig.session_count(&uid(1)).await, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// Query plans (P7, P12)

async fn seed_many_sessions(rig: &UsersRig, users: u32, sessions_per_user: u32) {
    sqlx::query(
        "INSERT INTO users (tid, oid, email, display_name, role, status) \
         SELECT $1, '00000000-0000-4000-8000-' || lpad(to_hex(g + 1000), 12, '0'), 'u' || g || '@example.com', 'U ' || g, 'viewer', 'active' \
         FROM generate_series(1, $2::int) g",
    )
    .bind(support::users_rig::TID)
    .bind(i32::try_from(users).unwrap())
    .execute(&rig.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO sessions (session_hash, user_tid, user_oid, csrf_hash, expires_at) \
         SELECT sha256(('s' || g || '-' || k)::bytea), $1, '00000000-0000-4000-8000-' || lpad(to_hex(g + 1000), 12, '0'), \
                sha256(('c' || g || '-' || k)::bytea), now() + interval '12 hours' \
         FROM generate_series(1, $2::int) g, generate_series(1, $3::int) k",
    )
    .bind(support::users_rig::TID)
    .bind(i32::try_from(users).unwrap())
    .bind(i32::try_from(sessions_per_user).unwrap())
    .execute(&rig.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE users, sessions")
        .execute(&rig.pool)
        .await
        .unwrap();
}

/// Every `(Node Type, Relation Name or Index Name)` of an `EXPLAIN (FORMAT JSON)` plan.
fn plan_nodes(explain: &serde_json::Value) -> Vec<(String, String)> {
    fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
        let ty = node["Node Type"].as_str().unwrap_or_default().to_owned();
        let what = node["Index Name"]
            .as_str()
            .or_else(|| node["Relation Name"].as_str())
            .unwrap_or_default()
            .to_owned();
        out.push((ty, what));
        if let Some(children) = node["Plans"].as_array() {
            for c in children {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&explain[0]["Plan"], &mut out);
    out
}

#[tokio::test]
async fn explain_session_lookup_is_index_scan() {
    let (rig, replica) = viewer_rig!();
    seed_many_sessions(&rig, 2_000, 5).await;
    let probe = RandomToken::generate().unwrap().hash();
    let plan = replica.sessions.store().explain_lookup(&probe).await.unwrap();
    let nodes = plan_nodes(&plan);
    assert!(
        !nodes.iter().any(|(t, _)| t == "Seq Scan"),
        "a session lookup must not scan a table: {nodes:?}"
    );
    assert!(
        nodes
            .iter()
            .any(|(t, i)| t.contains("Index") && i == "sessions_pkey"),
        "{nodes:?}"
    );
}

#[tokio::test]
async fn session_lookup_uses_primary_key() {
    let (rig, replica) = viewer_rig!();
    seed_many_sessions(&rig, 500, 4).await;
    let session = replica.create_session(&uid(1)).await;
    let plan = replica
        .sessions
        .store()
        .explain_lookup(&hash_of(&session))
        .await
        .unwrap();
    let text = plan.to_string();
    assert!(text.contains("\"Index Name\":\"sessions_pkey\""), "{text}");
    assert!(
        text.contains("session_hash"),
        "the key is an index condition: {text}"
    );
    // And the real lookup finds exactly that row, with the user's standing.
    let row = replica
        .sessions
        .store()
        .lookup(&hash_of(&session))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.user, uid(1));
    assert_eq!(row.role, Some(Role::Viewer));
    assert_eq!(row.status, UserStatus::Active);
}

// ---------------------------------------------------------------------------------------------------------------
// A role or status change ends sessions on every replica (S2)

#[tokio::test]
async fn disabled_user_cut_off_on_other_replica_within_1s() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    // Replica A made the change; replica B serves the user. Each has its own pool, bus connection and cache.
    let a = Replica::start(&rig.db, &rig.clock).await;
    let b = Replica::start(&rig.db, &rig.clock).await;
    // A replica that never subscribed, for contrast.
    let deaf = Replica::start_deaf(&rig.db, &rig.clock).await;
    let session = a.create_session(&uid(1)).await;
    let cookie = session.session.expose();
    for replica in [&a, &b, &deaf] {
        let got = replica.sessions.authenticate(cookie).await.unwrap().unwrap();
        assert_eq!(
            got.status,
            UserStatus::Active,
            "warm: every replica has the session cached"
        );
    }

    // The admin disables the user through replica A's services.
    let admin = admin_on(&rig, &a);
    let started = Instant::now();
    admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    b.wait_for_events(1, Duration::from_secs(1)).await;
    assert!(
        b.sessions.authenticate(cookie).await.unwrap().is_none(),
        "replica B no longer serves the session"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "cut off in {:?}",
        started.elapsed()
    );
    a.wait_for_events(1, Duration::from_secs(1)).await;
    assert!(a.sessions.authenticate(cookie).await.unwrap().is_none());

    // Without the event the cache would have kept serving it: the 30-second limit is the only bound there.
    assert!(
        deaf.sessions.authenticate(cookie).await.unwrap().is_some(),
        "a replica that is not subscribed serves the old entry until its time-to-live ends"
    );
    rig.clock.advance_secs(31);
    assert!(deaf.sessions.authenticate(cookie).await.unwrap().is_none());
}

fn admin_on(rig: &UsersRig, replica: &Replica) -> hub_identity::users::UserAdmin {
    let svc = hub_identity::users::UserServices {
        pool: replica.pool.clone(),
        bus: Arc::clone(&replica.bus) as Arc<_>,
        ..rig.svc.clone()
    };
    hub_identity::users::UserAdmin::new(svc)
}

#[tokio::test]
async fn a_role_change_reaches_the_other_replica_and_its_cached_role() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    let a = Replica::start(&rig.db, &rig.clock).await;
    let b = Replica::start(&rig.db, &rig.clock).await;
    let session = a.create_session(&uid(1)).await;
    assert_eq!(
        b.sessions
            .authenticate(session.session.expose())
            .await
            .unwrap()
            .unwrap()
            .role,
        Some(Role::Viewer)
    );
    admin_on(&rig, &a)
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    b.wait_for_events(1, Duration::from_secs(1)).await;
    // Sessions end with a role change (S2): the user signs in again and gets the new role.
    assert!(
        b.sessions
            .authenticate(session.session.expose())
            .await
            .unwrap()
            .is_none()
    );
    let fresh = a.create_session(&uid(1)).await;
    assert_eq!(
        b.sessions
            .authenticate(fresh.session.expose())
            .await
            .unwrap()
            .unwrap()
            .role,
        Some(Role::Editor)
    );
}

#[tokio::test]
async fn another_users_sessions_stay_cached_when_one_user_is_revoked() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    rig.seed(2, Some(Role::Viewer), UserStatus::Active).await;
    let a = Replica::start(&rig.db, &rig.clock).await;
    let b = Replica::start(&rig.db, &rig.clock).await;
    let one = a.create_session(&uid(1)).await;
    let two = a.create_session(&uid(2)).await;
    for s in [&one, &two] {
        assert!(
            b.sessions
                .authenticate(s.session.expose())
                .await
                .unwrap()
                .is_some()
        );
    }
    admin_on(&rig, &a)
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                status: Some(UserStatus::Disabled),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    b.wait_for_events(1, Duration::from_secs(1)).await;
    assert!(
        b.sessions
            .authenticate(one.session.expose())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        b.sessions
            .authenticate(two.session.expose())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn the_revocation_event_is_published_with_the_change_and_not_without_it() {
    let rig = UsersRig::new(db!()).await;
    rig.seed_admin(9).await;
    rig.seed(1, Some(Role::Viewer), UserStatus::Active).await;
    let bus = Arc::clone(&rig.bus);
    // A change that changes nothing (the role the user already has) publishes nothing.
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Viewer)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert!(bus.published().await.is_empty());
    // A role change publishes one revocation for that user.
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                role: Some(Some(Role::Editor)),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        bus.published().await,
        vec![DomainEvent::UserSessionsRevoked { user: uid(1) }]
    );
    // The approval flag alone does not end sessions, so it publishes nothing more.
    rig.admin
        .update_user(
            &actor(9),
            &uid(1),
            Change {
                requires_approval: Some(true),
                ..Change::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(bus.published().await.len(), 1);
}
