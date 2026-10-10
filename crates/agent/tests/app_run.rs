//! The whole agent (T7): what a connection sets going, what the probes say, what the metrics count, and how it stops.
//!
//! The agent runs as `agent::app::App` against the fake hub (real TLS 1.3 and gRPC over in-memory pipes), the fake
//! Kubernetes API server and a scripted file system, in virtual time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use agent::app::AppError;
use agent::ops::NotReady;
use domain::{
    AgentReply, ClusterReport, ContentHash, Expected, HubCommand, NfsPath, OpError, RequestId, ServiceRef,
};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use support::app_rig::{AppRig, Setup};
use support::fake_kube::{FakeKube, Kind};
use support::k8s_objects::{deployment, pod};
use support::scripted_source::ScriptedSource;
use tokio::time::sleep;

const NS: &str = "sit1";

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn with_cluster() -> (Setup, FakeKube) {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "web")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .build(),
    );
    kube.apply(pod(NS, "web-1", "web", "2026-10-10T11:00:00Z"));
    let setup = Setup {
        kube: Some(kube.clone()),
        ..Setup::default()
    };
    (setup, kube)
}

fn cluster_reports(conn: &support::fake_hub::ConnHandle) -> Vec<ClusterReport> {
    conn.received()
        .into_iter()
        .filter_map(|m| match m {
            FromAgent::Cluster(report) => Some(report),
            _ => None,
        })
        .collect()
}

fn allow_ttl(app: &AppRig) {
    app.rig.server.set_config(pb::AgentConfig {
        scan_interval_secs: 10,
        heartbeat_interval_secs: 10,
        max_file_bytes: 0,
        deny_globs: Vec::new(),
        env_allowlist: vec!["CONFIG_CLIENT_CACHE_TTL".to_owned()],
        tenants: vec!["sit1".to_owned()],
    });
}

#[tokio::test(start_paused = true)]
async fn readyz_only_while_connected() {
    let mut app = AppRig::start(Setup::default()).await;
    // Before the hub has been reached, and while it is being reached, the agent is not ready.
    assert!(matches!(
        app.health.ready(),
        Err(NotReady::NoSession | NotReady::Connecting)
    ));
    app.connected().await;
    app.wait_until("ready", AppRig::is_ready).await;

    // The hub goes away: not ready for as long as it is gone.
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    app.wait_until("not ready", |a| !a.is_ready()).await;
    sleep(Duration::from_secs(20)).await;
    assert!(!app.is_ready());

    // It comes back: ready again.
    app.rig.server.net.set_reachable(true);
    app.wait_until("ready again", AppRig::is_ready).await;

    // Shutting down: not ready from the moment it begins.
    app.stop().await.unwrap();
    assert_eq!(app.health.ready(), Err(NotReady::ShuttingDown));
}

#[tokio::test(start_paused = true)]
async fn the_watchers_start_when_the_hubs_configuration_arrives_and_list_once() {
    let (setup, kube) = with_cluster();
    let app = AppRig::start_on(support::rig::Rig::new(), setup).await;
    // A hub that never sends its configuration: the agent keeps trying, and has not touched the cluster.
    app.rig.server.set_send_config(false);
    sleep(Duration::from_secs(100)).await;
    assert!(
        kube.calls()
            .iter()
            .all(|c| !c.path.contains("/deployments") && !c.path.contains("/pods")),
        "the watchers must wait for the hub's configuration"
    );
    allow_ttl(&app);
    app.rig.server.set_send_config(true);
    // The agent's next attempt (within a minute) gets the configuration.
    sleep(Duration::from_secs(90)).await;
    let conn = app.rig.server.connection(app.rig.server.connection_count() - 1);
    conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;

    // The allowlist was known when the watchers started, so each Deployment list was made once.
    let lists = kube
        .calls()
        .into_iter()
        .filter(|c| {
            c.method == "GET"
                && c.path == FakeKube::collection_path(Kind::Deployment, NS)
                && c.query_param("watch").as_deref() != Some("true")
        })
        .count();
    assert_eq!(
        lists, 1,
        "one list of the Deployments, not one per change of the allowlist"
    );
}

