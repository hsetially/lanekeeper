//! What the agent may ask of the Kubernetes API (S17, acceptance 4): the manifest in `RBAC.md`, the lint that rejects a
//! Secret list, and a check that everything the agent really calls is inside the manifest.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use agent::config::KubeName;
use agent::identity::jointoken::{JoinTokenSource, KubeSecretJoinToken};
use agent::identity::store::{CertStore, KubeCertStore};
use agent::kube::helm::HelmFilter;
use agent::kube::jobs::JobMatcher;
use agent::kube::{Cluster, ClusterOps, WatchConfig};
use domain::ServiceRef;
use support::clock::TestClock;
use support::fake_kube::{Call, FakeKube};
use support::k8s_objects::{deployment, job, pod};
use support::rbac_lint::{Role, lint, permits, roles_from_markdown, roles_from_yaml};

const CERT_SECRET: &str = "lanekeeper-agent-cert";
const TOKEN_SECRET: &str = "lanekeeper-agent-join-token";

fn manifest() -> Vec<Role> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/RBAC.md")).expect("RBAC.md");
    let roles = roles_from_markdown(&text);
    assert!(!roles.is_empty(), "no Role in a fenced yaml block of RBAC.md");
    roles
}

#[test]
fn rbac_manifest_has_no_secret_list() {
    let roles = manifest();
    let problems = lint(&roles);
    assert!(problems.is_empty(), "{problems:#?}");
    // Said plainly, apart from the lint: no rule gives list, watch, create, delete or patch on Secrets, and none gives
    // anything on Secrets without naming them.
    for rule in roles.iter().flat_map(|r| &r.rules) {
        if rule.resources.iter().any(|r| r == "secrets") {
            assert!(!rule.resource_names.is_empty(), "{rule:?}");
            for verb in [
                "list",
                "watch",
                "create",
                "delete",
                "deletecollection",
                "patch",
                "*",
            ] {
                assert!(
                    !rule.verbs.iter().any(|v| v == verb),
                    "{verb} on Secrets: {rule:?}"
                );
            }
        }
    }
    assert!(
        roles.iter().all(|r| r.kind == "Role"),
        "namespaced Roles only, no ClusterRole"
    );
}

#[test]
fn rbac_lint_rejects_planted_secret_list() {
    let planted = [
        (
            "list on named Secrets",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [get, list]}\n",
            "the verb list on Secrets",
        ),
        (
            "watch on named Secrets",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [watch]}\n",
            "the verb watch on Secrets",
        ),
        (
            "get on every Secret",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], verbs: [get]}\n",
            "without resourceNames",
        ),
        (
            "list on every Secret",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], verbs: [list]}\n",
            "the verb list on Secrets",
        ),
        (
            "create and delete",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [create, delete]}\n",
            "the verb create on Secrets",
        ),
        (
            "wildcard verbs",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [\"*\"]}\n",
            "a wildcard",
        ),
        (
            "wildcard resources",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [\"*\"], verbs: [get]}\n",
            "a wildcard",
        ),
        (
            "a ClusterRole for Secrets",
            "kind: ClusterRole\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [get]}\n",
            "ClusterRole grants access to Secrets",
        ),
        (
            "Secrets next to pods",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [pods, secrets], resourceNames: [a], verbs: [get]}\n",
            "share a rule",
        ),
        (
            "ConfigMaps",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [configmaps], verbs: [get, list, watch]}\n",
            "configmaps",
        ),
        (
            "delete on Deployments",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [apps], resources: [deployments], verbs: [delete]}\n",
            "the verb delete on deployments",
        ),
        (
            "exec in pods",
            "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [\"\"], resources: [pods/exec], verbs: [create]}\n",
            "pods/exec",
        ),
    ];
    for (what, yaml, expected) in planted {
        let problems = lint(&roles_from_yaml(yaml));
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "{what}: expected a problem mentioning {expected:?}, got {problems:?}"
        );
    }
    // A manifest that is only what the agent needs is clean.
    let clean = "kind: Role\nmetadata: {name: x}\nrules:\n- {apiGroups: [apps], resources: [deployments], verbs: [get, list, watch, patch]}\n- {apiGroups: [\"\"], resources: [secrets], resourceNames: [a], verbs: [get, update]}\n";
    assert!(lint(&roles_from_yaml(clean)).is_empty());
}

