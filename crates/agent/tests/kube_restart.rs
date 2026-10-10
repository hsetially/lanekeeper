//! Restarting a Deployment (T6, D28, S17): one merge patch of the pod template's `restartedAt`, in the configured
//! namespaces only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::identity::KubeError;
use agent::kube::{RestartError, Restarter};
use domain::{OpError, ServiceRef};
use serde_json::{Value, json};
use support::clock::TestClock;
use support::fake_kube::{FakeKube, Kind};
use support::k8s_objects::deployment;

/// 2026-10-10T12:00:00Z.
const NOON_MS: i64 = 1_791_633_600_000;

fn restarter(kube: &FakeKube, namespaces: &[&str]) -> Restarter {
    Restarter::new(
        kube.client(),
        namespaces.iter().copied(),
        Arc::new(TestClock::starting_at(NOON_MS)),
    )
}

fn svc(namespace: &str, name: &str) -> ServiceRef {
    ServiceRef::new(namespace, name).unwrap()
}

fn annotation(kube: &FakeKube, namespace: &str, name: &str) -> Option<String> {
    kube.object(Kind::Deployment, namespace, name).unwrap()["spec"]["template"]["metadata"]["annotations"]
        ["kubectl.kubernetes.io/restartedAt"]
        .as_str()
        .map(str::to_owned)
}

#[tokio::test(start_paused = true)]
async fn restart_patches_annotation() {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("sit1", "csp-configuration-server").build());
    let before = kube
        .object(Kind::Deployment, "sit1", "csp-configuration-server")
        .unwrap();

    restarter(&kube, &["sit1"])
        .restart(&svc("sit1", "csp-configuration-server"))
        .await
        .unwrap();

    // Exactly one call, the merge patch `kubectl rollout restart` sends, and nothing else in it.
    let calls = kube.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = &calls[0];
    assert_eq!(call.method, "PATCH");
    assert_eq!(
        call.path,
        "/apis/apps/v1/namespaces/sit1/deployments/csp-configuration-server"
    );
    assert_eq!(call.content_type.as_deref(), Some("application/merge-patch+json"));
    assert_eq!(
        call.query_param("fieldManager").as_deref(),
        Some("lanekeeper-agent")
    );
    let body: Value = serde_json::from_slice(&call.body).unwrap();
    assert_eq!(
        body,
        json!({ "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": "2026-10-10T12:00:00Z"
        } } } } })
    );

    // On the object: the annotation is on the pod template, which is what makes the controller roll the pods, and
    // nothing else changed.
    assert_eq!(
        annotation(&kube, "sit1", "csp-configuration-server").as_deref(),
        Some("2026-10-10T12:00:00Z")
    );
    let mut after = kube
        .object(Kind::Deployment, "sit1", "csp-configuration-server")
        .unwrap();
    after["metadata"]["resourceVersion"] = before["metadata"]["resourceVersion"].clone();
    after["spec"]["template"]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("annotations");
    assert_eq!(after, before);
}

#[tokio::test(start_paused = true)]
async fn the_stamp_follows_the_clock() {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("sit1", "svc").build());
    let r = restarter(&kube, &["sit1"]);
    r.restart(&svc("sit1", "svc")).await.unwrap();
    assert_eq!(
        annotation(&kube, "sit1", "svc").as_deref(),
        Some("2026-10-10T12:00:00Z")
    );
    tokio::time::advance(Duration::from_secs(3_725)).await;
    r.restart(&svc("sit1", "svc")).await.unwrap();
    assert_eq!(
        annotation(&kube, "sit1", "svc").as_deref(),
        Some("2026-10-10T13:02:05Z")
    );
}

#[tokio::test(start_paused = true)]
async fn restart_unknown_deployment_is_not_found() {
    let kube = FakeKube::new("sit1");
    let error = restarter(&kube, &["sit1"])
        .restart(&svc("sit1", "no-such-service"))
        .await
        .unwrap_err();
    assert_eq!(error, RestartError::NotFound);
    assert_eq!(error.code(), OpError::NotFound);
    // It asked once, and nothing was created by asking.
    assert_eq!(kube.calls().len(), 1);
    assert!(kube.object(Kind::Deployment, "sit1", "no-such-service").is_none());
}

#[tokio::test(start_paused = true)]
async fn restart_refused_outside_configured_namespaces() {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("kube-system", "coredns").build());
    kube.apply(deployment("sit2", "svc").build());
    let r = restarter(&kube, &["sit1"]);
    for (namespace, name) in [
        ("kube-system", "coredns"),
        ("sit2", "svc"),
        ("sit1x", "svc"),
        ("", "x"),
    ] {
        // `ServiceRef` rejects an empty namespace; the others are valid names in namespaces the agent was not given.
        let Ok(service) = ServiceRef::new(namespace, name) else {
            continue;
        };
        let error = r.restart(&service).await.unwrap_err();
        assert_eq!(error, RestartError::NamespaceNotAllowed, "{namespace}/{name}");
        assert_eq!(error.code(), OpError::Denied);
    }
    // Refused before the API server was asked anything.
    assert!(kube.calls().is_empty(), "{:?}", kube.calls());
    assert_eq!(annotation(&kube, "kube-system", "coredns"), None);
    assert_eq!(annotation(&kube, "sit2", "svc"), None);
}

#[tokio::test(start_paused = true)]
async fn a_refusal_by_the_api_server_is_an_io_error_with_the_status() {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("sit1", "svc").build());
    kube.deny_everything(403);
    let error = restarter(&kube, &["sit1"])
        .restart(&svc("sit1", "svc"))
        .await
        .unwrap_err();
    match error {
        RestartError::Api(KubeError::Status { status: 403, .. }) => {}
        other => panic!("expected the 403 to be reported, got {other:?}"),
    }
    assert_eq!(error.code(), OpError::Io);
    // The error says what happened and not what the server said.
    assert!(!error.to_string().contains("denied by the fake RBAC"), "{error}");
}

#[tokio::test(start_paused = true)]
async fn the_api_server_not_answering_is_an_error_after_the_bound_and_not_a_hang() {
    let kube = FakeKube::new("sit1");
    kube.apply(deployment("sit1", "svc").build());
    kube.stall_everything();
    let started = tokio::time::Instant::now();
    let error = restarter(&kube, &["sit1"])
        .restart(&svc("sit1", "svc"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, RestartError::Api(KubeError::Timeout { .. })),
        "{error:?}"
    );
    assert_eq!(error.code(), OpError::Io);
    // Bounded (code rule 5): it gave up after the call timeout, not later.
    assert_eq!(started.elapsed(), Duration::from_secs(15));
}
