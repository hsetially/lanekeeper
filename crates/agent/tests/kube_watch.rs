//! The Kubernetes watchers against a fake API server (T6, S17, P4).
//!
//! The fake speaks list and watch like the real one; the tests change the cluster and read the reports the agent makes.
//! Time is virtual (`start_paused`): the one-second debounce and the retry delays cost nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::Duration;

use agent::config::{KubeName, Settings};
use agent::kube::helm::HelmFilter;
use agent::kube::jobs::JobMatcher;
use agent::kube::{ClusterWatcher, ReportError, WatchConfig};
use agent::windows::WindowLedger;
use domain::{ClusterReport, DeploymentInfo, ShortText};
use std::sync::Arc;
use support::fake_kube::{Call, FakeKube, Kind};
use support::k8s_objects::{deployment, finish_job, job, pod, terminating_pod};
use support::log_capture::LogCapture;
use tokio::sync::mpsc::Receiver;
use tokio::time::{Instant, sleep, timeout};

const NS: &str = "sit1";
const TTL: &str = "CONFIG_CLIENT_CACHE_TTL";

fn config(namespaces: &[&str]) -> WatchConfig {
    WatchConfig::new(
        namespaces.iter().map(|n| KubeName::label(n).unwrap()).collect(),
        HelmFilter::new(&texts(&["csp-tenant-data-*"])).unwrap(),
        JobMatcher::new(&texts(&["*dataload*"]), None).unwrap(),
    )
}

fn texts(items: &[&str]) -> Vec<ShortText> {
    items.iter().map(|s| ShortText::parse(s).unwrap()).collect()
}

/// Start watching `namespaces`. The allowlist is set before any task runs, so the first list already follows it.
fn start(
    kube: &FakeKube,
    namespaces: &[&str],
    allowlist: &[&str],
) -> (ClusterWatcher, Receiver<ClusterReport>) {
    start_with(kube, config(namespaces), allowlist)
}

fn start_with(
    kube: &FakeKube,
    config: WatchConfig,
    allowlist: &[&str],
) -> (ClusterWatcher, Receiver<ClusterReport>) {
    let (watcher, reports) = ClusterWatcher::start(&kube.client(), config);
    watcher.set_env_allowlist(&texts(allowlist));
    (watcher, reports)
}

async fn next_report(rx: &mut Receiver<ClusterReport>) -> ClusterReport {
    timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("a report within 30 virtual seconds")
        .expect("the report channel is open")
}

/// Let `secs` of virtual time pass, then assert that no report was released.
async fn assert_quiet(rx: &mut Receiver<ClusterReport>, secs: u64) {
    sleep(Duration::from_secs(secs)).await;
    if let Ok(report) = rx.try_recv() {
        panic!("a report nobody asked for: {report:?}");
    }
}

fn deployment_of<'a>(report: &'a ClusterReport, name: &str) -> &'a DeploymentInfo {
    report
        .deployments
        .iter()
        .find(|d| d.service.name() == name)
        .unwrap_or_else(|| panic!("no {name} in {report:?}"))
}

fn pod_names(info: &DeploymentInfo) -> Vec<&str> {
    info.pods.iter().map(|p| p.name.as_str()).collect()
}

/// List calls (a `GET` of a collection that is not a watch) to a collection.
fn lists(kube: &FakeKube, kind: Kind, namespace: &str) -> Vec<Call> {
    let path = FakeKube::collection_path(kind, namespace);
    kube.calls()
        .into_iter()
        .filter(|c| c.method == "GET" && c.path == path && c.query_param("watch").as_deref() != Some("true"))
        .collect()
}

