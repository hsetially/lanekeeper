//! The hub's cluster commands, answered by the dispatcher (T6): `RequestClusterReport` and `RestartDeployment`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::NfsRoot;
use agent::clock::Clock;
use agent::config::KubeName;
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::FileOps;
use agent::kube::helm::HelmFilter;
use agent::kube::jobs::JobMatcher;
use agent::kube::{Cluster, ClusterOps, WatchConfig};
use domain::{AgentReply, HubCommand, OpError, OpResult, RequestId, ServiceRef};
use proto::convert::FromAgent;
use support::clock::TestClock;
use support::fake_kube::{FakeKube, Kind};
use support::k8s_objects::{deployment, pod};
use tempfile::TempDir;

const LIMITS: OpLimits = OpLimits {
    max_file_bytes: 2 * 1024 * 1024,
};

fn rid(text: &str) -> RequestId {
    RequestId::parse(text).unwrap()
}

fn svc(namespace: &str, name: &str) -> ServiceRef {
    ServiceRef::new(namespace, name).unwrap()
}

struct Rig {
    kube: FakeKube,
    dispatcher: Dispatcher,
    _dir: TempDir,
    _cluster: Arc<Cluster>,
}

fn rig_with(kube: FakeKube, sync_timeout: Duration) -> Rig {
    let dir = TempDir::new().unwrap();
    let mut config = WatchConfig::new(
        vec![KubeName::label("sit1").unwrap()],
        HelmFilter::new(&[]).unwrap(),
        JobMatcher::new(&[], None).unwrap(),
    );
    config.sync_timeout = sync_timeout;
    let clock: Arc<dyn Clock> = Arc::new(TestClock::starting_at(1_791_633_600_000));
    let (cluster, reports) = Cluster::start_with(&kube.client(), config, clock);
    // The delta channel is the app's to drain; here nothing reads it.
    std::mem::forget(reports);
    let cluster = Arc::new(cluster);
    let ops = FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    );
    let dispatcher = Dispatcher::new(ops).with_cluster(Arc::clone(&cluster) as Arc<dyn ClusterOps>);
    Rig {
        kube,
        dispatcher,
        _dir: dir,
        _cluster: cluster,
    }
}

fn rig() -> Rig {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("sit1", "svc-a").build());
    kube.apply(pod("sit1", "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    rig_with(kube, Duration::from_secs(20))
}

fn op(reply: Option<FromAgent>) -> OpResult {
    match reply {
        Some(FromAgent::Reply(AgentReply::Op(result))) => result,
        other => panic!("an OpResult, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn request_cluster_report_is_answered_with_the_full_report() {
    let rig = rig();
    let reply = rig
        .dispatcher
        .handle(
            HubCommand::RequestClusterReport {
                request_id: rid("c1"),
            },
            LIMITS,
        )
        .await;
    let Some(FromAgent::Reply(AgentReply::Cluster { request_id, report })) = reply else {
        panic!("a cluster reply, got {reply:?}");
    };
    assert_eq!(request_id.as_str(), "c1");
    assert!(report.full);
    assert_eq!(report.deployments.len(), 1);
    assert_eq!(report.deployments[0].pods.len(), 1);

    // On the wire it carries the request id, and the hub's validation accepts it.
    let wire = FromAgent::Reply(AgentReply::Cluster { request_id, report });
    let back = FromAgent::from_proto(wire.clone().into_proto()).unwrap();
    assert_eq!(back, Some(wire));
}

#[tokio::test(start_paused = true)]
async fn a_report_asked_for_before_the_lists_are_in_is_an_io_error_not_an_empty_report() {
    let kube = FakeKube::new("sit1");
    kube.deny_everything(403);
    let rig = rig_with(kube, Duration::from_secs(3));
    let result = op(rig
        .dispatcher
        .handle(
            HubCommand::RequestClusterReport {
                request_id: rid("c2"),
            },
            LIMITS,
        )
        .await);
    assert_eq!((result.ok, result.error), (false, Some(OpError::Io)));
    assert_eq!(result.request_id.as_str(), "c2");
}

#[tokio::test(start_paused = true)]
async fn restart_deployment_patches_and_answers_ok() {
    let rig = rig();
    let result = op(rig
        .dispatcher
        .handle(
            HubCommand::RestartDeployment {
                request_id: rid("r1"),
                service: svc("sit1", "svc-a"),
            },
            LIMITS,
        )
        .await);
    assert!(result.ok);
    assert_eq!((result.error, result.current_hash), (None, None));
    let patched = rig.kube.object(Kind::Deployment, "sit1", "svc-a").unwrap();
    assert_eq!(
        patched["spec"]["template"]["metadata"]["annotations"]["kubectl.kubernetes.io/restartedAt"],
        "2026-10-10T12:00:00Z"
    );
}

#[tokio::test(start_paused = true)]
async fn restart_failures_map_to_the_codes_in_the_table() {
    let rig = rig();
    let cases = [
        (svc("sit1", "missing"), OpError::NotFound),
        (svc("kube-system", "coredns"), OpError::Denied),
    ];
    for (service, expected) in cases {
        let result = op(rig
            .dispatcher
            .handle(
                HubCommand::RestartDeployment {
                    request_id: rid("r2"),
                    service: service.clone(),
                },
                LIMITS,
            )
            .await);
        assert!(!result.ok, "{service:?}");
        assert_eq!(result.error, Some(expected), "{service:?}");
    }
    rig.kube.deny_everything(500);
    let result = op(rig
        .dispatcher
        .handle(
            HubCommand::RestartDeployment {
                request_id: rid("r3"),
                service: svc("sit1", "svc-a"),
            },
            LIMITS,
        )
        .await);
    assert_eq!(result.error, Some(OpError::Io));
}

#[tokio::test(start_paused = true)]
async fn the_answer_to_a_failed_restart_names_no_namespace_and_no_server_text() {
    let rig = rig();
    rig.kube.deny_everything(403);
    let reply = rig
        .dispatcher
        .handle(
            HubCommand::RestartDeployment {
                request_id: rid("r4"),
                service: svc("sit1", "svc-a"),
            },
            LIMITS,
        )
        .await;
    let shown = format!("{reply:?}");
    assert!(
        !shown.contains("denied by the fake RBAC") && !shown.contains("svc-a"),
        "{shown}"
    );
}

#[tokio::test(start_paused = true)]
async fn without_a_cluster_the_commands_are_still_unsupported() {
    let dir = TempDir::new().unwrap();
    let dispatcher = Dispatcher::new(FileOps::new(
        NfsRoot::open(dir.path()).unwrap(),
        support::recording_edits::RecordingEdits::new(),
    ));
    for command in [
        HubCommand::RequestClusterReport {
            request_id: rid("u1"),
        },
        HubCommand::RestartDeployment {
            request_id: rid("u2"),
            service: svc("sit1", "svc-a"),
        },
    ] {
        let result = op(dispatcher.handle(command, LIMITS).await);
        assert_eq!(result.error, Some(OpError::Unsupported));
    }
}
