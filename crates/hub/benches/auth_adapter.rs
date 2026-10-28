//! P7: what authentication adds to a request through the composed hub (plan 18, T4, CC1): `P7.auth_composed`, `hub/auth_composed_warm`,
//! and `P7.auth_composed_cold`, `hub/auth_composed_cold`. The budget is the 2 ms that `docs/performance.md` states for the whole of
//! authentication, p95.
//!
//! `hub-identity/auth_middleware` measures identity's two layers alone. This measures the path a request really takes in the hub:
//! the hub's own layers, then the hub-wide `AuthLayer`, then `CsrfLayer`, then the platform's guard asking the `CookieAuth` adapter
//! (the session's role and status from the `AuthContext`, the user's profile from the user cache), then a handler that does nothing.
//! It runs over the real `Identity` module and a real Postgres (a container, or the server in `LK_TEST_PG_ADMIN_URL`), with 100
//! signed-in users, as at design scale, taken in turn. The request is the expensive kind: a state-changing request with the session
//! cookie, the CSRF cookie and header, and `Origin`.
//!
//! - `auth_composed_warm`: both caches are warm, the normal case.
//! - `auth_composed_cold`: the session cache and the user cache are emptied before each request, so the request waits for the two
//!   indexed reads (the session joined with its user, and the user's profile).
//! - `noauth_baseline` (not a budget): the same hub, the same route and the same handler with a `SessionAuth` that reads a header and
//!   no session layers. What the two above cost over it is what authentication adds; the registered figures are the whole request,
//!   which is the safe side of that number.
//!
//! All three run on loopback to a local database, so they bound the code and the database's time and not the network's; prompt 16
//! measures staging.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::similar_names,
    clippy::doc_markdown
)]

#[path = "../../hub-platform/tests/support/clock.rs"]
mod clock;
#[path = "../../hub-identity/tests/support/fake_https.rs"]
mod fake_https;
#[path = "../../hub-identity/tests/support/mock_oidc.rs"]
mod mock_oidc;
#[path = "../tests/support/identity_rig.rs"]
mod rig;
#[path = "../tests/support/secrets.rs"]
mod secrets;
#[path = "../examples/support/soft_kms.rs"]
mod soft_kms;
#[path = "../tests/support/mod.rs"]
mod support;
#[path = "../tests/support/tls_pg.rs"]
mod tls_pg;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use domain::{Role, UserStatus};
use http::{Method, StatusCode};
use hub::config::{LimitSettings, Rate};
use hub::{Deps, Hub, HubSettings, Module, SessionEdge};
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use rig::{Cookies, IdentityRig, Options, Req, send};

/// Design scale: 100 web users (docs/performance.md).
const USERS: u32 = 100;
const PATH: &str = "/api/v1/bench/change";

/// One state-changing route behind the guard, whose handler does nothing.
struct BenchModule;

impl Module for BenchModule {
    fn name(&self) -> &'static str {
        "bench"
    }

    fn mount(&self, router: GuardedRouter) -> GuardedRouter {
        router.route(&Method::POST, PATH, Role::Editor, || async {
            StatusCode::NO_CONTENT
        })
    }
}

/// Limits that never bind, so that the request is not refused for coming fast from one address. The limiters still run.
fn open_limits() -> LimitSettings {
    let never = Rate {
        per_second: 1.0e9,
        burst: u32::MAX,
    };
    LimitSettings {
        per_ip: never,
        per_user: never,
        ..LimitSettings::default()
    }
}

struct Fixture {
    rig: IdentityRig,
    /// The session and CSRF cookies of each user.
    cookies: Vec<Cookies>,
    /// The same route and handler with no authentication to speak of.
    baseline: Router,
}

async fn fixture() -> Fixture {
    let Some(rig) = IdentityRig::start(Options {
        limits: open_limits(),
        modules: vec![Arc::new(BenchModule)],
        static_files: false,
        ..Options::default()
    })
    .await
    else {
        panic!("the auth benchmark needs a database: start Docker or set LK_TEST_PG_ADMIN_URL");
    };
    let mut cookies = Vec::new();
    for n in 1..=USERS {
        let (_, made) = rig.signed_in(n, Some(Role::Editor), UserStatus::Active).await;
        cookies.push(made);
    }
    let baseline = Hub::build(Deps {
        settings: HubSettings {
            limits: open_limits(),
            ..HubSettings::default()
        },
        session: SessionEdge::without_sessions(Arc::new(FakeSessionAuth::new().with_user(
            "editor",
            Some(Role::Editor),
            UserStatus::Active,
        ))),
        mounted: Vec::new(),
        modules: vec![Arc::new(BenchModule)],
        static_files: None,
        checks: Vec::new(),
    })
    .unwrap()
    .web;
    Fixture {
        rig,
        cookies,
        baseline,
    }
}

/// One request through everything; the handler answers 204 only if every layer and the guard let it through.
async fn request(app: &Router, cookies: &Cookies) {
    let reply = send(app, Req::post(PATH).with_token(cookies)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text());
    black_box(reply);
}

fn auth_composed(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let f = rt.block_on(fixture());
    let app = f.rig.app().clone();
    let identity = &f.rig.one().identity;
    // Warm both caches for every user, as a minute of traffic would.
    for cookies in &f.cookies {
        rt.block_on(request(&app, cookies));
    }

    let mut group = c.benchmark_group("hub");
    group.sample_size(100).measurement_time(Duration::from_secs(5));
    let mut next = 0usize;
    group.bench_function("auth_composed_warm", |b| {
        b.iter(|| {
            next = (next + 1) % f.cookies.len();
            rt.block_on(request(&app, &f.cookies[next]));
        });
    });

    let mut next = 0usize;
    group.bench_function("auth_composed_cold", |b| {
        b.iter_batched(
            || {
                // Not timed: forget every cached session and every cached user, so the request reads both rows.
                identity.sessions.cache().flush();
                identity.user_cache.flush();
                next = (next + 1) % f.cookies.len();
                next
            },
            |i| rt.block_on(request(&app, &f.cookies[i])),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("noauth_baseline", |b| {
        b.iter(|| {
            let reply = rt.block_on(send(
                &f.baseline,
                Req::post(PATH).header(TEST_USER_HEADER, "editor"),
            ));
            assert_eq!(reply.status, StatusCode::NO_CONTENT);
            black_box(reply);
        });
    });
    group.finish();
    drop(app);
    // Inside the runtime: dropping the container outside one panics in `testcontainers` (`hub-identity/tests/pg_teardown.rs`).
    rt.block_on(async move { drop(f) });
}

criterion_group!(benches, auth_composed);
criterion_main!(benches);
