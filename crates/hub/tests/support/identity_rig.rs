//! The hub composed from the real identity module, for the tests of plan 18, T4 (S1, S2, S3, S4, S9, P7, P13).
//!
//! Everything under test is real: `Identity::build` (sessions, the revocation and user-cache listeners, the Entra sign-in, the
//! users, the audit chain's jobs), `PgEventBus`, `PgAuditLog`, `PgLeases`, the platform's event stream and `Hub::build` with its
//! `AuthLayer` and `CsrfLayer`, over a real Postgres 16. Software appears only at the edges `Edges` names: a mock Entra (identity's
//! own, served through its fake HTTPS client), secrets in memory, a KMS with software keys, an in-memory checkpoint bucket and a
//! clock the test moves.
//!
//! A test that needs the whole composition includes this file with `#[path]` and declares, at the root of its crate, the modules
//! it names (`support`, `tls_pg`, `secrets`, `clock`, `soft_kms`, `fake_https`, `mock_oidc`, `sse_client`).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use domain::{Guid, Role, UserId, UserStatus};
use http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use hub::client_ip::PeerAddr;
use hub::config::{CheckpointSettings, IdentitySettings, LimitSettings};
use hub::edges::{Edges, IdentityEdges};
use hub::identity::{AuditSchedule, Identity, IdentityInputs, IdentityMounts};
use hub::notify::UnavailableNotifier;
use hub::static_files::StaticFiles;
use hub::wiring::Tasks;
use hub::{Deps, EventsModule, Hub, HubSettings, Module};
use hub_identity::audit::{MemoryBucket, PgAuditLog};
use hub_identity::config::{ClientAuth, IdentityConfig};
use hub_platform::ca::profile::PeerName;
use hub_platform::events::bus::{BusSettings, PgEventBus};
use hub_platform::events::metrics::EventMetrics;
use hub_platform::events::sse::{SseSettings, SseState};
use hub_platform::events::visibility::RoleVisibility;
use hub_platform::leases::PgLeases;
use ports::fakes::FakeTokenVerifier;
use ports::{AuditLog, KeyRef, Leases};
use sqlx::PgPool;
use tokio::sync::watch;
use tower::ServiceExt;

use crate::clock::ManualClock;
use crate::mock_oidc::{self, CLIENT_SECRET, MockOidc, ORIGIN, TID};
use crate::secrets::MemorySecrets;
use crate::soft_kms::SoftKms;
use crate::support::pki::TestCa;
use crate::tls_pg::{self, TlsPg};

/// The key version that signs the checkpoints of the tests.
pub const CHECKPOINT_KEY: &str = "projects/p/locations/l/keyRings/r/cryptoKeys/audit/cryptoKeyVersions/1";
/// The Entra object id of the person the mock identity provider signs in; the first admin of the rig.
pub use crate::mock_oidc::OID as FIRST_ADMIN_OID;

/// What differs between the tests.
#[derive(Clone)]
pub struct Options {
    pub limits: LimitSettings,
    pub sse: SseSettings,
    pub schedule: AuditSchedule,
    /// Modules mounted beside the platform's event stream (the other tasks add theirs here).
    pub modules: Vec<Arc<dyn Module>>,
    /// Serve the built web app on the fallback, as the real hub does.
    pub static_files: bool,
    /// Replicas on the one database.
    pub replicas: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limits: LimitSettings::default(),
            // A short beat, so that a test that waits for a stream to end does not wait 15 seconds.
            sse: SseSettings {
                heartbeat: Duration::from_millis(200),
                ..SseSettings::default()
            },
            schedule: AuditSchedule::default(),
            modules: Vec::new(),
            static_files: true,
            replicas: 1,
        }
    }
}

/// One hub process's worth of the composition.
pub struct Replica {
    pub name: String,
    pub hub: Hub,
    pub identity: Identity,
    pub bus: PgEventBus,
    pub pool: PgPool,
    /// Turns true to stop the replica's tasks.
    pub stop: watch::Sender<bool>,
    tasks: Option<Tasks>,
    _dist: tempfile::TempDir,
}