#[tokio::test(start_paused = true)]
async fn a_connection_sends_the_full_report_first_and_then_the_changes() {
    let (setup, kube) = with_cluster();
    let app = AppRig::start(setup).await;
    allow_ttl(&app);
    let conn = app.connected().await;
    conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;

    let reports = cluster_reports(&conn);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].full);
    let web = &reports[0].deployments[0];
    assert_eq!(web.service.name(), "web");
    assert_eq!(web.env_values[0].name.as_str(), "CONFIG_CLIENT_CACHE_TTL");

    kube.apply(pod(NS, "web-2", "web", "2026-10-10T12:00:00Z"));
    conn.wait_for(|m| matches!(m, FromAgent::Cluster(r) if !r.full))
        .await;
    let reports = cluster_reports(&conn);
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[1].deployments[0].pods.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn after_a_reconnect_the_full_report_replaces_what_was_queued_meanwhile() {
    let (setup, kube) = with_cluster();
    let app = AppRig::start(setup).await;
    let first = app.connected().await;
    first.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;

    // The hub is gone; the cluster changes three times meanwhile.
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    app.wait_until("not ready", |a| !a.is_ready()).await;
    for i in 2..5 {
        kube.apply(pod(NS, &format!("web-{i}"), "web", "2026-10-10T12:00:00Z"));
        sleep(Duration::from_secs(2)).await;
    }
    app.rig.server.net.set_reachable(true);
    let second = app.rig.server.wait_for_connection(2).await;
    second.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;
    sleep(Duration::from_secs(10)).await;

    let reports = cluster_reports(&second);
    assert!(
        reports[0].full,
        "the first report on the new connection is the full one"
    );
    assert_eq!(reports[0].deployments[0].pods.len(), 4);
    assert!(
        reports
            .iter()
            .skip(1)
            .all(|r| r.deployments.iter().all(|d| d.pods.len() == 4)),
        "no stale delta follows the full report: {reports:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn commands_reach_the_file_operations_and_the_cluster() {
    let (setup, kube) = with_cluster();
    let app = AppRig::start(setup).await;
    let conn = app.connected().await;
    conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;

    std::fs::create_dir_all(app.dir.path().join("svc")).unwrap();
    std::fs::write(app.dir.path().join("svc/a.yml"), b"a: 1\r\n").unwrap();
    let path = NfsPath::parse("svc/a.yml").unwrap();

    conn.send(ToAgent::Command(HubCommand::ReadFile {
        request_id: rid("read-1"),
        path: path.clone(),
    }))
    .await;
    let read = conn
        .wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::File { request_id, .. }) if request_id.as_str() == "read-1"))
        .await;
    let FromAgent::Reply(AgentReply::File { bytes, .. }) = read else {
        unreachable!()
    };
    assert_eq!(&bytes[..], b"a: 1\r\n");

    // A write with the wrong hash is a conflict and changes nothing.
    conn.send(ToAgent::Command(HubCommand::WriteFile {
        request_id: rid("write-1"),
        path: path.clone(),
        expected: Expected::Hash {
            hash: ContentHash::from_bytes([9; 32]),
        },
        bytes: bytes::Bytes::from_static(b"a: 2\r\n"),
    }))
    .await;
    let write = conn
        .wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == "write-1"))
        .await;
    assert!(matches!(write, FromAgent::Reply(AgentReply::Op(r)) if r.error == Some(OpError::Conflict)));
    assert_eq!(
        std::fs::read(app.dir.path().join("svc/a.yml")).unwrap(),
        b"a: 1\r\n"
    );

    // A restart patches the Deployment.
    conn.send(ToAgent::Command(HubCommand::RestartDeployment {
        request_id: rid("restart-1"),
        service: ServiceRef::new(NS, "web").unwrap(),
    }))
    .await;
    let restart = conn
        .wait_for(
            |m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == "restart-1"),
        )
        .await;
    assert!(matches!(restart, FromAgent::Reply(AgentReply::Op(r)) if r.ok));
    assert!(
        kube.calls()
            .iter()
            .any(|c| c.method == "PATCH" && c.path.ends_with("/deployments/web")),
        "{:#?}",
        kube.calls()
    );

    // And the cluster report on request.
    conn.send(ToAgent::Command(HubCommand::RequestClusterReport {
        request_id: rid("report-1"),
    }))
    .await;
    conn.wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Cluster { request_id, .. }) if request_id.as_str() == "report-1"))
        .await;
}