#[test]
fn rbac_secret_rules_are_named_and_minimal() {
    let roles = manifest();
    let secret_rules: Vec<_> = roles
        .iter()
        .flat_map(|r| &r.rules)
        .filter(|rule| rule.resources.iter().any(|r| r == "secrets"))
        .collect();
    // Exactly two grants (A2): the certificate Secret (read it, write the renewed certificate), and the join-token
    // Secret (read it, only when Workload Identity is unavailable).
    assert_eq!(secret_rules.len(), 2, "{secret_rules:?}");
    let cert = secret_rules
        .iter()
        .find(|r| r.resource_names == [CERT_SECRET])
        .expect("the certificate Secret rule");
    let mut verbs = cert.verbs.clone();
    verbs.sort();
    assert_eq!(verbs, ["get", "update"]);
    let token = secret_rules
        .iter()
        .find(|r| r.resource_names == [TOKEN_SECRET])
        .expect("the join-token Secret rule");
    assert_eq!(token.verbs, ["get"]);
    // And the same two names the code reads (the examples in `valid_env` are the chart's defaults).
    assert!(permits(&roles, "", "secrets", "get", Some(CERT_SECRET)));
    assert!(permits(&roles, "", "secrets", "update", Some(CERT_SECRET)));
    assert!(permits(&roles, "", "secrets", "get", Some(TOKEN_SECRET)));
    assert!(!permits(&roles, "", "secrets", "update", Some(TOKEN_SECRET)));
    assert!(!permits(&roles, "", "secrets", "get", Some("some-other-secret")));
    assert!(!permits(&roles, "", "secrets", "list", None));
    assert!(!permits(&roles, "", "secrets", "get", None));
}

#[test]
fn rbac_grants_the_workload_verbs_the_prompt_lists() {
    let roles = manifest();
    for (group, resource) in [("apps", "deployments"), ("", "pods"), ("batch", "jobs")] {
        for verb in ["get", "list", "watch"] {
            assert!(permits(&roles, group, resource, verb, None), "{verb} {resource}");
        }
    }
    assert!(permits(&roles, "apps", "deployments", "patch", None));
    for (group, resource) in [("", "pods"), ("batch", "jobs")] {
        assert!(!permits(&roles, group, resource, "patch", None), "{resource}");
        assert!(!permits(&roles, group, resource, "delete", None), "{resource}");
    }
    assert!(!permits(&roles, "apps", "deployments", "delete", None));
    assert!(!permits(&roles, "", "configmaps", "get", None));
}

/// The `(group, resource, verb, name)` the real API server would authorise a recorded call as.
fn authorised_as(call: &Call) -> (&'static str, String, &'static str, Option<String>) {
    let parts: Vec<&str> = call.path.trim_start_matches('/').split('/').collect();
    // /api/v1/namespaces/{ns}/{resource}[/{name}]  or  /apis/{group}/{version}/namespaces/{ns}/{resource}[/{name}]
    let (group, rest): (&'static str, &[&str]) = match parts.as_slice() {
        ["api", "v1", rest @ ..] => ("", rest),
        ["apis", "apps", "v1", rest @ ..] => ("apps", rest),
        ["apis", "batch", "v1", rest @ ..] => ("batch", rest),
        other => panic!("a path outside the API groups the agent uses: {other:?}"),
    };
    let ["namespaces", _namespace, resource, tail @ ..] = rest else {
        panic!("not a namespaced request: {}", call.path);
    };
    let name = tail.first().map(|n| (*n).to_owned());
    let verb = match (call.method.as_str(), name.is_some()) {
        ("GET", true) => "get",
        ("GET", false) if call.query_param("watch").as_deref() == Some("true") => "watch",
        ("GET", false) => "list",
        ("PUT", true) => "update",
        ("PATCH", true) => "patch",
        ("POST", _) => "create",
        ("DELETE", true) => "delete",
        ("DELETE", false) => "deletecollection",
        (m, _) => panic!("unexpected method {m}"),
    };
    (group, (*resource).to_owned(), verb, name)
}

