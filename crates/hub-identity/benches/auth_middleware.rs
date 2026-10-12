//! P7: authentication adds under 2 ms to a request (`P7.auth_middleware`, `hub-identity/auth_middleware`, and
//! `P7.auth_middleware_cold`, `hub-identity/auth_middleware_cold`).
//!
//! The real `AuthLayer` and `CsrfLayer` in a tower service in front of a handler that does nothing, over a real Postgres
//! (a container, or the server in `LK_TEST_PG_ADMIN_URL`), with 100 signed-in users, as at design scale, taken in turn. The
//! request is the expensive kind: a state-changing request with a session cookie, the CSRF cookie and header, and `Origin`,
//! so the parse of two cookies, the hash, the cache read, the idle and expiry checks, the touch and the CSRF comparisons are
//! all in the sample.
//!
//! - `auth_middleware`: the session cache is warm, the normal case (a session is looked up once in 30 seconds).
//! - `auth_middleware_cold`: the cache is emptied before each request, so the request waits for the one indexed read.
//!
//! Both run on loopback to a local database, so they bound the code and the database's time and not the network's; prompt 16
//! measures staging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/mod.rs"]
mod support;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use domain::{Role, UserStatus};
use hub_identity::session::{SessionPolicy, Sessions};
use hub_identity::{Clock, SystemClock};
use support::pg::TestDb;
use support::web_rig::{Req, app};
use tower::ServiceExt;

/// Design scale: 100 web users (docs/performance.md).
const USERS: u32 = 100;

struct Fixture {
    // Keeps the container (if any) and the pool alive; torn down by `db.shutdown()` inside the runtime, at the end.
    db: TestDb,
    sessions: Arc<Sessions>,
    app: Router,
    /// `(session cookie, csrf cookie)` of each user.
    cookies: Vec<(String, String)>,
}

fn user(n: u32) -> domain::UserId {
    support::users_rig::uid(n)
}

async fn fixture() -> Fixture {
    let Some(db) = TestDb::ready().await else {
        panic!("the auth benchmark needs a database: start Docker or set LK_TEST_PG_ADMIN_URL");
    };
    let pool = db.app_pool(8).await;
    for n in 1..=USERS {
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(user(n).tid().as_str())
        .bind(user(n).oid().as_str())
        .bind(format!("user{n}@example.com"))
        .bind(format!("User {n}"))
        .bind(Role::Editor.as_str())
        .bind(UserStatus::Active.as_str())
        .execute(&pool)
        .await
        .unwrap();
    }
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let sessions = Arc::new(Sessions::new(
        pool.clone(),
        Arc::clone(&clock),
        &SessionPolicy::default(),
    ));
    let mut cookies = Vec::new();
    for n in 1..=USERS {
        let mut tx = pool.begin().await.unwrap();
        let s = sessions.store().create_in(&mut tx, &user(n)).await.unwrap();
        tx.commit().await.unwrap();
        cookies.push((s.session.expose().clone(), s.csrf.expose().clone()));
    }
    let app = app(&sessions);
    Fixture {
        db,
        sessions,
        app,
        cookies,
    }
}

/// One request through the layers; the handler answers 204 only if every layer let it through.
async fn request(app: &Router, cookies: &(String, String)) {
    let req = Req::post("/change").with_session(&cookies.0, &cookies.1).build();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    black_box(response);
}

fn auth_middleware(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let f = rt.block_on(fixture());
    // Warm the cache for every user, as a minute of traffic would.
    for cookies in &f.cookies {
        rt.block_on(request(&f.app, cookies));
    }

    let mut group = c.benchmark_group("hub-identity");
    group.sample_size(100).measurement_time(Duration::from_secs(5));
    let mut next = 0usize;
    group.bench_function("auth_middleware", |b| {
        b.iter(|| {
            next = (next + 1) % f.cookies.len();
            rt.block_on(request(&f.app, &f.cookies[next]));
        });
    });

    let mut next = 0usize;
    group.bench_function("auth_middleware_cold", |b| {
        b.iter_batched(
            || {
                // Not timed: forget every cached session, so the request has to read its row.
                f.sessions.cache().flush();
                next = (next + 1) % f.cookies.len();
                next
            },
            |i| rt.block_on(request(&f.app, &f.cookies[i])),
            BatchSize::PerIteration,
        );
    });
    group.finish();
    // Inside the runtime: dropping the container outside one panics in `testcontainers` (`tests/pg_teardown.rs`).
    rt.block_on(f.db.shutdown());
}

criterion_group!(benches, auth_middleware);
criterion_main!(benches);
