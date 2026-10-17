//! The read API as a client sees it: the real router of `registry_routes` behind hub-platform's guard, over the real service and a real
//! Postgres, called with `oneshot`. Users are the guard's `FakeSessionAuth` keys: `viewer`, `editor`, `admin`, and the three that must be
//! refused, `pending`, `disabled` and `norole`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use domain::{Role, UserStatus};
use http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use hub_platform::{FakeSessionAuth, GuardedRouter, RouteSpec, TEST_USER_HEADER};
use hub_registry::{ReadDeps, RegistryApi, RegistryService, registry_routes};
use tower::ServiceExt;

use super::reads::Reads;

/// Every user the tests use, by key.
pub fn auth() -> Arc<FakeSessionAuth> {
    Arc::new(
        FakeSessionAuth::new()
            .with_user("viewer", Some(Role::Viewer), UserStatus::Active)
            .with_user("editor", Some(Role::Editor), UserStatus::Active)
            .with_user("admin", Some(Role::Admin), UserStatus::Active)
            .with_user("pending", None, UserStatus::Pending)
            .with_user("disabled", Some(Role::Viewer), UserStatus::Disabled)
            .with_user("norole", None, UserStatus::Active),
    )
}

/// An empty guarded router over the test users.
pub fn guarded() -> GuardedRouter {
    GuardedRouter::new(auth(), None)
}

/// The router the hub would mount for the registry, and its table.
pub fn router_of(api: Arc<dyn RegistryApi>) -> (Router, Vec<RouteSpec>) {
    let guarded = registry_routes(GuardedRouter::new(auth(), None), api);
    guarded.into_parts().unwrap()
}

/// A service that has no database behind it (the pool never connects): for tests that only look at the route table.
pub fn service_without_a_database() -> RegistryService {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://nobody:none@127.0.0.1:1/none")
        .unwrap();
    RegistryService::new(ReadDeps {
        pool,
        blobs: Arc::new(ports::fakes::FakeBlobStore::new()),
        git: Arc::new(ports::fakes::FakeGit::new()),
        gateway: Arc::new(ports::fakes::FakeAgentGateway::default()),
        clock: Arc::new(hub_registry::SystemClock),
        config: hub_registry::RegistryConfig::default(),
        metrics: hub_registry::RegistryMetrics::default(),
    })
    .unwrap()
}

/// A value the engine flags as a secret (a GitHub token), planted in `sit1`'s `core.yml` as `token`.
pub const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

pub struct Http {
    pub reads: Reads,
    router: Router,
    pub specs: Vec<RouteSpec>,
}

/// What came back.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Reply {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The `code` of a problem body.
    pub fn code(&self) -> String {
        self.json()["code"].as_str().unwrap_or_default().to_owned()
    }
}

impl Http {
    pub async fn new(tenants: &[&str]) -> Option<Self> {
        Some(Self::over(Reads::new(tenants).await?))
    }

    pub fn over(reads: Reads) -> Self {
        let (router, specs) = router_of(Arc::new(reads.service.clone()));
        Self { reads, router, specs }
    }

    /// `GET uri` as the user with this key (`None`: no session at all).
    pub async fn get(&self, user: Option<&str>, uri: &str) -> Reply {
        self.get_with(user, uri, &[]).await
    }

    pub async fn get_with(&self, user: Option<&str>, uri: &str, headers: &[(&str, &str)]) -> Reply {
        let mut request = Request::builder().method(Method::GET).uri(uri);
        if let Some(user) = user {
            request = request.header(TEST_USER_HEADER, user);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let body = body.collect().await.unwrap().to_bytes();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }

    /// Three swimlanes that run an application `core`, with files of every kind a read can meet: `sit1` (tenants `sit1` and `sit2`, a
    /// tenant file, a text file, a denied key), `sit2` and `sit3`. The index is current and the pipeline has run for all three, so the
    /// derived tables (effective hashes, drift) are there.
    pub async fn seeded() -> Option<Self> {
        let mut reads = Reads::new(&["sit1", "sit2"]).await?;
        for id in ["sit2", "sit3"] {
            reads.world.rig.add_swimlane(id, &[id], false).await;
        }
        for id in ["sit1", "sit2", "sit3"] {
            reads
                .sql(&format!(
                    "INSERT INTO services (swimlane_id, namespace, name, env_values) \
                     VALUES ('{id}', 'ns', 'core', '{{\"SPRING_APPLICATION_NAME\": \"core\"}}')"
                ))
                .await;
        }
        reads
            .world
            .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
        reads
            .world
            .inventory(&[
                ("application.yml", "common: base\nport: 1\n"),
                ("core.yml", &format!("port: 2\nname: core\ntoken: {SECRET}\n")),
                ("core-sit1.yml", "name: sit1-core\n"),
                ("notes.txt", "hello timeout world\n"),
                ("crlf.txt", "one\ntwo\n"),
                ("blob.bin", "\u{0}\u{1}binary one"),
                ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nMIIB-planted\n"),
            ])
            .await;
        // `notes.txt` has two versions, so that its history has a second page.
        reads
            .world
            .nfs_change(&[("notes.txt", "hello timeout world, again\n")])
            .await;
        reads
            .world
            .second_lane(
                "sit2",
                &["sit2"],
                &[
                    ("core.yml", "port: 3\nname: core\n"),
                    ("notes.txt", "other\n"),
                    ("crlf.txt", "one\r\ntwo\r\n"),
                    ("blob.bin", "\u{0}\u{1}binary two"),
                ],
            )
            .await;
        reads
            .world
            .second_lane("sit3", &["sit3"], &[("core.yml", "port: 2\nname: core\n")])
            .await;
        reads
            .sql(
                "INSERT INTO service_state (service_id, pickup_state, pending_since, live_by, pending_files) \
                 SELECT id, 'live_within_ttl', now() - interval '1 minute', now() + interval '10 minutes', '[\"core.yml\"]' \
                 FROM services WHERE swimlane_id = 'sit1' AND name = 'core'; \
                 INSERT INTO services (swimlane_id, namespace, name) VALUES ('sit1', 'ns', 'api'); \
                 INSERT INTO service_state (service_id, pickup_state, pending_since, live_by, pending_files) \
                 SELECT id, 'live_within_ttl', now() - interval '2 minutes', now() + interval '5 minutes', '[\"core.yml\"]' \
                 FROM services WHERE swimlane_id = 'sit1' AND name = 'api'",
            )
            .await;
        reads.index_all().await;
        let lanes = ["sit1", "sit2", "sit3"]
            .iter()
            .map(|id| (domain::SwimlaneId::parse(id).unwrap(), hub_registry::Work::Whole))
            .collect();
        reads.world.pipeline.process(lanes).await.unwrap();
        Some(Self::over(reads))
    }

    /// A viewer's `GET`.
    pub async fn view(&self, uri: &str) -> Reply {
        self.get(Some("viewer"), uri).await
    }
}
