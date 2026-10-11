//! The whole identity component behind `GuardedRouter`, wired to the mock Entra: the routes of T3 and T4, the layers, and
//! (optionally) a real Postgres. `lazy_app` needs no database: its pool never connects, which is enough for every request
//! that carries no cookie (the auth layer reads the database only for a presented session).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use domain::{Role, Timestamp, UserId};
use hub_identity::guard::{GuardedRouter, RouteEntry};
use hub_identity::oidc::login::OidcLogin;
use hub_identity::routes::auth::AuthState;
use hub_identity::routes::{IdentityState, identity_routes};
use hub_identity::session::store::NewSession;
use hub_identity::session::{SessionPolicy, Sessions};
use hub_identity::users::onboarding::PgGithubTokenPresence;
use hub_identity::users::{AccessNotifier, PgUsers, UserAdmin, UserServices};
use hub_identity::{Clock, ManualClock};
use ports::fakes::{FakeAuditLog, FakeEventBus, FakeNotifier};
use ports::{AuditLog, EventBus};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;

use super::mock_oidc::{MockOidc, NOW_MS, config, secret_auth, secrets};
use super::web_rig::{Answer, ORIGIN, Req, RoutesRig, send};

/// A pool that never connects until it is used.
pub fn lazy_pool() -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(300))
        .connect_lazy_with(
            PgConnectOptions::new()
                .host("127.0.0.1")
                .port(1)
                .username("nobody"),
        )
}

/// Everything the identity routes need, on `pool`.
pub async fn identity_guarded(
    pool: PgPool,
    sessions: Arc<Sessions>,
    audit: Arc<dyn AuditLog>,
    bus: Arc<dyn EventBus>,
    clock: &Arc<ManualClock>,
) -> GuardedRouter {
    clock.set(Timestamp::from_unix_millis(NOW_MS));
    let clock_dyn: Arc<dyn Clock> = Arc::clone(clock) as Arc<dyn Clock>;
    let mock = MockOidc::new();
    let login = OidcLogin::new(
        config(secret_auth()),
        mock.https() as Arc<_>,
        secrets(),
        Arc::clone(&clock_dyn),
    )
    .await
    .unwrap();
    let svc = UserServices {
        pool: pool.clone(),
        audit: Arc::clone(&audit),
        bus: Arc::clone(&bus),
        notifier: AccessNotifier::new(Arc::new(FakeNotifier::new())),
        clock: Arc::clone(&clock_dyn),
    };
    let users = PgUsers::new(svc.clone(), BTreeSet::new());
    let admin = UserAdmin::new(svc);
    let auth = AuthState::new(
        login,
        users.clone(),
        Arc::clone(&sessions),
        pool.clone(),
        audit,
        bus,
        clock_dyn,
    );
    let ids = IdentityState::new(users, admin, Arc::new(PgGithubTokenPresence::new(pool)));
    identity_routes(GuardedRouter::new(sessions, ORIGIN), auth, ids)
}

/// The identity router with no database behind it, and its route table.
pub async fn lazy_app() -> (Router, Vec<RouteEntry>) {
    let pool = lazy_pool();
    let clock = Arc::new(ManualClock::at(Timestamp::from_unix_millis(NOW_MS)));
    let sessions = Arc::new(Sessions::new(
        pool.clone(),
        Arc::clone(&clock) as Arc<dyn Clock>,
        &SessionPolicy::default(),
    ));
    let guarded = identity_guarded(
        pool,
        sessions,
        Arc::new(FakeAuditLog::new()),
        Arc::new(FakeEventBus::default()),
        &clock,
    )
    .await;
    let table = guarded.route_table().to_vec();
    (guarded.into_router().unwrap(), table)
}

/// The identity router on a real database: a rig with a replica, plus the guarded app and its table.
pub struct App {
    pub rig: RoutesRig,
    pub router: Router,
    pub table: Vec<RouteEntry>,
}

impl App {
    pub async fn new(db: super::pg::TestDb) -> Self {
        let rig = RoutesRig::new(db).await;
        let guarded = identity_guarded(
            rig.replica.pool.clone(),
            Arc::clone(&rig.replica.sessions),
            Arc::new(super::simple_audit::SimpleChainAudit),
            Arc::clone(&rig.replica.bus) as Arc<dyn EventBus>,
            &rig.users.clock,
        )
        .await;
        let table = guarded.route_table().to_vec();
        Self {
            router: guarded.into_router().unwrap(),
            table,
            rig,
        }
    }

    /// Seed a user and give them a session.
    pub async fn user(&self, n: u32, role: Option<Role>, status: domain::UserStatus) -> (UserId, NewSession) {
        let id = self.rig.users.seed(n, role, status).await;
        let session = self.rig.replica.create_session(&id).await;
        (id, session)
    }

    pub async fn send(&self, req: Req) -> Answer {
        send(&self.router, req).await
    }

    /// A request as the browser of `session` would send it.
    #[allow(clippy::unused_self)]
    pub fn as_user(&self, method: Method, path: &str, s: &NewSession) -> Req {
        Req::new(method, path).with_session(s.session.expose(), s.csrf.expose())
    }
}

/// A request with a JSON body, sent as the browser of `session` would.
pub async fn send_json(
    router: &Router,
    method: Method,
    path: &str,
    s: Option<&NewSession>,
    body: &str,
) -> Answer {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(s) = s {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-lk_session={}; __Host-lk_csrf={}",
                    s.session.expose(),
                    s.csrf.expose()
                ),
            )
            .header("x-csrf-token", s.csrf.expose().as_str())
            .header(header::ORIGIN, ORIGIN);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 256 * 1024).await.unwrap();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The `type` of a problem answer, without the URN prefix.
pub fn problem_kind(a: &Answer) -> String {
    a.json()["type"]
        .as_str()
        .and_then(|t| t.strip_prefix("urn:lanekeeper:problem:"))
        .unwrap_or_default()
        .to_owned()
}

pub fn assert_problem(a: &Answer, status: StatusCode, kind: &str) {
    assert_eq!(a.status, status, "{}", a.body);
    assert_eq!(
        a.headers.get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap()),
        Some("application/problem+json"),
        "{}",
        a.body
    );
    assert_eq!(problem_kind(a), kind, "{}", a.body);
}
