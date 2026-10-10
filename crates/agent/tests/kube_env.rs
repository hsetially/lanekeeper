//! Environment variables in cluster reports (T6, S17, D88, A19): values only for the hub's allowlist, names for the
//! rest, and nothing at all for what looks like a secret or what the hub would reject.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use agent::config::KubeName;
use agent::kube::helm::HelmFilter;
use agent::kube::jobs::JobMatcher;
use agent::kube::{ClusterWatcher, WatchConfig};
use domain::{ClusterReport, DeploymentInfo, ShortText};
use support::fake_kube::{FakeKube, Kind};
use support::k8s_objects::deployment;
use support::log_capture::LogCapture;
use tokio::sync::mpsc::Receiver;
use tokio::time::{sleep, timeout};

const NS: &str = "sit1";

fn texts(items: &[&str]) -> Vec<ShortText> {
    items.iter().map(|s| ShortText::parse(s).unwrap()).collect()
}

fn start(kube: &FakeKube, allowlist: &[&str]) -> (ClusterWatcher, Receiver<ClusterReport>) {
    let config = WatchConfig::new(
        vec![KubeName::label(NS).unwrap()],
        HelmFilter::new(&[]).unwrap(),
        JobMatcher::new(&[], None).unwrap(),
    );
    let (watcher, reports) = ClusterWatcher::start(&kube.client(), config);
    watcher.set_env_allowlist(&texts(allowlist));
    (watcher, reports)
}

fn values(info: &DeploymentInfo) -> Vec<(&str, &str)> {
    info.env_values
        .iter()
        .map(|v| (v.name.as_str(), v.value.as_str()))
        .collect()
}

fn names(info: &DeploymentInfo) -> Vec<&str> {
    info.env_names.iter().map(ShortText::as_str).collect()
}

fn dep_lists(kube: &FakeKube) -> usize {
    let path = FakeKube::collection_path(Kind::Deployment, NS);
    kube.calls()
        .iter()
        .filter(|c| c.method == "GET" && c.path == path && c.query_param("watch").is_none())
        .count()
}

#[tokio::test(start_paused = true)]
async fn env_values_only_for_allowlist() {
    let logs = LogCapture::new();
    let _guard = logs.install();
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .env("CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED", "true")
            .env("JAVA_OPTS", "-Dnot.on.the.list=VALUE_MARKER_1")
            .env("DB_URL", "jdbc:postgresql://host/db?VALUE_MARKER_2")
            .env_from_secret("FROM_A_SECRET", "SECRET_REF_MARKER")
            .build(),
    );
    let (watcher, _rx) = start(
        &kube,
        &[
            "CONFIG_CLIENT_CACHE_TTL",
            "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED",
            "FROM_A_SECRET",
        ],
    );
    let report = watcher.full_report().await.unwrap();
    let a = &report.deployments[0];

    assert_eq!(
        values(a),
        vec![
            ("CONFIG_CLIENT_CACHE_TTL", "20m"),
            ("CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED", "true"),
        ]
    );
    // Every other variable is there by name, an allowlisted one that has no literal value included.
    assert_eq!(names(a), vec!["DB_URL", "FROM_A_SECRET", "JAVA_OPTS"]);
    // And no value outside the list is anywhere: not in the report, not in what the agent keeps, not in its log.
    let everything = format!("{report:?} {} {}", watcher.retained_state(), logs.text());
    for marker in ["VALUE_MARKER_1", "VALUE_MARKER_2", "SECRET_REF_MARKER"] {
        assert!(!everything.contains(marker), "{marker} leaked");
    }
}

#[tokio::test(start_paused = true)]
async fn secret_named_env_never_reported() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("DB_PASSWORD", "PASSWORD_MARKER")
            .env("api_key", "KEY_MARKER")
            .env("OAUTH_TOKEN", "TOKEN_MARKER")
            .env("CLIENT_SECRET", "SECRET_MARKER")
            .env("SvcCredentials", "CREDENTIAL_MARKER")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .build(),
    );
    // The hub allowlists every one of them, by mistake or by an attacker. The agent still refuses.
    let (watcher, _rx) = start(
        &kube,
        &[
            "DB_PASSWORD",
            "api_key",
            "OAUTH_TOKEN",
            "CLIENT_SECRET",
            "SvcCredentials",
            "CONFIG_CLIENT_CACHE_TTL",
        ],
    );
    let report = watcher.full_report().await.unwrap();
    let a = &report.deployments[0];
    assert_eq!(values(a), vec![("CONFIG_CLIENT_CACHE_TTL", "20m")]);
    assert_eq!(
        names(a),
        vec![
            "CLIENT_SECRET",
            "DB_PASSWORD",
            "OAUTH_TOKEN",
            "SvcCredentials",
            "api_key"
        ]
    );
    let everything = format!("{report:?} {}", watcher.retained_state());
    for marker in [
        "PASSWORD_MARKER",
        "KEY_MARKER",
        "TOKEN_MARKER",
        "SECRET_MARKER",
        "CREDENTIAL_MARKER",
    ] {
        assert!(!everything.contains(marker), "{marker} leaked");
    }
}