impl Replica {
    /// The web router, every layer applied.
    pub fn web(&self) -> &Router {
        &self.hub.web
    }

    /// Stop the replica's tasks and wait for them, as the hub's shutdown does.
    pub async fn shut_down(&mut self) {
        self.stop.send_replace(true);
        if let Some(tasks) = self.tasks.take() {
            let _ = tasks.finish(Duration::from_secs(10)).await;
        }
    }
}

/// A database and the replicas on it.
pub struct IdentityRig {
    pub pg: TlsPg,
    pub idp: Arc<MockOidc>,
    pub clock: Arc<ManualClock>,
    pub secrets: Arc<MemorySecrets>,
    pub kms: Arc<SoftKms>,
    /// The bucket every replica puts its checkpoints in, as the real ones share one.
    pub checkpoints: Arc<MemoryBucket>,
    pub settings: IdentitySettings,
    pub replicas: Vec<Replica>,
    /// Every secret value the sign-in flows made (the ID token, the sealed login cookie, the state and the nonce), so that a test can
    /// search the logs for them.
    pub seen: std::sync::Mutex<Vec<String>>,
}

/// The settings of the identity group, for the mock identity provider: its tenant and client, the rig's origin, and the object id of
/// the first admin.
pub fn identity_settings() -> IdentitySettings {
    let mut entra = mock_oidc::config(ClientAuth::Secret {
        name: "entra-client-secret".to_owned(),
    });
    ORIGIN.clone_into(&mut entra.public_origin);
    IdentitySettings {
        config: IdentityConfig {
            entra,
            bootstrap_admins: vec![Guid::parse(FIRST_ADMIN_OID).unwrap()],
        },
        checkpoint: CheckpointSettings {
            bucket: "audit-checkpoints".to_owned(),
            key: KeyRef::parse(CHECKPOINT_KEY).unwrap(),
            older_keys: Vec::new(),
        },
    }
}

impl IdentityRig {
    /// A migrated database and `options.replicas` replicas on it, or `None` when Docker is not there and not required.
    pub async fn start(options: Options) -> Option<Self> {
        let ca = TestCa::new("lanekeeper-test-ca");
        let pg = tls_pg::start(&ca).await?;
        pg.migrate().await;
        let settings = identity_settings();
        let secrets = Arc::new(MemorySecrets::default());
        secrets.put("entra-client-secret", CLIENT_SECRET);
        secrets.put("login-cookie-key", &URL_SAFE_NO_PAD.encode([5u8; 32]));
        let key = KeyRef::parse(CHECKPOINT_KEY).unwrap();
        let mut rig = Self {
            pg,
            idp: MockOidc::new(),
            clock: ManualClock::at_t0(),
            secrets,
            kms: Arc::new(SoftKms::new(&[&key])),
            checkpoints: Arc::new(MemoryBucket::new()),
            settings,
            replicas: Vec::new(),
            seen: std::sync::Mutex::default(),
        };
        for n in 0..options.replicas.max(1) {
            let replica = rig.build_replica(&format!("hub-{n}"), &options).await;
            rig.replicas.push(replica);
        }
        Some(rig)
    }

    /// The first replica, which most tests use alone.
    pub fn one(&self) -> &Replica {
        &self.replicas[0]
    }

    pub fn app(&self) -> &Router {
        &self.replicas[0].hub.web
    }

    /// The pool every replica's data is in, for the assertions that look at the tables.
    pub fn pool(&self) -> &PgPool {
        &self.replicas[0].pool
    }