fn watches(kube: &FakeKube, kind: Kind, namespace: &str) -> Vec<Call> {
    let path = FakeKube::collection_path(kind, namespace);
    kube.calls()
        .into_iter()
        .filter(|c| c.method == "GET" && c.path == path && c.query_param("watch").as_deref() == Some("true"))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn watch_driven_cluster_report() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "svc-a")
            .env(TTL, "20m")
            .env("JAVA_OPTS", "-Xmx1g")
            .build(),
    );
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    let (watcher, mut rx) = start(&kube, &[NS], &[TTL]);

    let full = watcher.full_report().await.unwrap();
    assert!(full.full);
    let a = deployment_of(&full, "svc-a");
    assert_eq!((a.service.namespace(), a.removed), (NS, false));
    assert_eq!(pod_names(a), vec!["svc-a-1"]);
    assert_eq!(a.pods[0].started_at.unix_millis(), 1_791_630_000_000);
    assert_eq!(
        a.env_values
            .iter()
            .map(|v| (v.name.as_str(), v.value.as_str()))
            .collect::<Vec<_>>(),
        vec![(TTL, "20m")]
    );
    assert_eq!(
        a.env_names.iter().map(ShortText::as_str).collect::<Vec<_>>(),
        vec!["JAVA_OPTS"]
    );
    // What the full report covered is not sent again.
    assert_quiet(&mut rx, 5).await;

    // A pod appears: a delta with the Deployment it belongs to, and nothing else.
    kube.apply(pod(NS, "svc-a-2", "svc-a", "2026-10-10T12:00:00Z"));
    let delta = next_report(&mut rx).await;
    assert!(!delta.full);
    assert_eq!(delta.deployments.len(), 1);
    assert_eq!(pod_names(&delta.deployments[0]), vec!["svc-a-1", "svc-a-2"]);

    // The Deployment goes away.
    kube.delete(Kind::Deployment, NS, "svc-a");
    let gone = next_report(&mut rx).await;
    assert!(!gone.full);
    assert!(deployment_of(&gone, "svc-a").removed);
    assert_quiet(&mut rx, 5).await;

    // Watched, not polled: one list per kind, however many changes came.
    for kind in [Kind::Deployment, Kind::Pod, Kind::Job] {
        assert_eq!(lists(&kube, kind, NS).len(), 1, "{kind:?}");
        assert!(!watches(&kube, kind, NS).is_empty(), "{kind:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn report_debounced_to_one_second() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();

    // Five pods in 600 ms.
    let first_change = Instant::now();
    for i in 0..5 {
        kube.apply(pod(NS, &format!("svc-a-{i}"), "svc-a", "2026-10-10T12:00:00Z"));
        sleep(Duration::from_millis(150)).await;
    }
    sleep(Duration::from_millis(250)).await;
    assert!(
        rx.try_recv().is_err(),
        "nothing leaves before a second has passed since the first change"
    );
    let report = next_report(&mut rx).await;
    let waited = first_change.elapsed();
    assert!(
        waited >= Duration::from_secs(1) && waited < Duration::from_millis(1_050),
        "released after {waited:?}"
    );
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(
        report.deployments[0].pods.len(),
        5,
        "all five changes are in the one report"
    );
    assert_quiet(&mut rx, 10).await;

    // The next change opens the next window.
    let second_change = Instant::now();
    kube.apply(pod(NS, "svc-a-9", "svc-a", "2026-10-10T12:00:00Z"));
    let second = next_report(&mut rx).await;
    assert_eq!(second_change.elapsed(), Duration::from_secs(1));
    assert_eq!(second.deployments[0].pods.len(), 6);
}

#[tokio::test(start_paused = true)]
async fn full_report_on_request() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    kube.apply(deployment(NS, "svc-b").build());
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    let first = watcher.full_report().await.unwrap();
    assert_eq!(first.deployments.len(), 2);

    // A change that is still waiting in the debounce window is covered by the next full report, which drops it.
    kube.apply(pod(NS, "svc-b-1", "svc-b", "2026-10-10T12:00:00Z"));
    sleep(Duration::from_millis(100)).await;
    let second = watcher.full_report().await.unwrap();
    assert!(second.full);
    assert_eq!(pod_names(deployment_of(&second, "svc-b")), vec!["svc-b-1"]);
    assert_quiet(&mut rx, 5).await;

    // And it can be asked for again and again.
    assert_eq!(watcher.full_report().await.unwrap(), second);
}