#[tokio::test(start_paused = true)]
async fn fake_api_log_has_no_configmap_and_only_named_secret_calls() {
    let roles = manifest();
    let kube = FakeKube::new("lanekeeper")
        .with_secret(CERT_SECRET, &[("tls.crt", b""), ("tls.key", b"")])
        .with_secret(TOKEN_SECRET, &[("token", b"lk_join_abc")]);
    kube.apply(deployment("sit1", "svc-a").build());
    kube.apply(pod("sit1", "svc-a-1", "svc-a", "2026-10-10T11:00:00Z"));
    kube.apply(job("sit1", "csp-dataload-1", "uid-1", &[]));

    // Everything the agent does on the Kubernetes API: the identity calls, the watchers, a report and a restart.
    let client = kube.client();
    let cert_store = KubeCertStore::new(client.clone(), &KubeName::subdomain(CERT_SECRET).unwrap());
    assert!(cert_store.load().await.unwrap().is_none());
    cert_store.probe().await.unwrap();
    let token = KubeSecretJoinToken::new(client.clone(), &KubeName::subdomain(TOKEN_SECRET).unwrap());
    token.join_token().await.unwrap();

    let config = WatchConfig::new(
        vec![KubeName::label("sit1").unwrap()],
        HelmFilter::new(&[]).unwrap(),
        JobMatcher::new(&[domain::ShortText::parse("*dataload*").unwrap()], None).unwrap(),
    );
    let (cluster, _reports) = Cluster::start_with(
        &client,
        config,
        Arc::new(TestClock::starting_at(1_791_633_600_000)),
    );
    cluster.full_report().await.unwrap();
    cluster
        .restart(&ServiceRef::new("sit1", "svc-a").unwrap())
        .await
        .unwrap();
    kube.apply(pod("sit1", "svc-a-2", "svc-a", "2026-10-10T12:00:00Z"));
    tokio::time::sleep(Duration::from_secs(5)).await;

    let calls = kube.calls();
    assert!(calls.len() > 8, "the scenario made {} calls", calls.len());
    for call in &calls {
        assert!(!call.path.contains("configmaps"), "{call:?}");
        let (group, resource, verb, name) = authorised_as(call);
        // The real API server would allow it only if the manifest does.
        assert!(
            permits(&roles, group, &resource, verb, name.as_deref()),
            "RBAC.md does not allow {verb} {resource} {name:?}: {call:?}"
        );
        if resource == "secrets" {
            let name = name.expect("a Secret call names its Secret");
            assert!(name == CERT_SECRET || name == TOKEN_SECRET, "{name}");
            assert!(matches!(verb, "get" | "update"), "{verb}");
        }
    }
    // And the scenario really did use each kind of call it is meant to cover.
    let seen = |g: &str, r: &str, v: &str| {
        calls
            .iter()
            .any(|c| authorised_as(c) == (g, r.to_owned(), v, authorised_as(c).3))
    };
    for (g, r, v) in [
        ("", "secrets", "get"),
        ("", "secrets", "update"),
        ("apps", "deployments", "list"),
        ("apps", "deployments", "watch"),
        ("apps", "deployments", "patch"),
        ("", "pods", "list"),
        ("", "pods", "watch"),
        ("batch", "jobs", "list"),
        ("batch", "jobs", "watch"),
    ] {
        assert!(seen(g, r, v), "the scenario never made a {v} call on {r}");
    }
}
