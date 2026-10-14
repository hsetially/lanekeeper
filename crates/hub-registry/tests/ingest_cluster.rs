//! `cluster` (T1, D75, D85, D88, S10): Deployments, pods, environment names, release hints, the config-server's start and sync
//! windows.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{
    ClusterReport, DeploymentInfo, DomainEvent, EnvValue, JobRef, PodInfo, ReleaseHint, ServiceRef,
    SyncWindowEvent, SyncWindowKind, Timestamp,
};
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::{MessageKind, Outcome};
use ports::SinkError;
use support::rig::{Rig, text};

const T0: i64 = hub_registry::ManualClock::T0_MS;

fn svc(ns: &str, name: &str) -> ServiceRef {
    ServiceRef::new(ns, name).unwrap()
}

fn pod(name: &str, started: i64) -> PodInfo {
    PodInfo {
        name: text(name),
        started_at: Timestamp::from_unix_millis(started),
    }
}

fn dep(ns: &str, name: &str, pods: Vec<PodInfo>) -> DeploymentInfo {
    DeploymentInfo {
        service: svc(ns, name),
        pods,
        env_values: Vec::new(),
        env_names: Vec::new(),
        removed: false,
    }
}

fn env(name: &str, value: &str) -> EnvValue {
    EnvValue {
        name: text(name),
        value: text(value),
    }
}

fn report(full: bool, deployments: Vec<DeploymentInfo>) -> ClusterReport {
    ClusterReport {
        full,
        deployments,
        release_hints: Vec::new(),
        config_server_started_at: None,
        sync_windows: Vec::new(),
    }
}