#[tokio::test(start_paused = true)]
async fn a_pod_that_starts_terminating_leaves_the_report() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();

    kube.apply(terminating_pod(NS, "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    let delta = next_report(&mut rx).await;
    assert!(deployment_of(&delta, "svc-a").pods.is_empty());
    assert!(pod_names(deployment_of(&watcher.full_report().await.unwrap(), "svc-a")).is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_the_configured_namespaces_are_watched() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "in-one").build());
    kube.apply(deployment("sit2", "in-two").build());
    kube.apply(deployment("elsewhere", "not-ours").build());
    let (watcher, mut rx) = start(&kube, &[NS, "sit2"], &[]);

    let full = watcher.full_report().await.unwrap();
    let mut names: Vec<_> = full.deployments.iter().map(|d| d.service.name()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["in-one", "in-two"]);

    kube.apply(deployment("elsewhere", "also-not-ours").build());
    kube.apply(pod("elsewhere", "x-1", "not-ours", "2026-10-10T12:00:00Z"));
    assert_quiet(&mut rx, 5).await;
    assert!(
        kube.calls().iter().all(|c| !c.path.contains("/elsewhere/")),
        "the agent asked about a namespace it was not given"
    );
    assert!(
        kube.calls()
            .iter()
            .all(|c| c.query_param("labelSelector").is_none()),
        "no server-side selection is relied on"
    );
}

#[tokio::test(start_paused = true)]
async fn a_dropped_watch_connection_is_picked_up_again() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();

    kube.drop_watches();
    sleep(Duration::from_secs(5)).await;
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T12:00:00Z"));
    let delta = next_report(&mut rx).await;
    assert_eq!(pod_names(&delta.deployments[0]), vec!["svc-a-1"]);
    // The watch resumed from where it was; it did not list everything again.
    assert_eq!(lists(&kube, Kind::Deployment, NS).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failing_list_is_retried_and_the_report_follows() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    kube.fail_next_lists(&[500, 500, 503]);
    let (watcher, _rx) = start(&kube, &[NS], &[]);
    let full = watcher.full_report().await.unwrap();
    assert_eq!(full.deployments.len(), 1);
    assert!(
        lists(&kube, Kind::Deployment, NS).len()
            + lists(&kube, Kind::Pod, NS).len()
            + lists(&kube, Kind::Job, NS).len()
            > 3
    );
}