#[tokio::test(start_paused = true)]
async fn without_a_kubernetes_client_cluster_commands_are_unsupported_and_nothing_is_reported() {
    let app = AppRig::start(Setup::default()).await;
    let conn = app.connected().await;
    conn.send(ToAgent::Command(HubCommand::RequestClusterReport {
        request_id: rid("report-1"),
    }))
    .await;
    let answer = conn
        .wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == "report-1"))
        .await;
    assert!(matches!(answer, FromAgent::Reply(AgentReply::Op(r)) if r.error == Some(OpError::Unsupported)));
    sleep(Duration::from_secs(30)).await;
    assert!(cluster_reports(&conn).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_later_config_with_another_allowlist_changes_what_is_reported() {
    let (setup, _kube) = with_cluster();
    let app = AppRig::start(setup).await;
    let conn = app.connected().await;
    conn.wait_for(|m| matches!(m, FromAgent::Cluster(_))).await;
    assert!(cluster_reports(&conn)[0].deployments[0].env_values.is_empty());

    conn.send(ToAgent::Config(
        pb::AgentConfig {
            scan_interval_secs: 10,
            heartbeat_interval_secs: 10,
            max_file_bytes: 0,
            deny_globs: Vec::new(),
            env_allowlist: vec!["CONFIG_CLIENT_CACHE_TTL".to_owned()],
            tenants: vec!["sit1".to_owned()],
        }
        .try_into()
        .unwrap(),
    ))
    .await;
    conn.send(ToAgent::Command(HubCommand::RequestClusterReport {
        request_id: rid("report-2"),
    }))
    .await;
    let answer = conn
        .wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Cluster { request_id, .. }) if request_id.as_str() == "report-2"))
        .await;
    let FromAgent::Reply(AgentReply::Cluster { report, .. }) = answer else {
        unreachable!()
    };
    assert_eq!(report.deployments[0].env_values.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_closes_the_stream_in_an_orderly_way_and_stops_being_ready() {
    let mut app = AppRig::start(Setup::default()).await;
    let conn = app.connected().await;
    conn.send(ToAgent::Command(HubCommand::ReadFile {
        request_id: rid("read-1"),
        path: NfsPath::parse("svc/missing.yml").unwrap(),
    }))
    .await;
    conn.wait_for(|m| matches!(m, FromAgent::Reply(AgentReply::Op(r)) if r.request_id.as_str() == "read-1"))
        .await;
    app.stop().await.unwrap();
    conn.wait_closed().await;
    assert_eq!(app.health.ready(), Err(NotReady::ShuttingDown));
}

#[tokio::test(start_paused = true)]
async fn shutdown_while_the_hub_is_unreachable_still_exits() {
    let mut app = AppRig::start(Setup::default()).await;
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    sleep(Duration::from_secs(30)).await;
    app.stop().await.unwrap();
}

/// A scan that panics, standing in for a bug: the agent must not go on pretending.
#[derive(Debug)]
struct PanicsOnSecondScan {
    inner: ScriptedSource,
}

#[async_trait::async_trait]
impl agent::tree::TreeSource for PanicsOnSecondScan {
    async fn scan(
        &self,
        previous: Option<agent::tree::MerkleTree>,
        mode: agent::tree::ScanMode,
    ) -> Result<agent::tree::ScanOutcome, agent::tree::ScanError> {
        assert!(previous.is_none(), "the scanner has a bug");
        self.inner.scan(previous, mode).await
    }

    async fn refresh(&self, files: Vec<agent::tree::RefreshRequest>) -> Vec<agent::tree::Refreshed> {
        self.inner.refresh(files).await
    }

    async fn read(
        &self,
        files: Vec<agent::tree::ReadRequest>,
    ) -> Vec<Result<agent::tree::FileRead, agent::tree::ReadError>> {
        self.inner.read(files).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_loop_that_panics_ends_the_agent_with_an_error() {
    let inner = ScriptedSource::new();
    inner.write("svc/a.yml", b"a: 1\n");
    let mut app = AppRig::start(Setup {
        source: Some(std::sync::Arc::new(PanicsOnSecondScan { inner })),
        ..Setup::default()
    })
    .await;
    app.connected().await;
    let ended = app.ended().await;
    assert!(matches!(ended, Err(AppError::LoopEnded(_))), "{ended:?}");
    assert!(app.health.ready().is_err());
}

/// A source whose second walk never finishes: a stale mount that hangs.
#[derive(Debug)]
struct HangsOnSecondScan {
    inner: ScriptedSource,
    scans: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl agent::tree::TreeSource for HangsOnSecondScan {
    async fn scan(
        &self,
        previous: Option<agent::tree::MerkleTree>,
        mode: agent::tree::ScanMode,
    ) -> Result<agent::tree::ScanOutcome, agent::tree::ScanError> {
        if self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1 {
            sleep(Duration::from_secs(24 * 3600)).await;
        }
        self.inner.scan(previous, mode).await
    }

    async fn refresh(&self, files: Vec<agent::tree::RefreshRequest>) -> Vec<agent::tree::Refreshed> {
        self.inner.refresh(files).await
    }

    async fn read(
        &self,
        files: Vec<agent::tree::ReadRequest>,
    ) -> Vec<Result<agent::tree::FileRead, agent::tree::ReadError>> {
        self.inner.read(files).await
    }
}

#[tokio::test(start_paused = true)]
async fn healthz_fails_when_the_scanner_stops_making_progress_and_only_then() {
    let inner = ScriptedSource::new();
    inner.write("svc/a.yml", b"a: 1\n");
    let app = AppRig::start(Setup {
        source: Some(std::sync::Arc::new(HangsOnSecondScan {
            inner,
            scans: std::sync::atomic::AtomicU32::new(0),
        })),
        ..Setup::default()
    })
    .await;
    app.connected().await;
    assert!(app.health.unhealthy().is_empty());

    // The walk hangs. The hub being connected or not changes nothing; ten minutes of silence are still allowed.
    sleep(Duration::from_secs(10 * 60)).await;
    assert!(app.health.unhealthy().is_empty(), "{:?}", app.health.unhealthy());
    sleep(Duration::from_secs(2 * 60)).await;
    assert_eq!(app.health.unhealthy(), vec!["scanner progress"]);
    // Everything else is still alive, so the liveness probe names exactly the loop that is stuck.
    assert!(app.is_ready(), "the session is fine");
}

#[tokio::test(start_paused = true)]
async fn healthz_stays_ok_across_a_long_hub_outage() {
    let app = AppRig::start(Setup::default()).await;
    app.connected().await;
    app.rig.server.net.set_reachable(false);
    app.rig.server.net.kill_connections();
    sleep(Duration::from_secs(6 * 3600)).await;
    assert!(app.health.unhealthy().is_empty(), "{:?}", app.health.unhealthy());
    assert!(!app.is_ready());
}
