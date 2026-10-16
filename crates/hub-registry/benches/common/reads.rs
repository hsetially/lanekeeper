//! The bed of the read benchmarks (T6): the pipeline's bed (40 swimlanes of about 2,000 files, 60 tenant branches, a real Postgres)
//! with what the reads find in a running hub: every swimlane computed once by the pipeline (states, findings, effective hashes), the
//! settings index current (the sweeper has run), the real `PgBlobStore` with its cache in front of the `blobs` table, and the read
//! service behind the router of `registry_routes`, called with `oneshot` as a client's request would be (the guard, the handler, the
//! JSON and the body are all inside the number).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Body;
use domain::{Role, UserStatus};
use http::{Method, Request};
use http_body_util::BodyExt;
use hub_platform::blobs::metrics::BlobMetrics;
use hub_platform::blobs::store::{BlobSettings, PgBlobStore};
use hub_platform::{FakeSessionAuth, GuardedRouter, TEST_USER_HEADER};
use hub_registry::{
    ReadDeps, RegistryApi, RegistryConfig, RegistryMetrics, RegistryService, SettingsSweeper, registry_routes,
};
use ports::fakes::FakeAgentGateway;
use tower::ServiceExt;

use super::Bed;

/// One request, measured.
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub ms: f64,
    pub status: u16,
    pub bytes: usize,
}

pub struct ReadBed {
    pub bed: Bed,
    pub service: RegistryService,
    pub blobs: Arc<PgBlobStore>,
    pub gateway: FakeAgentGateway,
    pub config: RegistryConfig,
    router: Router,
}

impl ReadBed {
    /// The bed with `lanes` swimlanes ingested, computed and indexed, or `None` when there is no database.
    pub async fn new(lanes: usize, config: RegistryConfig) -> Option<Self> {
        let started = Instant::now();
        let bed = Bed::new(lanes, config.clone()).await?;
        let report = bed.pipeline.refresh_all().await.unwrap();
        assert_eq!(report.lanes, lanes, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        eprintln!(
            "read bed: the pipeline has computed {lanes} swimlanes ({:.1} s)",
            started.elapsed().as_secs_f64()
        );
        let (indexer, _worker) = bed
            .rig
            .another_indexer(config.clone(), RegistryMetrics::default());
        let swept = SettingsSweeper::new(indexer).sweep(|| Ok(())).await.unwrap();
        eprintln!(
            "read bed: the settings index is current ({} blobs, {:.1} s since the start)",
            swept.found,
            started.elapsed().as_secs_f64()
        );
        // A hub in steady state has had its tables analysed by autovacuum; a database that was loaded a minute ago has not, and the
        // first analyze comes whenever the launcher next wakes (once a minute), usually after the bed is built. Until it does, the
        // planner takes the tables for nearly empty: the dense settings searches (`%name%`) were planned as a trigram scan and a sort
        // of about 100,000 rows, 110 to 160 ms a statement instead of 3 to 30 ms, and the rule of `steady_state_queries` (no statement
        // over 50 ms) failed in two runs out of two on this head, with `settings_index` shown as never analysed. The bed therefore
        // does what autovacuum would have done, once, as the owner of the tables (the app role may not analyse them).
        let mut admin = bed.rig.db.admin().await;
        sqlx::raw_sql("ANALYZE").execute(&mut admin).await.unwrap();
        eprintln!(
            "read bed: the tables are analysed ({:.1} s since the start)",
            started.elapsed().as_secs_f64()
        );
        let gateway = FakeAgentGateway::default();
        let (service, blobs, router) = Self::wire(&bed, &gateway, &config);
        Some(Self {
            bed,
            service,
            blobs,
            gateway,
            config,
            router,
        })
    }

    fn wire(
        bed: &Bed,
        gateway: &FakeAgentGateway,
        config: &RegistryConfig,
    ) -> (RegistryService, Arc<PgBlobStore>, Router) {
        let blobs = Arc::new(PgBlobStore::new(
            bed.rig.pool.clone(),
            BlobSettings::default(),
            BlobMetrics::default(),
        ));
        let service = RegistryService::new(ReadDeps {
            pool: bed.rig.pool.clone(),
            blobs: Arc::clone(&blobs) as _,
            git: Arc::new(bed.git.clone()),
            gateway: Arc::new(gateway.clone()),
            clock: Arc::clone(&bed.rig.clock) as _,
            config: config.clone(),
            metrics: RegistryMetrics::default(),
        })
        .unwrap();
        let auth =
            Arc::new(FakeSessionAuth::new().with_user("viewer", Some(Role::Viewer), UserStatus::Active));
        let api: Arc<dyn RegistryApi> = Arc::new(service.clone());
        let (router, _) = registry_routes(GuardedRouter::new(auth, None), api)
            .into_parts()
            .unwrap();
        (service, blobs, router)
    }

    /// A service and a blob store that have nothing cached, as after a restart of the hub (the database is as warm as it was).
    pub fn restart(&mut self) {
        let (service, blobs, router) = Self::wire(&self.bed, &self.gateway, &self.config);
        self.service = service;
        self.blobs = blobs;
        self.router = router;
    }

    /// The JSON of a 200 from a service of its own that has nothing cached and a blob store that has nothing cached, over the same database:
    /// what a request gives on this data when nothing is reused. The reference that an answer from reused state is held to.
    pub async fn json_from_scratch(&self, uri: &str) -> serde_json::Value {
        let (_service, _blobs, router) = Self::wire(&self.bed, &self.gateway, &self.config);
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(TEST_USER_HEADER, "viewer")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "{uri}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }

    /// A viewer's `GET`, measured from the request to the last byte of the body.
    pub async fn get(&self, uri: &str) -> Reading {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(TEST_USER_HEADER, "viewer")
            .body(Body::empty())
            .unwrap();
        let started = Instant::now();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        Reading {
            ms: started.elapsed().as_secs_f64() * 1000.0,
            status,
            bytes: body.len(),
        }
    }

    /// As [`Self::get`], and the answer must be a 200.
    pub async fn ok(&self, uri: &str) -> Reading {
        let r = self.get(uri).await;
        assert_eq!(r.status, 200, "{uri}");
        r
    }

    /// The JSON of a 200 and how long the request took, from the request to the last byte of the body.
    pub async fn get_json_timed(&self, uri: &str) -> (f64, serde_json::Value) {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(TEST_USER_HEADER, "viewer")
            .body(Body::empty())
            .unwrap();
        let started = Instant::now();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(status, 200, "{uri}");
        (ms, serde_json::from_slice(&body).unwrap())
    }

    /// The JSON of a 200.
    pub async fn json(&self, uri: &str) -> serde_json::Value {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(TEST_USER_HEADER, "viewer")
            .body(Body::empty())
            .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 200, "{uri}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
    }
}

/// `p50`, `p95` and `max` of `samples` for a line of the log.
pub fn summary(samples: &[f64]) -> String {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    format!(
        "n={} p50={:.1} ms p95={:.1} ms max={:.1} ms",
        sorted.len(),
        super::percentile(&sorted, 0.5),
        super::percentile(&sorted, 0.95),
        sorted.last().copied().unwrap_or(0.0)
    )
}