async fn restart_events(rig: &Rig) -> Vec<Vec<String>> {
    rig.bus
        .published()
        .await
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::PendingRestartChanged { services, .. } => Some(
                services
                    .iter()
                    .map(|s| format!("{}/{}", s.namespace(), s.name()))
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn deployments_pods_and_environment_are_stored() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let mut api = dep(
        "csp",
        "tx-infinity-api",
        vec![pod("api-2", T0 - 1_000), pod("api-1", T0 - 9_000)],
    );
    api.env_values = vec![
        env("SPRING_APPLICATION_NAME", "tx-infinity-api"),
        env("CONFIG_CLIENT_CACHE_TTL", "1200"),
        env("CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED", "true"),
    ];
    api.env_names = vec![
        text("JAVA_OPTS"),
        text("TXSECURITY_KEYSTORE_PASSWORD_FILE"),
        text("JAVA_OPTS"),
    ];
    rig.cluster(report(true, vec![api])).await.unwrap();

    let row: (String, String, serde_json::Value, Vec<String>, bool) = sqlx::query_as(
        "SELECT namespace, name, env_values, env_names, removed_at IS NULL FROM services WHERE swimlane_id = 'sit1'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!((row.0.as_str(), row.1.as_str()), ("csp", "tx-infinity-api"));
    assert_eq!(row.2["CONFIG_CLIENT_CACHE_TTL"], "1200");
    assert_eq!(row.2["SPRING_APPLICATION_NAME"], "tx-infinity-api");
    assert_eq!(
        row.3,
        ["JAVA_OPTS", "TXSECURITY_KEYSTORE_PASSWORD_FILE"],
        "names only, sorted and unique"
    );
    assert!(row.4);
    let state: (serde_json::Value, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT pods, (EXTRACT(EPOCH FROM oldest_pod_started_at) * 1000)::bigint, \
                (EXTRACT(EPOCH FROM newest_pod_started_at) * 1000)::bigint FROM service_state",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(state.0.as_array().unwrap().len(), 2);
    assert_eq!((state.1, state.2), (Some(T0 - 9_000), Some(T0 - 1_000)));
}

#[tokio::test]
async fn only_the_allowlisted_environment_values_are_kept() {
    // D88, S10: whatever the agent sends, a value of any other variable is dropped, and a "name" that is not a name too.
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let mut d = dep("csp", "svc", vec![pod("p", T0)]);
    d.env_values = vec![
        env("CONFIG_CLIENT_CACHE_TTL", "600"),
        env("DB_PASSWORD", "hunter2-planted-secret"),
        env("TOKEN", "planted-token-value"),
    ];
    d.env_names = vec![
        text("OK_NAME"),
        text("password=hunter2"),
        text("A B"),
        text("1START"),
    ];
    rig.cluster(report(true, vec![d])).await.unwrap();
    let (values, names): (serde_json::Value, Vec<String>) =
        sqlx::query_as("SELECT env_values, env_names FROM services")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(values, serde_json::json!({"CONFIG_CLIENT_CACHE_TTL": "600"}));
    assert_eq!(names, ["OK_NAME"]);
    let everything: String = sqlx::query_scalar(
        "SELECT (SELECT string_agg(env_values::text || env_names::text, '') FROM services)",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert!(!everything.contains("hunter2") && !everything.contains("planted"));
}

#[tokio::test]
async fn a_change_in_the_pods_publishes_a_pending_restart_hint_and_a_repeat_does_not() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let report_of = |pods: Vec<PodInfo>| report(false, vec![dep("csp", "a", pods)]);
    rig.cluster(report_of(vec![pod("a-1", T0 - 5_000)]))
        .await
        .unwrap();
    assert_eq!(
        restart_events(&rig).await,
        vec![vec!["csp/a".to_owned()]],
        "a Deployment appeared"
    );
    rig.cluster(report_of(vec![pod("a-1", T0 - 5_000)]))
        .await
        .unwrap();
    assert_eq!(
        restart_events(&rig).await.len(),
        1,
        "the same report changes nothing"
    );
    rig.cluster(report_of(vec![pod("a-2", T0)])).await.unwrap();
    assert_eq!(restart_events(&rig).await.len(), 2, "a restart");
    // A report that is not full does not mention the other Deployments, which are not touched.
    rig.cluster(report(false, vec![dep("csp", "b", vec![pod("b-1", T0)])]))
        .await
        .unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM services WHERE removed_at IS NULL")
            .await,
        2
    );
}

#[tokio::test]
async fn a_full_report_is_the_whole_list_and_a_removed_flag_removes() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.cluster(report(
        true,
        vec![
            dep("csp", "a", vec![pod("a-1", T0)]),
            dep("csp", "b", vec![pod("b-1", T0)]),
        ],
    ))
    .await
    .unwrap();
    rig.cluster(report(true, vec![dep("csp", "a", vec![pod("a-1", T0)])]))
        .await
        .unwrap();
    let gone: Vec<String> = sqlx::query_scalar("SELECT name FROM services WHERE removed_at IS NOT NULL")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(gone, ["b"]);
    assert!(
        restart_events(&rig)
            .await
            .last()
            .unwrap()
            .contains(&"csp/b".to_owned())
    );
    // In a delta report the flag says the Deployment no longer exists.
    let mut a = dep("csp", "a", vec![]);
    a.removed = true;
    rig.cluster(report(false, vec![a])).await.unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM services WHERE removed_at IS NOT NULL")
            .await,
        2
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM service_state WHERE pods <> '[]'::jsonb")
            .await,
        0
    );
    // It comes back.
    rig.cluster(report(false, vec![dep("csp", "a", vec![pod("a-9", T0 + 1_000)])]))
        .await
        .unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM services WHERE removed_at IS NULL")
            .await,
        1
    );
}

#[tokio::test]
async fn release_hints_and_the_config_servers_start_are_stored() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let hint = |service: &str, key: &str, value: &str| ReleaseHint {
        service: svc("csp", service),
        key: text(key),
        value: text(value),
    };
    let mut r = report(true, vec![]);
    r.release_hints = vec![
        hint("a", "release", "csp-tenant-data-sit1"),
        hint("a", "chart", "csp-tenant-data-1.2"),
    ];
    r.config_server_started_at = Some(Timestamp::from_unix_millis(T0 - 7_200_000));
    rig.cluster(r).await.unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM release_hints").await, 2);
    let started: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM config_server_started_at) * 1000)::bigint FROM swimlanes WHERE id = 'sit1'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(started, Some(T0 - 7_200_000));

    // The latest full report replaces the earlier one; a delta report adds to it.
    rig.clock.advance_secs(60);
    let mut full = report(true, vec![]);
    full.release_hints = vec![hint("a", "release", "csp-tenant-data-sit5")];
    rig.cluster(full).await.unwrap();
    let values: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM release_hints ORDER BY key")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        values,
        [("release".to_owned(), "csp-tenant-data-sit5".to_owned())]
    );
    rig.clock.advance_secs(60);
    let mut delta_report = report(false, vec![]);
    delta_report.release_hints = vec![hint("b", "release", "csp-tenant-data-sit5")];
    rig.cluster(delta_report).await.unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM release_hints").await, 2);
}

