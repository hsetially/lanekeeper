//! When the config-server pod started (T12, C11): the config-server computes its search locations once, at startup, so
//! the hub needs to know when each pod started to tell that a folder created later is not served yet (D83, D85).
//!
//! The agent reports the start time of the oldest pod of the configured Deployment (`LK_CONFIG_SERVER_DEPLOYMENT`). Oldest,
//! because a replica that started before the folder was made does not serve it, and the report must not hide it behind a
//! newer one.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use agent::config::{DeploymentRef, KubeName};
use agent::kube::helm::HelmFilter;
use agent::kube::jobs::JobMatcher;
use agent::kube::{ClusterWatcher, WatchConfig};
use domain::{ClusterReport, Timestamp};
use support::fake_kube::{FakeKube, Kind};
use support::k8s_objects::{deployment, pod, terminating_pod};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

const NS: &str = "sit1";
const SERVER: &str = "csp-configuration-server";
/// 2026-10-10 10:00:00, 11:00:00 and 12:30:00 UTC.
const T10: i64 = 1_791_626_400_000;
const T11: i64 = 1_791_630_000_000;
const T1230: i64 = 1_791_635_400_000;

fn config(config_server: Option<&str>) -> WatchConfig {
    let config = WatchConfig::new(
        vec![KubeName::label(NS).unwrap()],
        HelmFilter::new(&[]).unwrap(),
        JobMatcher::new(&[], None).unwrap(),
    );
    match config_server {
        Some(name) => config.with_config_server(DeploymentRef {
            namespace: KubeName::label(NS).unwrap(),
            name: KubeName::label(name).unwrap(),
        }),
        None => config,
    }
}

fn start(kube: &FakeKube, config: WatchConfig) -> (ClusterWatcher, Receiver<ClusterReport>) {
    ClusterWatcher::start(&kube.client(), config)
}

async fn next_report(rx: &mut Receiver<ClusterReport>) -> ClusterReport {
    timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("a report within 30 virtual seconds")
        .expect("the report channel is open")
}

fn cluster() -> FakeKube {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, SERVER).build());
    kube.apply(deployment(NS, "web").build());
    kube.apply(pod(NS, "web-1", "web", "2026-10-10T09:00:00Z"));
    kube
}

#[tokio::test(start_paused = true)]
async fn cluster_report_has_config_server_start_time() {
    let kube = cluster();
    kube.apply(pod(NS, "server-a", SERVER, "2026-10-10T11:00:00Z"));
    let (watcher, _reports) = start(&kube, config(Some(SERVER)));
    let report = watcher.full_report().await.unwrap();
    assert_eq!(
        report.config_server_started_at,
        Some(Timestamp::from_unix_millis(T11))
    );
    assert!(report.full);
    assert_eq!(
        report.deployments.len(),
        2,
        "the config-server is reported like any Deployment"
    );
}

#[tokio::test(start_paused = true)]
async fn with_replicas_the_oldest_pod_counts() {
    let kube = cluster();
    kube.apply(pod(NS, "server-new", SERVER, "2026-10-10T12:30:00Z"));
    kube.apply(pod(NS, "server-old", SERVER, "2026-10-10T10:00:00Z"));
    kube.apply(pod(NS, "server-mid", SERVER, "2026-10-10T11:00:00Z"));
    let (watcher, _reports) = start(&kube, config(Some(SERVER)));
    let report = watcher.full_report().await.unwrap();
    assert_eq!(
        report.config_server_started_at,
        Some(Timestamp::from_unix_millis(T10))
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_of_the_config_server_is_reported_as_it_happens() {
    let kube = cluster();
    kube.apply(pod(NS, "server-a", SERVER, "2026-10-10T10:00:00Z"));
    let (watcher, mut reports) = start(&kube, config(Some(SERVER)));
    let first = watcher.full_report().await.unwrap();
    assert_eq!(
        first.config_server_started_at,
        Some(Timestamp::from_unix_millis(T10))
    );

    // A rolling restart: the new pod comes up, then the old one goes.
    kube.apply(pod(NS, "server-b", SERVER, "2026-10-10T12:30:00Z"));
    kube.delete(Kind::Pod, NS, "server-a");
    // The report may come in two steps (the new pod, then the old one gone); the last one is the new start time.
    loop {
        let report = next_report(&mut reports).await;
        assert!(!report.full);
        if report.config_server_started_at == Some(Timestamp::from_unix_millis(T1230)) {
            break;
        }
        assert!(
            report.config_server_started_at == Some(Timestamp::from_unix_millis(T10)),
            "only the old or the new start time: {report:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_delta_about_another_deployment_carries_no_start_time() {
    let kube = cluster();
    kube.apply(pod(NS, "server-a", SERVER, "2026-10-10T10:00:00Z"));
    let (watcher, mut reports) = start(&kube, config(Some(SERVER)));
    watcher.full_report().await.unwrap();
    kube.apply(pod(NS, "web-2", "web", "2026-10-10T12:30:00Z"));
    let report = next_report(&mut reports).await;
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(report.deployments[0].service.name(), "web");
    assert_eq!(
        report.config_server_started_at, None,
        "the hub keeps what it has until the config-server itself changes"
    );
}

#[tokio::test(start_paused = true)]
async fn no_start_time_without_a_configured_deployment() {
    let kube = cluster();
    kube.apply(pod(NS, "server-a", SERVER, "2026-10-10T10:00:00Z"));
    let (watcher, _reports) = start(&kube, config(None));
    assert_eq!(
        watcher.full_report().await.unwrap().config_server_started_at,
        None
    );
}

#[tokio::test(start_paused = true)]
async fn no_start_time_when_the_deployment_has_no_pod_or_does_not_exist() {
    // No pods.
    let kube = cluster();
    let (watcher, _reports) = start(&kube, config(Some(SERVER)));
    assert_eq!(
        watcher.full_report().await.unwrap().config_server_started_at,
        None
    );

    // A configured name that is not a Deployment of the cluster.
    let kube = cluster();
    kube.apply(pod(NS, "server-a", SERVER, "2026-10-10T10:00:00Z"));
    let (watcher, _reports) = start(&kube, config(Some("not-there")));
    assert_eq!(
        watcher.full_report().await.unwrap().config_server_started_at,
        None
    );
}

#[tokio::test(start_paused = true)]
async fn a_pod_that_is_going_away_or_has_not_started_does_not_set_the_time() {
    let kube = cluster();
    kube.apply(terminating_pod(NS, "server-old", SERVER, "2026-10-10T10:00:00Z"));
    kube.apply(pod(NS, "server-new", SERVER, "2026-10-10T12:30:00Z"));
    // A pod the scheduler has not started yet: no start time and no creation time, so the agent has nothing to report.
    let mut pending = pod(NS, "server-pending", SERVER, "2026-10-10T09:00:00Z");
    pending["status"] = serde_json::json!({ "phase": "Pending" });
    kube.apply(pending);
    let (watcher, _reports) = start(&kube, config(Some(SERVER)));
    assert_eq!(
        watcher.full_report().await.unwrap().config_server_started_at,
        Some(Timestamp::from_unix_millis(T1230)),
        "a terminating pod and a pending one are not the pod that serves"
    );
}