#[tokio::test(start_paused = true)]
async fn env_value_over_256_bytes_or_control_chars_dropped() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("FITS", &"x".repeat(256))
            .env("TOO_LONG", &"x".repeat(257))
            .env("MULTILINE", "line one\nline two")
            .env("ESCAPE", "\u{1b}[31mred")
            .env("EMPTY", "")
            .build(),
    );
    let (watcher, _rx) = start(&kube, &["FITS", "TOO_LONG", "MULTILINE", "ESCAPE", "EMPTY"]);
    let report = watcher.full_report().await.unwrap();
    let a = &report.deployments[0];
    // One value the hub rejects would make it reject the whole report; those are reported as names.
    assert_eq!(values(a), vec![("EMPTY", ""), ("FITS", &"x".repeat(256))]);
    assert_eq!(names(a), vec!["ESCAPE", "MULTILINE", "TOO_LONG"]);
    // The report converts to the wire form the hub validates.
    let wire = proto::convert::FromAgent::Cluster(report.clone());
    assert!(proto::convert::FromAgent::from_proto(wire.into_proto()).is_ok());
}

#[tokio::test(start_paused = true)]
async fn nothing_is_reported_until_the_hub_sends_an_allowlist() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .build(),
    );
    let (watcher, _rx) = start(&kube, &[]);
    let a = watcher.full_report().await.unwrap().deployments.remove(0);
    assert!(a.env_values.is_empty());
    assert_eq!(names(&a), vec!["CONFIG_CLIENT_CACHE_TTL"]);
}

#[tokio::test(start_paused = true)]
async fn a_full_report_waits_for_the_new_allowlist_to_take_effect() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .env("JAVA_OPTS", "-Xmx1g")
            .build(),
    );
    let (watcher, _rx) = start(&kube, &[]);
    watcher.full_report().await.unwrap();
    assert_eq!(dep_lists(&kube), 1);

    // The hub sends the list. The very next full report already follows it: no sleeping, no second ask.
    let asked = tokio::time::Instant::now();
    watcher.set_env_allowlist(&texts(&["CONFIG_CLIENT_CACHE_TTL"]));
    let widened = watcher.full_report().await.unwrap();
    assert_eq!(
        values(&widened.deployments[0]),
        vec![("CONFIG_CLIENT_CACHE_TTL", "20m")]
    );
    assert_eq!(dep_lists(&kube), 2, "the Deployments were listed again, once");
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "listing again because of a new allowlist is not a retry and must not wait like one: {:?}",
        asked.elapsed()
    );

    // The same list again changes nothing and costs nothing.
    watcher.set_env_allowlist(&texts(&["CONFIG_CLIENT_CACHE_TTL"]));
    watcher.full_report().await.unwrap();
    assert_eq!(dep_lists(&kube), 2);

    // A narrower list takes the value back out, and the agent no longer holds it.
    watcher.set_env_allowlist(&[]);
    let narrowed = watcher.full_report().await.unwrap();
    assert!(narrowed.deployments[0].env_values.is_empty());
    assert!(!watcher.retained_state().contains("20m"));
    assert_eq!(dep_lists(&kube), 3);
}

#[tokio::test(start_paused = true)]
async fn an_allowlist_change_that_nobody_asked_a_report_about_is_sent_as_a_delta() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .build(),
    );
    kube.apply(deployment(NS, "svc-b").env("OTHER", "x").build());
    let (watcher, mut rx) = start(&kube, &[]);
    watcher.full_report().await.unwrap();

    watcher.set_env_allowlist(&texts(&["CONFIG_CLIENT_CACHE_TTL"]));
    let delta = timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(!delta.full);
    // Only the Deployment whose report changed is in it.
    assert_eq!(delta.deployments.len(), 1);
    assert_eq!(
        values(&delta.deployments[0]),
        vec![("CONFIG_CLIENT_CACHE_TTL", "20m")]
    );
    sleep(Duration::from_secs(5)).await;
    assert!(rx.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn a_changed_value_of_an_allowlisted_variable_is_sent() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "20m")
            .build(),
    );
    let (watcher, mut rx) = start(&kube, &["CONFIG_CLIENT_CACHE_TTL"]);
    watcher.full_report().await.unwrap();

    kube.apply(
        deployment(NS, "svc-a")
            .env("CONFIG_CLIENT_CACHE_TTL", "5m")
            .build(),
    );
    let delta = timeout(Duration::from_secs(30), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        values(&delta.deployments[0]),
        vec![("CONFIG_CLIENT_CACHE_TTL", "5m")]
    );
}