    async fn build_replica(&self, name: &str, options: &Options) -> Replica {
        let pool = self.pg.app_pool_sized(8).await;
        let (stop, closing) = watch::channel(false);
        let event_metrics = EventMetrics::default();
        let audit: Arc<dyn AuditLog> = Arc::new(PgAuditLog::new());
        let leases: Arc<dyn Leases> = Arc::new(PgLeases::new(pool.clone(), &PeerName::parse(name).unwrap()));
        let bus = PgEventBus::new(pool.clone(), BusSettings::default(), event_metrics.clone());
        let bus_tasks = bus.start(closing.clone()).await;
        let mut tasks = Tasks::new(closing.clone());
        tasks.adopt("event-listener", bus_tasks.listener);
        tasks.adopt("event-relay", bus_tasks.relay);

        let edges = Edges::new(
            self.secrets.clone(),
            self.clock.clone(),
            self.kms.clone(),
            self.kms.clone(),
            Arc::new(FakeTokenVerifier::new()),
            IdentityEdges {
                entra: self.idp.https(),
                google_keys: Arc::new(hub::testing::RefusingHttps),
                checkpoints: self.checkpoints.clone(),
            },
        );
        let (identity, IdentityMounts { session, mounted }) = Identity::build(
            IdentityInputs {
                settings: &self.settings,
                pool: pool.clone(),
                edges: &edges,
                audit,
                leases,
                bus: Arc::new(bus.clone()),
                notifier: Arc::new(UnavailableNotifier),
                closing: closing.clone(),
                start_timeout: Duration::from_secs(20),
                schedule: options.schedule,
            },
            &mut tasks,
        )
        .await
        .expect("the identity module is built");

        let sse = SseState::new(
            bus.source(),
            identity.auth.clone(),
            Arc::new(RoleVisibility),
            options.sse.clone(),
            closing,
            event_metrics,
        );
        let mut modules: Vec<Arc<dyn Module>> = vec![Arc::new(EventsModule::new(sse))];
        modules.extend(options.modules.iter().cloned());

        let dist = crate::support::write_dist();
        let static_files = options
            .static_files
            .then(|| Arc::new(StaticFiles::open(dist.path()).unwrap()));
        let settings = HubSettings {
            limits: options.limits.clone(),
            ..HubSettings::default()
        };
        let hub = Hub::build(Deps {
            settings,
            session,
            modules,
            mounted: vec![mounted],
            static_files,
            checks: Vec::new(),
        })
        .expect("the hub is composed");
        hub.metrics.register(|registry| {
            registry.register_collector(Box::new(hub::metrics_bridge::SourceCollector::new(
                identity.metrics.clone() as Arc<dyn hub_identity::metrics::CounterSource>,
            )));
        });
        Replica {
            name: name.to_owned(),
            hub,
            identity,
            bus,
            pool,
            stop,
            tasks: Some(tasks),
            _dist: dist,
        }
    }

    /// Build the identity module alone over these secrets, for the tests of what stops it from starting. Nothing is kept.
    pub async fn try_identity(
        &self,
        secrets: Arc<MemorySecrets>,
    ) -> Result<(), hub::identity::IdentityBuildError> {
        let pool = self.pg.app_pool_sized(4).await;
        let (_stop, closing) = watch::channel(false);
        let bus = PgEventBus::new(pool.clone(), BusSettings::default(), EventMetrics::default());
        let mut tasks = Tasks::new(closing.clone());
        let edges = Edges::new(
            secrets,
            self.clock.clone(),
            self.kms.clone(),
            self.kms.clone(),
            Arc::new(FakeTokenVerifier::new()),
            IdentityEdges {
                entra: self.idp.https(),
                google_keys: Arc::new(hub::testing::RefusingHttps),
                checkpoints: self.checkpoints.clone(),
            },
        );
        let leases: Arc<dyn Leases> =
            Arc::new(PgLeases::new(pool.clone(), &PeerName::parse("hub-x").unwrap()));
        Identity::build(
            IdentityInputs {
                settings: &self.settings,
                pool,
                edges: &edges,
                audit: Arc::new(PgAuditLog::new()),
                leases,
                bus: Arc::new(bus),
                notifier: Arc::new(UnavailableNotifier),
                closing,
                start_timeout: Duration::from_secs(10),
                schedule: AuditSchedule::default(),
            },
            &mut tasks,
        )
        .await
        .map(|_| ())
    }