#[tokio::test(start_paused = true)]
async fn no_full_report_while_the_lists_are_refused_and_the_log_has_only_the_status() {
    let logs = LogCapture::new();
    let _guard = logs.install();
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    kube.deny_everything(403);
    let mut cfg = config(&[NS]);
    cfg.sync_timeout = Duration::from_secs(5);
    let (watcher, mut rx) = start_with(&kube, cfg, &[]);

    // An empty report would say that there are no Deployments; the agent says it does not know yet.
    assert_eq!(watcher.full_report().await, Err(ReportError::NotSynced));
    assert!(!watcher.is_ready());
    assert!(rx.try_recv().is_err());
    // The agent's own lines. (kube-rs logs the server's `Status` message at WARN and DEBUG; the log filter that T7 sets
    // up must keep `kube_client` and `kube_runtime` quieter than that, and T7 tests it.)
    let text = logs.text();
    let ours: Vec<&str> = text.lines().filter(|l| l.contains(" agent::")).collect();
    assert!(
        ours.iter().any(|l| l.contains("403")),
        "the status is logged: {ours:?}"
    );
    assert!(
        ours.iter().all(|l| !l.contains("denied by the fake RBAC")),
        "the server's message is not: {ours:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn release_hints_read_labels_only() {
    let kube = FakeKube::new(NS);
    kube.apply(
        deployment(NS, "tenant-config")
            .helm("csp-tenant-data-sit1-0.3.1", "tenant-sit1")
            .label("owner", "LABEL_NOT_A_HINT_MARKER")
            .annotation("meta.helm.sh/release-name", "ANNOTATION_HINT_MARKER")
            .build(),
    );
    kube.apply(
        deployment(NS, "ingress")
            .helm("ingress-nginx-4.0.0", "ingress")
            .build(),
    );
    kube.apply(deployment(NS, "plain").build());
    kube.apply(job(
        NS,
        "tenant-data-load",
        "job-uid-1",
        &[
            ("helm.sh/chart", "csp-tenant-data-sit2-0.3.1"),
            ("app.kubernetes.io/instance", "tenant-sit2"),
        ],
    ));
    let (watcher, _rx) = start(&kube, &[NS], &[]);

    let full = watcher.full_report().await.unwrap();
    let hints: Vec<(&str, &str, &str)> = full
        .release_hints
        .iter()
        .map(|h| (h.service.name(), h.key.as_str(), h.value.as_str()))
        .collect();
    assert_eq!(
        hints,
        vec![
            ("tenant-config", "helm.sh/chart", "csp-tenant-data-sit1-0.3.1"),
            ("tenant-config", "app.kubernetes.io/instance", "tenant-sit1"),
            ("tenant-data-load", "helm.sh/chart", "csp-tenant-data-sit2-0.3.1"),
            ("tenant-data-load", "app.kubernetes.io/instance", "tenant-sit2"),
        ]
    );
    let shown = format!("{full:?}");
    assert!(
        !shown.contains("HINT_MARKER") && !shown.contains("LABEL_NOT_A_HINT"),
        "{shown}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_hint_that_appears_later_is_sent_in_a_delta() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "tenant-config").build());
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    assert!(watcher.full_report().await.unwrap().release_hints.is_empty());

    kube.apply(
        deployment(NS, "tenant-config")
            .helm("csp-tenant-data-sit1-0.3.2", "tenant-sit1")
            .build(),
    );
    let delta = next_report(&mut rx).await;
    assert_eq!(delta.release_hints.len(), 2);
    assert_eq!(
        delta.release_hints[0].value.as_str(),
        "csp-tenant-data-sit1-0.3.2"
    );
}

#[tokio::test(start_paused = true)]
async fn sync_jobs_are_tracked_while_they_run() {
    let kube = FakeKube::new(NS);
    kube.apply(job(NS, "csp-dataload-1", "uid-1", &[]));
    kube.apply(job(NS, "nightly-report", "uid-2", &[]));
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();
    let running = watcher.active_sync_jobs();
    assert_eq!(running.len(), 1);
    assert_eq!((running[0].name(), running[0].uid()), ("csp-dataload-1", "uid-1"));

    kube.apply(finish_job(job(NS, "csp-dataload-1", "uid-1", &[])));
    sleep(Duration::from_millis(100)).await;
    assert!(watcher.active_sync_jobs().is_empty());
    // A Job without a hint is not a report: nothing is sent for it.
    assert_quiet(&mut rx, 5).await;
}

#[tokio::test(start_paused = true)]
async fn reflector_objects_are_trimmed() {
    let kube = FakeKube::new(NS);
    let mut manifest_bytes = 0;
    for d in 0..5 {
        let manifest = deployment(NS, &format!("svc-{d}"))
            .annotation(
                "kubectl.kubernetes.io/last-applied-configuration",
                &"ANNOTATION_MARKER ".repeat(200),
            )
            .env("JAVA_OPTS", "-Dpassword=ENV_VALUE_MARKER")
            .env(TTL, "20m")
            .env_from_secret("DB_URL", "SECRET_REF_MARKER")
            .build();
        manifest_bytes += manifest.to_string().len();
        kube.apply(manifest);
        for p in 0..20 {
            let pod = pod(
                NS,
                &format!("svc-{d}-{p:02}"),
                &format!("svc-{d}"),
                "2026-10-10T11:00:00Z",
            );
            manifest_bytes += pod.to_string().len();
            kube.apply(pod);
        }
    }
    let (watcher, _rx) = start(&kube, &[NS], &[TTL]);
    watcher.full_report().await.unwrap();

    let kept = watcher.retained_state();
    for marker in [
        "ANNOTATION_MARKER",
        "MANAGED_FIELDS_MARKER",
        "IMAGE_MARKER",
        "ENV_VALUE_MARKER",
        "SECRET_REF_MARKER",
        "POD_ANNOTATION_MARKER",
    ] {
        assert!(!kept.contains(marker), "{marker} is retained: {kept}");
    }
    assert!(kept.contains("20m"), "the allowlisted value is kept");
    assert!(
        kept.len() * 8 < manifest_bytes,
        "{} bytes kept of {manifest_bytes} bytes of manifests",
        kept.len()
    );
}

#[tokio::test(start_paused = true)]
async fn a_hundred_pods_in_one_burst_are_one_report() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();
    for i in 0..100 {
        kube.apply(pod(NS, &format!("svc-a-{i:03}"), "svc-a", "2026-10-10T12:00:00Z"));
    }
    let report = next_report(&mut rx).await;
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(report.deployments[0].pods.len(), 100);
    assert_quiet(&mut rx, 5).await;
}

#[tokio::test(start_paused = true)]
async fn nothing_is_released_until_deployments_and_pods_are_both_listed() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    // The pod list is refused for a while. The Deployment list is not.
    kube.fail_next_lists_of(Kind::Pod, &[500, 500, 500]);
    let (watcher, mut rx) = start(&kube, &[NS], &[]);

    // The Deployment is known and has changed, but a report now would say that it has no pods.
    sleep(Duration::from_millis(1_300)).await;
    assert!(!watcher.is_ready());
    assert!(rx.try_recv().is_err(), "a delta left before the pods were listed");

    // Once the pods are in, the Deployment is reported with them.
    let delta = next_report(&mut rx).await;
    assert_eq!(pod_names(deployment_of(&delta, "svc-a")), vec!["svc-a-1"]);
    assert!(watcher.is_ready());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_watcher_stops_the_watches_and_closes_the_report_channel() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    let (watcher, mut rx) = start(&kube, &[NS], &[]);
    watcher.full_report().await.unwrap();
    sleep(Duration::from_secs(2)).await;
    let calls = kube.calls().len();

    drop(watcher);
    kube.apply(pod(NS, "svc-a-1", "svc-a", "2026-10-10T12:00:00Z"));
    sleep(Duration::from_secs(30)).await;
    assert!(
        rx.recv().await.is_none(),
        "no report after the watcher is gone, and the channel is closed"
    );
    assert_eq!(
        kube.calls().len(),
        calls,
        "nothing asked of the API server after the drop"
    );
}

