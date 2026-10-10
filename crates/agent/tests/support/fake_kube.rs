//! A fake Kubernetes API server for Secrets, as a tower service behind a real `kube::Client`.
//!
//! It keeps Secrets in memory, speaks the JSON the real server does for `GET` and `PUT` (including `resourceVersion`
//! conflicts and `Status` errors), and records every request it receives, bodies included, so a test can assert what
//! the agent asked for and what it did not. Anything else is answered `403 Forbidden`, like the agent's RBAC would.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::client::Body;

/// One request the fake API server received.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub body: Bytes,
}

#[derive(Debug, Clone, Default)]
struct StoredSecret {
    data: BTreeMap<String, Vec<u8>>,
    labels: BTreeMap<String, String>,
    secret_type: Option<String>,
    resource_version: u64,
}

#[derive(Debug, Default)]
struct State {
    namespace: String,
    secrets: BTreeMap<String, StoredSecret>,
    calls: Vec<Call>,
    /// Statuses to answer the next `PUT`s with, before they are applied.
    put_failures: VecDeque<u16>,
    /// Answer every request with this status.
    deny_all: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct FakeKube {
    state: Arc<Mutex<State>>,
}

impl FakeKube {
    pub fn new(namespace: &str) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                namespace: namespace.to_owned(),
                ..State::default()
            })),
        }
    }

    /// Add a Secret with these entries.
    pub fn with_secret(self, name: &str, data: &[(&str, &[u8])]) -> Self {
        self.state.lock().unwrap().secrets.insert(
            name.to_owned(),
            StoredSecret {
                data: data.iter().map(|(k, v)| ((*k).to_owned(), v.to_vec())).collect(),
                resource_version: 1,
                ..StoredSecret::default()
            },
        );
        self
    }

    /// Add a label and a type to a Secret, to check that a rewrite keeps them.
    pub fn decorate(&self, name: &str, label: (&str, &str), secret_type: &str) {
        let mut state = self.state.lock().unwrap();
        let secret = state.secrets.get_mut(name).unwrap();
        secret.labels.insert(label.0.to_owned(), label.1.to_owned());
        secret.secret_type = Some(secret_type.to_owned());
    }

    /// A real client whose requests are answered by this fake. Call it inside a tokio runtime.
    pub fn client(&self) -> kube::Client {
        let namespace = self.state.lock().unwrap().namespace.clone();
        kube::Client::new(
            FakeService {
                state: Arc::clone(&self.state),
            },
            namespace,
        )
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn secret_data(&self, name: &str) -> Option<BTreeMap<String, Vec<u8>>> {
        self.state
            .lock()
            .unwrap()
            .secrets
            .get(name)
            .map(|s| s.data.clone())
    }

    pub fn secret_labels(&self, name: &str) -> BTreeMap<String, String> {
        self.state.lock().unwrap().secrets[name].labels.clone()
    }

    pub fn secret_type(&self, name: &str) -> Option<String> {
        self.state.lock().unwrap().secrets[name].secret_type.clone()
    }

    pub fn resource_version(&self, name: &str) -> u64 {
        self.state.lock().unwrap().secrets[name].resource_version
    }

    /// Fail the next `PUT`s with these statuses, one each, then behave normally.
    pub fn fail_next_puts(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .put_failures
            .extend(statuses.iter().copied());
    }

    /// Answer every request with `status`, as RBAC does for a verb the agent was not granted.
    pub fn deny_everything(&self, status: u16) {
        self.state.lock().unwrap().deny_all = Some(status);
    }

    /// The path of a Secret in the namespace.
    pub fn secret_path(&self, name: &str) -> String {
        let namespace = self.state.lock().unwrap().namespace.clone();
        format!("/api/v1/namespaces/{namespace}/secrets/{name}")
    }
}

#[derive(Clone)]
struct FakeService {
    state: Arc<Mutex<State>>,
}

type ServiceFuture = Pin<Box<dyn Future<Output = Result<Response<Full<Bytes>>, Infallible>> + Send>>;

impl tower::Service<Request<Body>> for FakeService {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = ServiceFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let body = body.collect().await.unwrap().to_bytes();
            Ok(handle(&state, &parts, &body))
        })
    }
}

fn handle(state: &Mutex<State>, parts: &http::request::Parts, body: &Bytes) -> Response<Full<Bytes>> {
    let mut state = state.lock().unwrap();
    let call = Call {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        body: body.clone(),
    };
    state.calls.push(call.clone());
    if let Some(status) = state.deny_all {
        return status_error(status, "Forbidden", "denied by the fake RBAC");
    }
    let prefix = format!("/api/v1/namespaces/{}/secrets/", state.namespace);
    let Some(name) = call
        .path
        .strip_prefix(&prefix)
        .filter(|n| !n.is_empty() && !n.contains('/'))
    else {
        return status_error(
            403,
            "Forbidden",
            "the agent may only get and update named Secrets",
        );
    };
    let name = name.to_owned();
    match call.method.as_str() {
        "GET" => match state.secrets.get(&name) {
            Some(secret) => json(200, &render(&state.namespace, &name, secret)),
            None => status_error(404, "NotFound", &format!("secrets \"{name}\" not found")),
        },
        "PUT" => {
            let Some(current) = state.secrets.get(&name).cloned() else {
                return status_error(404, "NotFound", &format!("secrets \"{name}\" not found"));
            };
            if let Some(status) = state.put_failures.pop_front() {
                let reason = if status == 409 { "Conflict" } else { "Forbidden" };
                return status_error(status, reason, "scripted failure");
            }
            let sent: Secret = serde_json::from_slice(body).unwrap();
            let sent_version = sent.metadata.resource_version.clone().unwrap_or_default();
            if sent_version != current.resource_version.to_string() {
                return status_error(409, "Conflict", "the object has been modified");
            }
            let data: BTreeMap<String, Vec<u8>> = sent
                .data
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v.0))
                .collect();
            let changed = data != current.data;
            let updated = StoredSecret {
                data,
                labels: sent.metadata.labels.unwrap_or_default(),
                secret_type: sent.type_,
                resource_version: current.resource_version + u64::from(changed),
            };
            state.secrets.insert(name.clone(), updated.clone());
            json(200, &render(&state.namespace, &name, &updated))
        }
        _ => status_error(
            403,
            "Forbidden",
            "the agent may only get and update named Secrets",
        ),
    }
}

fn render(namespace: &str, name: &str, secret: &StoredSecret) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            resource_version: Some(secret.resource_version.to_string()),
            labels: (!secret.labels.is_empty()).then(|| secret.labels.clone()),
            ..ObjectMeta::default()
        },
        data: Some(
            secret
                .data
                .iter()
                .map(|(k, v)| (k.clone(), ByteString(v.clone())))
                .collect(),
        ),
        type_: secret.secret_type.clone(),
        ..Secret::default()
    }
}

fn json(status: u16, value: &impl serde::Serialize) -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(value).unwrap())))
        .unwrap()
}

fn status_error(code: u16, reason: &str, message: &str) -> Response<Full<Bytes>> {
    json(
        code,
        &serde_json::json!({
            "kind": "Status",
            "apiVersion": "v1",
            "metadata": {},
            "status": "Failure",
            "message": message,
            "reason": reason,
            "code": code,
        }),
    )
}
