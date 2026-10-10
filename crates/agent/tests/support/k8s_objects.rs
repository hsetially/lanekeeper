//! Manifests for the fake Kubernetes API server: Deployments, Pods and Jobs as the real API server would send them,
//! including the parts the agent must not keep (managed fields, annotations, Secret references).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};

/// A Deployment whose selector is `app=<name>`, with one container.
#[derive(Debug, Clone)]
pub struct DeploymentSpec {
    namespace: String,
    name: String,
    labels: Vec<(String, String)>,
    annotations: Vec<(String, String)>,
    env: Vec<Value>,
    selector: Value,
}

pub fn deployment(namespace: &str, name: &str) -> DeploymentSpec {
    DeploymentSpec {
        namespace: namespace.to_owned(),
        name: name.to_owned(),
        labels: Vec::new(),
        annotations: Vec::new(),
        env: Vec::new(),
        selector: json!({ "matchLabels": { "app": name } }),
    }
}

impl DeploymentSpec {
    pub fn label(mut self, key: &str, value: &str) -> Self {
        self.labels.push((key.to_owned(), value.to_owned()));
        self
    }

    pub fn annotation(mut self, key: &str, value: &str) -> Self {
        self.annotations.push((key.to_owned(), value.to_owned()));
        self
    }

    /// An environment variable with a literal value.
    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.push(json!({ "name": name, "value": value }));
        self
    }

    /// An environment variable that comes from a Secret: no value in the manifest.
    pub fn env_from_secret(mut self, name: &str, secret: &str) -> Self {
        self.env.push(json!({
            "name": name,
            "valueFrom": { "secretKeyRef": { "name": secret, "key": "value" } },
        }));
        self
    }

    /// The Helm labels a chart puts on what it renders.
    pub fn helm(self, chart: &str, instance: &str) -> Self {
        self.label("helm.sh/chart", chart)
            .label("app.kubernetes.io/instance", instance)
    }

    pub fn selector(mut self, selector: Value) -> Self {
        self.selector = selector;
        self
    }

    pub fn build(self) -> Value {
        let labels: serde_json::Map<String, Value> = self
            .labels
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        let annotations: serde_json::Map<String, Value> = self
            .annotations
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        json!({
            "kind": "Deployment",
            "apiVersion": "apps/v1",
            "metadata": {
                "name": self.name,
                "namespace": self.namespace,
                "labels": labels,
                "annotations": annotations,
                // The part of a real object that is large and of no use to the agent.
                "managedFields": [{
                    "manager": "kubectl",
                    "operation": "Apply",
                    "fieldsV1": { "f:metadata": { "f:annotations": { "f:MANAGED_FIELDS_MARKER": {} } } },
                }],
            },
            "spec": {
                "replicas": 1,
                "selector": self.selector,
                "template": {
                    "metadata": { "labels": { "app": self.name } },
                    "spec": {
                        "containers": [{
                            "name": "main",
                            "image": "registry.example.com/IMAGE_MARKER:1",
                            "env": self.env,
                        }],
                    },
                },
            },
        })
    }
}

/// A running Pod labelled `app=<app>`, started at `started` (RFC 3339). It carries what a real pod does and the agent
/// has no use for: managed fields, annotations, conditions and container statuses.
pub fn pod(namespace: &str, name: &str, app: &str, started: &str) -> Value {
    json!({
        "kind": "Pod",
        "apiVersion": "v1",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": { "app": app },
            "annotations": { "POD_ANNOTATION_MARKER": "x" },
            "managedFields": [
                { "manager": "kube-controller-manager", "operation": "Update", "apiVersion": "v1",
                  "fieldsType": "FieldsV1", "fieldsV1": { "f:metadata": { "f:labels": { "f:MANAGED_FIELDS_MARKER": {} },
                  "f:generateName": {}, "f:ownerReferences": { "k:{\"uid\":\"0\"}": { "f:name": {}, "f:uid": {} } } } } },
                { "manager": "kubelet", "operation": "Update", "apiVersion": "v1", "subresource": "status",
                  "fieldsType": "FieldsV1", "fieldsV1": { "f:status": { "f:conditions": { "k:{\"type\":\"Ready\"}": {} },
                  "f:containerStatuses": {}, "f:hostIP": {}, "f:podIP": {}, "f:startTime": {} } } },
            ],
        },
        "spec": { "containers": [{ "name": "main", "image": "registry.example.com/IMAGE_MARKER:1",
                                   "resources": { "limits": { "cpu": "1", "memory": "1Gi" } } }] },
        "status": {
            "phase": "Running",
            "startTime": started,
            "conditions": [
                { "type": "Ready", "status": "True", "lastTransitionTime": started },
                { "type": "ContainersReady", "status": "True", "lastTransitionTime": started },
            ],
            "containerStatuses": [{ "name": "main", "ready": true, "restartCount": 0,
                                    "image": "registry.example.com/IMAGE_MARKER:1",
                                    "containerID": "containerd://0123456789abcdef0123456789abcdef" }],
            "hostIP": "10.0.0.1", "podIP": "10.1.0.5",
        },
    })
}

/// A Pod that is being deleted (it still shows as running until the kubelet is done).
pub fn terminating_pod(namespace: &str, name: &str, app: &str, started: &str) -> Value {
    let mut pod = pod(namespace, name, app, started);
    pod["metadata"]["deletionTimestamp"] = json!("2026-10-10T12:00:00Z");
    pod
}

/// A Pod in the given phase (`Succeeded`, `Failed`, `Pending`).
pub fn pod_in_phase(namespace: &str, name: &str, app: &str, started: &str, phase: &str) -> Value {
    let mut pod = pod(namespace, name, app, started);
    pod["status"]["phase"] = json!(phase);
    pod
}

/// A Job with these labels. It is running until [`finish_job`] is applied.
pub fn job(namespace: &str, name: &str, uid: &str, labels: &[(&str, &str)]) -> Value {
    let labels: serde_json::Map<String, Value> = labels
        .iter()
        .map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned())))
        .collect();
    json!({
        "kind": "Job",
        "apiVersion": "batch/v1",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "uid": uid,
            "labels": labels,
            "creationTimestamp": "2026-10-10T11:59:00Z",
        },
        "spec": { "template": { "spec": { "containers": [{ "name": "load", "image": "x" }], "restartPolicy": "Never" } } },
        "status": { "active": 1, "startTime": "2026-10-10T11:59:05Z" },
    })
}

/// The same Job, finished.
pub fn finish_job(mut job: Value) -> Value {
    job["status"] = json!({
        "succeeded": 1,
        "startTime": "2026-10-10T11:59:05Z",
        "completionTime": "2026-10-10T12:03:00Z",
        "conditions": [{ "type": "Complete", "status": "True" }],
    });
    job
}