#[tokio::test(start_paused = true)]
async fn the_watch_configuration_follows_the_settings() {
    let mut env = support::valid_env(std::path::Path::new("/mnt/csp"));
    for (k, v) in [
        ("LK_NAMESPACES", "sit1,sit2"),
        ("LK_HELM_HINT_CHART_GLOBS", "tenant-*"),
        ("LK_SYNC_JOB_NAME_GLOBS", "*sync*"),
        ("LK_SYNC_JOB_LABEL", "lanekeeper.io/sync=true"),
    ] {
        env.insert(k.to_owned(), v.to_owned());
    }
    let settings = Settings::from_env(&env).unwrap();
    let config = WatchConfig::from_settings(&settings).unwrap();
    assert_eq!(
        config.namespaces.iter().map(KubeName::as_str).collect::<Vec<_>>(),
        vec!["sit1", "sit2"]
    );
    assert_eq!((config.debounce, config.page_size), (Duration::from_secs(1), 200));

    let chart = |v: &str| std::collections::BTreeMap::from([("helm.sh/chart".to_owned(), v.to_owned())]);
    assert!(config.helm.hints(Some(&chart("tenant-sit1-1.0.0"))).is_some());
    assert!(
        config
            .helm
            .hints(Some(&chart("csp-tenant-data-sit1-1.0.0")))
            .is_none(),
        "the default no longer applies"
    );
    let labelled = std::collections::BTreeMap::from([("lanekeeper.io/sync".to_owned(), "true".to_owned())]);
    assert!(config.jobs.matches("nightly-sync", None));
    assert!(config.jobs.matches("copy", Some(&labelled)));
    assert!(
        !config.jobs.matches("csp-dataload-1", None),
        "the default glob no longer applies"
    );
}

#[tokio::test(start_paused = true)]
async fn the_first_list_is_paged() {
    let kube = FakeKube::new(NS);
    kube.apply(deployment(NS, "svc-a").build());
    for i in 0..450 {
        kube.apply(pod(NS, &format!("svc-a-{i:03}"), "svc-a", "2026-10-10T11:00:00Z"));
    }
    let (watcher, _rx) = start(&kube, &[NS], &[]);
    let full = watcher.full_report().await.unwrap();
    assert_eq!(deployment_of(&full, "svc-a").pods.len(), 450);

    // 200 at a time, so a list never holds more than a page of full objects.
    let pod_lists = lists(&kube, Kind::Pod, NS);
    assert_eq!(pod_lists.len(), 3, "{pod_lists:?}");
    assert!(
        pod_lists
            .iter()
            .all(|c| c.query_param("limit").as_deref() == Some("200"))
    );
    assert_eq!(pod_lists[0].query_param("continue"), None);
    assert_eq!(pod_lists[1].query_param("continue").as_deref(), Some("200"));
    assert_eq!(pod_lists[2].query_param("continue").as_deref(), Some("400"));
}