#[tokio::test]
async fn sync_windows_open_and_close_once_each_with_their_events() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let job = JobRef::new("sync-sit1", "uid-1").unwrap();
    let window = |kind, at| SyncWindowEvent {
        kind,
        job: job.clone(),
        at: Timestamp::from_unix_millis(at),
    };
    let mut r = report(false, vec![]);
    r.sync_windows = vec![window(SyncWindowKind::Opened, T0 - 60_000)];
    rig.cluster(r.clone()).await.unwrap();
    rig.cluster(r).await.unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM sync_windows WHERE closed_at IS NULL")
            .await,
        1
    );
    let events = rig.bus.published().await;
    let opened = events
        .iter()
        .filter(|e| matches!(e, DomainEvent::SyncWindowOpened { .. }))
        .count();
    assert_eq!(opened, 1, "D75: the same window reported twice is one event");

    let mut close = report(false, vec![]);
    close.sync_windows = vec![window(SyncWindowKind::Closed, T0 - 30_000)];
    rig.cluster(close.clone()).await.unwrap();
    rig.cluster(close).await.unwrap();
    let closed_at: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM closed_at) * 1000)::bigint FROM sync_windows WHERE job_uid = 'uid-1'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(closed_at, Some(T0 - 30_000));
    let events = rig.bus.published().await;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, DomainEvent::SyncWindowClosed { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_window_reported_closed_that_the_hub_never_saw_open_is_recorded() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    let job = JobRef::new("sync-sit1", "uid-2").unwrap();
    let mut r = report(false, vec![]);
    r.sync_windows = vec![SyncWindowEvent {
        kind: SyncWindowKind::Closed,
        job,
        at: Timestamp::from_unix_millis(T0),
    }];
    rig.cluster(r).await.unwrap();
    assert_eq!(
        rig.count("SELECT count(*) FROM sync_windows WHERE closed_at IS NOT NULL")
            .await,
        1
    );
}

#[tokio::test]
async fn reports_over_the_bounds_are_refused_and_change_nothing() {
    let config = RegistryConfig {
        max_services_per_report: 2,
        max_pods_per_service: 1,
        max_release_hints_per_report: 1,
        max_sync_windows_per_report: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.hello().await;
    let three = report(
        true,
        vec![
            dep("c", "a", vec![]),
            dep("c", "b", vec![]),
            dep("c", "c", vec![]),
        ],
    );
    assert_eq!(rig.cluster(three).await, Err(SinkError::TooLarge));
    let many_pods = report(true, vec![dep("c", "a", vec![pod("1", T0), pod("2", T0)])]);
    assert_eq!(rig.cluster(many_pods).await, Err(SinkError::TooLarge));
    let hint = |k: &str| ReleaseHint {
        service: svc("c", "a"),
        key: text(k),
        value: text("v"),
    };
    let mut hints = report(true, vec![]);
    hints.release_hints = vec![hint("a"), hint("b")];
    assert_eq!(rig.cluster(hints).await, Err(SinkError::TooLarge));
    assert_eq!(rig.count("SELECT count(*) FROM services").await, 0);
    assert_eq!(
        rig.metrics.message_count(MessageKind::Cluster, Outcome::TooLarge),
        3
    );
}

#[tokio::test]
async fn a_failing_bus_rolls_the_whole_report_back() {
    let Some(rig) = Rig::new().await else { return };
    rig.hello().await;
    rig.bus.set_failing(true);
    assert_eq!(
        rig.cluster(report(true, vec![dep("c", "a", vec![pod("p", T0)])]))
            .await,
        Err(SinkError::Unavailable)
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM services").await,
        0,
        "D80: the change and its event commit together"
    );
}