    /// Insert a user the way sign-in would have, with the role and status given. Returns the id.
    pub async fn add_user(&self, n: u32, role: Option<Role>, status: UserStatus) -> UserId {
        let id = user_id(n);
        sqlx::query(
            "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id.tid().as_str())
        .bind(id.oid().as_str())
        .bind(format!("user{n}@example.com"))
        .bind(format!("User {n}"))
        .bind(role.map(Role::as_str))
        .bind(status.as_str())
        .execute(self.pool())
        .await
        .unwrap();
        id
    }

    /// A session for `user`, made the way sign-in makes one, with its two cookies.
    pub async fn session_for(&self, user: &UserId) -> Cookies {
        let sessions = &self.one().identity.sessions;
        let mut tx = self.pool().begin().await.unwrap();
        let made = sessions.store().create_in(&mut tx, user).await.unwrap();
        tx.commit().await.unwrap();
        Cookies {
            session: made.session.expose().clone(),
            csrf: made.csrf.expose().clone(),
        }
    }

    /// A user and a session for them, in one step.
    pub async fn signed_in(&self, n: u32, role: Option<Role>, status: UserStatus) -> (UserId, Cookies) {
        let id = self.add_user(n, role, status).await;
        let cookies = self.session_for(&id).await;
        (id, cookies)
    }

    /// Sign in through the real flow on `app`: `/auth/login`, the mock identity provider, `/auth/callback`. `oid` is the Entra object
    /// id the provider vouches for. Returns the reply of the callback and, when it signed in, the cookies it set.
    pub async fn sign_in_on(
        &self,
        app: &Router,
        oid: &str,
        presented: Option<&Cookies>,
    ) -> (Reply, Option<Cookies>) {
        let login = send(app, Req::get("/auth/login?return_to=%2Fswimlanes")).await;
        assert_eq!(login.status, StatusCode::FOUND, "{}", login.text());
        let location = login.header("location").unwrap().to_owned();
        let query: std::collections::HashMap<String, String> = url::Url::parse(&location)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let login_cookie = login.cookie("__Host-lk_login").expect("the sealed login cookie");
        let id_token = mock_oidc::TokenSpec::valid(&query["nonce"])
            .claim("oid", serde_json::json!(oid))
            .claim("email", serde_json::json!(format!("{oid}@example.com")))
            .mint();
        self.seen.lock().unwrap().extend([
            id_token.clone(),
            login_cookie.clone(),
            query["state"].clone(),
            query["nonce"].clone(),
        ]);
        self.idp.set_id_token(id_token);
        let mut callback = Req::get(&format!(
            "/auth/callback?code=the-auth-code&state={}",
            query["state"]
        ))
        .cookie("__Host-lk_login", &login_cookie);
        if let Some(cookies) = presented {
            callback = callback.cookie("__Host-lk_session", &cookies.session);
        }
        let reply = send(app, callback).await;
        let cookies = reply
            .cookie("__Host-lk_session")
            .zip(reply.cookie("__Host-lk_csrf"))
            .map(|(session, csrf)| Cookies { session, csrf });
        (reply, cookies)
    }
}

/// The user `n` of the rig: tenant of the mock identity provider, an object id made from `n`.
pub fn user_id(n: u32) -> UserId {
    UserId::new(
        Guid::parse(TID).unwrap(),
        Guid::parse(&format!("00000000-0000-4000-8000-{n:012x}")).unwrap(),
    )
}

/// A session cookie that is well formed (a fresh 256-bit token) and names no session.
pub fn unknown_session_cookie() -> String {
    hub_identity::session::cookie::RandomToken::generate()
        .unwrap()
        .to_text()
        .expose()
        .clone()
}

/// A signed-in browser's two cookies.
#[derive(Debug, Clone)]
pub struct Cookies {
    pub session: String,
    pub csrf: String,
}

/// A request to build. By default from 198.51.100.1 with no cookie.
#[derive(Debug, Clone)]
pub struct Req {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    cookies: Vec<(String, String)>,
    body: Option<String>,
    ip: [u8; 4],
}

impl Req {
    pub fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            headers: Vec::new(),
            cookies: Vec::new(),
            body: None,
            ip: [198, 51, 100, 1],
        }
    }

    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: &str) -> Self {
        Self::new(Method::POST, path)
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.push((name.to_owned(), value.to_owned()));
        self
    }

    /// The session cookie and the CSRF cookie, but not the header or `Origin`: what a browser sends of its own accord.
    pub fn session(self, cookies: &Cookies) -> Self {
        self.cookie("__Host-lk_session", &cookies.session)
            .cookie("__Host-lk_csrf", &cookies.csrf)
    }

    /// A page's state-changing request: the cookies, the token in the header, and the right `Origin`.
    pub fn with_token(self, cookies: &Cookies) -> Self {
        self.session(cookies)
            .header("x-csrf-token", &cookies.csrf)
            .header("origin", ORIGIN)
    }

    pub fn json(mut self, body: &serde_json::Value) -> Self {
        self.body = Some(body.to_string());
        self.header("content-type", "application/json")
    }

    /// A body as it is, for a test that needs one of an exact size or one that is not JSON. The content type is the caller's.
    pub fn raw_body(mut self, body: String) -> Self {
        self.body = Some(body);
        self
    }

    pub fn from(mut self, ip: [u8; 4]) -> Self {
        self.ip = ip;
        self
    }

    pub fn build(&self) -> Request<Body> {
        let mut builder = Request::builder().method(self.method.clone()).uri(&self.path);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if !self.cookies.is_empty() {
            let line = self
                .cookies
                .iter()
                .map(|(n, v)| format!("{n}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            builder = builder.header(header::COOKIE, line);
        }
        let mut request = builder
            .body(self.body.clone().map_or_else(Body::empty, Body::from))
            .unwrap();
        request
            .extensions_mut()
            .insert(PeerAddr(SocketAddr::from((self.ip, 40000))));
        request
    }
}

/// What came back.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Reply {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Every `Set-Cookie` line.
    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .collect()
    }

    /// The value of the cookie `name` that this reply sets, if it sets one with a value.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.set_cookies().into_iter().find_map(|line| {
            let (pair, _) = line.split_once(';').unwrap_or((&line, ""));
            let (n, v) = pair.split_once('=')?;
            (n == name && !v.is_empty()).then(|| v.to_owned())
        })
    }

    /// The `Set-Cookie` line of the cookie `name`, whole.
    pub fn cookie_line(&self, name: &str) -> Option<String> {
        self.set_cookies()
            .into_iter()
            .find(|line| line.starts_with(&format!("{name}=")))
    }

    /// What kind of problem a problem+json body is: its `code`, or, when the hub has normalised the body to the standard members
    /// (as it does for every error), the last segment of its `type` (`urn:lanekeeper:problem:csrf` is `csrf`).
    pub fn problem_code(&self) -> Option<String> {
        let body = self.json();
        body["code"]
            .as_str()
            .or_else(|| body["type"].as_str().and_then(|t| t.rsplit(':').next()))
            .map(str::to_owned)
    }
}

/// Send `req` through `app` and read the whole reply (not for streams).
pub async fn send(app: &Router, req: Req) -> Reply {
    let response = app.clone().oneshot(req.build()).await.unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes(),
    }
}

/// Send `req` and return the response with its body unread: for a stream.
pub async fn open(app: &Router, req: Req) -> http::Response<Body> {
    app.clone().oneshot(req.build()).await.unwrap()
}

/// A path template made concrete: every `{name}` becomes `x1`.
pub fn concrete(path: &str) -> String {
    path.split('/')
        .map(|segment| if segment.starts_with('{') { "x1" } else { segment })
        .collect::<Vec<_>>()
        .join("/")
}

/// Poll `check` every 20 ms until it is true, for at most `within`. Returns how long it took.
pub async fn eventually<F, Fut>(within: Duration, what: &str, mut check: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    loop {
        if check().await {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