// ------------------------------------------------------------------------------------------------ sync windows (T10)

fn with_ledger() -> (Arc<WindowLedger>, WatchConfig) {
    let clock: Arc<dyn agent::clock::Clock> = Arc::new(agent::clock::SystemClock);
    let ledger = Arc::new(WindowLedger::new(clock));
    (ledger.clone(), config(&[NS]).with_windows(ledger))
}

#[tokio::test(start_paused = true)]
async fn a_sync_job_opens_a_window_when_it_runs_and_closes_it_when_it_completes() {
    let kube = FakeKube::new(NS);
    let (ledger, config) = with_ledger();
    let (watcher, _rx) = start_with(&kube, config, &[]);
    watcher.full_report().await.unwrap();
    assert!(ledger.views().is_empty());

    kube.apply(job(NS, "csp-dataload-1", "uid-1", &[]));
    sleep(Duration::from_millis(100)).await;
    let views = ledger.views();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].job.name(), "csp-dataload-1");
    assert!(views[0].closed_gen.is_none());
    // The start time is the API server's.
    assert_eq!(views[0].opened_at.unix_millis(), 1_791_633_545_000);

    kube.apply(finish_job(job(NS, "csp-dataload-1", "uid-1", &[])));
    sleep(Duration::from_millis(100)).await;
    let views = ledger.views();
    assert_eq!(views.len(), 1, "the same window, now closed");
    assert!(views[0].closed_gen.is_some());
    assert_eq!(views[0].closed_at.unwrap().unix_millis(), 1_791_633_780_000);
}

#[tokio::test(start_paused = true)]
async fn a_job_that_is_not_a_sync_job_opens_no_window() {
    let kube = FakeKube::new(NS);
    let (ledger, config) = with_ledger();
    let (watcher, _rx) = start_with(&kube, config, &[]);
    kube.apply(job(NS, "nightly-report", "uid-2", &[]));
    watcher.full_report().await.unwrap();
    sleep(Duration::from_millis(100)).await;
    assert!(ledger.views().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_sync_job_that_is_deleted_while_running_closes_its_window() {
    let kube = FakeKube::new(NS);
    let (ledger, config) = with_ledger();
    let (watcher, _rx) = start_with(&kube, config, &[]);
    kube.apply(job(NS, "csp-dataload-1", "uid-1", &[]));
    watcher.full_report().await.unwrap();
    sleep(Duration::from_millis(100)).await;
    assert!(ledger.views()[0].closed_gen.is_none());
    kube.delete(Kind::Job, NS, "csp-dataload-1");
    sleep(Duration::from_millis(100)).await;
    assert!(ledger.views()[0].closed_gen.is_some());
    assert!(ledger.active().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_job_found_running_by_the_first_list_has_a_window() {
    let kube = FakeKube::new(NS);
    kube.apply(job(NS, "csp-dataload-1", "uid-1", &[]));
    let (ledger, config) = with_ledger();
    let (watcher, _rx) = start_with(&kube, config, &[]);
    watcher.full_report().await.unwrap();
    assert_eq!(
        ledger.active().map(|j| j.name().to_owned()).as_deref(),
        Some("csp-dataload-1")
    );
}

#[tokio::test(start_paused = true)]
async fn a_job_already_finished_when_first_listed_has_no_window() {
    let kube = FakeKube::new(NS);
    kube.apply(finish_job(job(NS, "csp-dataload-1", "uid-1", &[])));
    let (ledger, config) = with_ledger();
    let (watcher, _rx) = start_with(&kube, config, &[]);
    watcher.full_report().await.unwrap();
    assert!(
        ledger.views().is_empty(),
        "nothing was tagged during a window nobody saw"
    );
}
