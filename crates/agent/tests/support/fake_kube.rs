//! A fake Kubernetes API server behind a real `kube::Client`, as a tower service.
//!
//! Two halves, both in memory:
//!
//! - Secrets (T2): `GET` and `PUT` on named Secrets, with `resourceVersion` conflicts and `Status` errors.
//! - The cluster (T6): Deployments, Pods and Jobs, with paged `list`, streaming `watch` that replays everything after a
//!   `resourceVersion`, and the merge patch a restart uses. Tests change the cluster with [`FakeKube::apply`] and
//!   [`FakeKube::delete`], and the watchers see events exactly as they would from a real server.
//!
//! Every request is recorded, bodies and content types included, so a test can assert what the agent asked for and what
//! it did not. Anything the agent has no RBAC for (any other path or verb) is answered `403 Forbidden`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::client::Body;
use serde_json::{Value, json};
use tokio::sync::watch;

/// One request the fake API server received.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub content_type: Option<String>,
    pub body: Bytes,
}

impl Call {
    /// The value of a query parameter, as sent (no percent-decoding: the agent sends none that needs it).
    pub fn query_param(&self, key: &str) -> Option<String> {
        query_param(self.query.as_deref()?, key)
    }
}

/// The kinds of object the fake cluster holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Deployment,
    Pod,
    Job,
}

impl Kind {
    fn from_kind_field(kind: &str) -> Self {
        match kind {
            "Deployment" => Self::Deployment,
            "Pod" => Self::Pod,
            "Job" => Self::Job,
            other => panic!("the fake cluster has no {other}"),
        }
    }

    fn resource(self) -> &'static str {
        match self {
            Self::Deployment => "deployments",
            Self::Pod => "pods",
            Self::Job => "jobs",
        }
    }

    fn api_version(self) -> &'static str {
        match self {
            Self::Deployment => "apps/v1",
            Self::Pod => "v1",
            Self::Job => "batch/v1",
        }
    }

    fn kind_name(self) -> &'static str {
        match self {
            Self::Deployment => "Deployment",
            Self::Pod => "Pod",
            Self::Job => "Job",
        }
    }

    /// The URL prefix up to and including the namespace segment's parent: `/apis/apps/v1`, `/api/v1`, `/apis/batch/v1`.
    fn prefix(self) -> &'static str {
        match self {
            Self::Deployment => "/apis/apps/v1",
            Self::Pod => "/api/v1",
            Self::Job => "/apis/batch/v1",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct StoredSecret {
    data: BTreeMap<String, Vec<u8>>,
    labels: BTreeMap<String, String>,
    secret_type: Option<String>,
    resource_version: u64,
}

/// One change to the cluster, kept so that a watch can replay what it missed.
#[derive(Debug, Clone)]
struct LogEntry {
    rv: u64,
    kind: Kind,
    namespace: String,
    event: &'static str,
    object: Value,
}

#[derive(Debug, Default)]
struct State {
    namespace: String,
    secrets: BTreeMap<String, StoredSecret>,
    calls: Vec<Call>,
    /// Statuses to answer the next `PUT`s with, before they are applied.
    put_failures: VecDeque<u16>,
    /// Statuses to answer the next list calls with, for one kind or for any.
    list_failures: VecDeque<(Option<Kind>, u16)>,
    /// Answer every request with this status.
    deny_all: Option<u16>,
    /// Never answer: the API server is up and says nothing.
    stalled: bool,
    /// The cluster: the latest version of every object, and the log of changes.
    objects: BTreeMap<(Kind, String, String), Value>,
    log: Vec<LogEntry>,
    rv: u64,
    /// Watches that started before this epoch end (the connection dropped).
    epoch: u64,
}

#[derive(Debug, Clone)]
pub struct FakeKube {
    state: Arc<Mutex<State>>,
    /// Bumped on every change to the cluster, so that open watches wake up.
    changes: Arc<watch::Sender<u64>>,
}

impl FakeKube {
    pub fn new(namespace: &str) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                namespace: namespace.to_owned(),
                ..State::default()
            })),
            changes: Arc::new(watch::channel(0).0),
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
                changes: Arc::clone(&self.changes),
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

    /// Stop answering requests (they are still recorded), as an overloaded API server does.
    pub fn stall_everything(&self) {
        self.state.lock().unwrap().stalled = true;
    }

    /// The path of a Secret in the namespace.
    pub fn secret_path(&self, name: &str) -> String {
        let namespace = self.state.lock().unwrap().namespace.clone();
        format!("/api/v1/namespaces/{namespace}/secrets/{name}")
    }

    // ------------------------------------------------------------ the cluster

    /// Create or replace an object. `object` is a full manifest (`kind`, `metadata.name` and `metadata.namespace`);
    /// the fake sets `resourceVersion` and a `uid` when there is none, and a watch sees `ADDED` or `MODIFIED`.
    pub fn apply(&self, mut object: Value) {
        let kind = Kind::from_kind_field(object["kind"].as_str().expect("an object with a kind"));
        let namespace = object["metadata"]["namespace"]
            .as_str()
            .expect("an object with a namespace")
            .to_owned();
        let name = object["metadata"]["name"]
            .as_str()
            .expect("an object with a name")
            .to_owned();
        object["apiVersion"] = json!(kind.api_version());
        let mut state = self.state.lock().unwrap();
        let existed = state
            .objects
            .contains_key(&(kind, namespace.clone(), name.clone()));
        state.rv += 1;
        let rv = state.rv;
        object["metadata"]["resourceVersion"] = json!(rv.to_string());
        if object["metadata"]["uid"].is_null() {
            object["metadata"]["uid"] = json!(format!("uid-{name}-{rv}"));
        }
        state
            .objects
            .insert((kind, namespace.clone(), name), object.clone());
        state.log.push(LogEntry {
            rv,
            kind,
            namespace,
            event: if existed { "MODIFIED" } else { "ADDED" },
            object,
        });
        drop(state);
        self.changes.send_modify(|n| *n += 1);
    }

    /// Remove an object; a watch sees `DELETED`.
    pub fn delete(&self, kind: Kind, namespace: &str, name: &str) {
        let mut state = self.state.lock().unwrap();
        let Some(mut object) = state
            .objects
            .remove(&(kind, namespace.to_owned(), name.to_owned()))
        else {
            panic!("no {kind:?} {namespace}/{name} to delete");
        };
        state.rv += 1;
        let rv = state.rv;
        object["metadata"]["resourceVersion"] = json!(rv.to_string());
        state.log.push(LogEntry {
            rv,
            kind,
            namespace: namespace.to_owned(),
            event: "DELETED",
            object,
        });
        drop(state);
        self.changes.send_modify(|n| *n += 1);
    }

    /// The current version of an object.
    pub fn object(&self, kind: Kind, namespace: &str, name: &str) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .objects
            .get(&(kind, namespace.to_owned(), name.to_owned()))
            .cloned()
    }

    /// End every watch that is open now, as a dropped connection does. The agent must start a new one.
    pub fn drop_watches(&self) {
        self.state.lock().unwrap().epoch += 1;
        self.changes.send_modify(|n| *n += 1);
    }

    /// Fail the next list calls (of any kind) with these statuses, one each, then behave normally.
    pub fn fail_next_lists(&self, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .list_failures
            .extend(statuses.iter().map(|s| (None, *s)));
    }

    /// Fail the next list calls of one kind with these statuses, one each.
    pub fn fail_next_lists_of(&self, kind: Kind, statuses: &[u16]) {
        self.state
            .lock()
            .unwrap()
            .list_failures
            .extend(statuses.iter().map(|s| (Some(kind), *s)));
    }

    /// How many requests (of any method) went to a path that ends with `suffix`.
    pub fn count_calls_to(&self, suffix: &str) -> usize {
        self.calls().iter().filter(|c| c.path.ends_with(suffix)).count()
    }

    /// The path of a collection in a namespace, such as `/apis/apps/v1/namespaces/sit1/deployments`.
    pub fn collection_path(kind: Kind, namespace: &str) -> String {
        format!("{}/namespaces/{namespace}/{}", kind.prefix(), kind.resource())
    }
}

type FakeBody = UnsyncBoxBody<Bytes, Infallible>;

#[derive(Clone)]
struct FakeService {
    state: Arc<Mutex<State>>,
    changes: Arc<watch::Sender<u64>>,
}

type ServiceFuture = Pin<Box<dyn Future<Output = Result<Response<FakeBody>, Infallible>> + Send>>;

impl tower::Service<Request<Body>> for FakeService {
    type Response = Response<FakeBody>;
    type Error = Infallible;
    type Future = ServiceFuture;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let state = Arc::clone(&self.state);
        let changes = Arc::clone(&self.changes);
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let body = body.collect().await.unwrap().to_bytes();
            let response = handle(&state, &changes, &parts, &body);
            if state.lock().unwrap().stalled {
                std::future::pending::<()>().await;
            }
            Ok(response)
        })
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_owned())
}

fn handle(
    state_lock: &Arc<Mutex<State>>,
    changes: &Arc<watch::Sender<u64>>,
    parts: &http::request::Parts,
    body: &Bytes,
) -> Response<FakeBody> {
    let mut state = state_lock.lock().unwrap();
    let call = Call {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        content_type: parts
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        body: body.clone(),
    };
    state.calls.push(call.clone());
    if let Some(status) = state.deny_all {
        return status_error(status, "Forbidden", "denied by the fake RBAC");
    }
    if let Some((kind, namespace, name)) = cluster_path(&call.path) {
        return handle_cluster(
            &mut state,
            state_lock,
            changes,
            &call,
            kind,
            &namespace,
            name.as_deref(),
        );
    }
    handle_secret(&mut state, &call, body)
}

/// `(kind, namespace, name)` for the paths of the three kinds, or `None` for anything else.
fn cluster_path(path: &str) -> Option<(Kind, String, Option<String>)> {
    for kind in [Kind::Deployment, Kind::Pod, Kind::Job] {
        let Some(rest) = path
            .strip_prefix(kind.prefix())
            .and_then(|r| r.strip_prefix("/namespaces/"))
        else {
            continue;
        };
        let mut parts = rest.split('/');
        let (namespace, resource) = (parts.next()?, parts.next()?);
        if resource != kind.resource() {
            continue;
        }
        let name = parts.next().map(str::to_owned);
        if parts.next().is_some() {
            return None;
        }
        return Some((kind, namespace.to_owned(), name));
    }
    None
}

const FORBIDDEN: &str = "the agent may only get and update named Secrets, and read or restart workloads";

fn handle_cluster(
    state: &mut State,
    state_lock: &Arc<Mutex<State>>,
    changes: &Arc<watch::Sender<u64>>,
    call: &Call,
    kind: Kind,
    namespace: &str,
    name: Option<&str>,
) -> Response<FakeBody> {
    match (call.method.as_str(), name) {
        ("GET", None) if call.query_param("watch").as_deref() == Some("true") => {
            let from = call
                .query_param("resourceVersion")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(state.rv);
            watch_response(state_lock, changes, kind, namespace.to_owned(), from, state.epoch)
        }
        ("GET", None) => {
            if let Some(at) = state
                .list_failures
                .iter()
                .position(|(only, _)| only.is_none_or(|k| k == kind))
            {
                let (_, status) = state.list_failures.remove(at).unwrap();
                return status_error(status, "InternalError", "scripted list failure");
            }
            list_response(state, call, kind, namespace)
        }
        ("GET", Some(name)) => match state.objects.get(&(kind, namespace.to_owned(), name.to_owned())) {
            Some(object) => json(200, object),
            None => not_found(kind, name),
        },
        // Only Deployments are patched (a restart), and only with a merge patch.
        ("PATCH", Some(name)) if kind == Kind::Deployment => {
            if call.content_type.as_deref() != Some("application/merge-patch+json") {
                return status_error(415, "UnsupportedMediaType", "only merge patches");
            }
            let key = (kind, namespace.to_owned(), name.to_owned());
            let Some(mut object) = state.objects.get(&key).cloned() else {
                return not_found(kind, name);
            };
            let patch: Value = serde_json::from_slice(&call.body).unwrap();
            merge(&mut object, &patch);
            state.rv += 1;
            let rv = state.rv;
            object["metadata"]["resourceVersion"] = json!(rv.to_string());
            state.objects.insert(key, object.clone());
            state.log.push(LogEntry {
                rv,
                kind,
                namespace: namespace.to_owned(),
                event: "MODIFIED",
                object: object.clone(),
            });
            changes.send_modify(|n| *n += 1);
            json(200, &object)
        }
        _ => status_error(403, "Forbidden", FORBIDDEN),
    }
}

/// RFC 7386 JSON merge patch.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let target = target.as_object_mut().unwrap();
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

fn list_response(state: &State, call: &Call, kind: Kind, namespace: &str) -> Response<FakeBody> {
    let mut items: Vec<&Value> = state
        .objects
        .iter()
        .filter(|((k, ns, _), _)| *k == kind && ns == namespace)
        .map(|(_, v)| v)
        .collect();
    let offset = call
        .query_param("continue")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = call
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(usize::MAX);
    let total = items.len();
    items = items.into_iter().skip(offset).take(limit).collect();
    let next = offset.saturating_add(items.len());
    let mut metadata = json!({ "resourceVersion": state.rv.to_string() });
    if next < total {
        metadata["continue"] = json!(next.to_string());
    }
    json(
        200,
        &json!({
            "apiVersion": kind.api_version(),
            "kind": format!("{}List", kind.kind_name()),
            "metadata": metadata,
            "items": items,
        }),
    )
}

/// The streaming answer to a watch: every change after `from`, then every later one, one JSON object per line.
fn watch_response(
    state_lock: &Arc<Mutex<State>>,
    changes: &Arc<watch::Sender<u64>>,
    kind: Kind,
    namespace: String,
    from: u64,
    epoch: u64,
) -> Response<FakeBody> {
    let rx = changes.subscribe();
    let stream = futures::stream::unfold(
        (Arc::clone(state_lock), rx, from, kind, namespace),
        move |(state_lock, mut rx, mut cursor, kind, namespace)| async move {
            loop {
                let next = {
                    let state = state_lock.lock().unwrap();
                    if state.epoch != epoch {
                        return None;
                    }
                    state
                        .log
                        .iter()
                        .find(|e| e.rv > cursor && e.kind == kind && e.namespace == namespace)
                        .cloned()
                };
                if let Some(entry) = next {
                    cursor = entry.rv;
                    let mut line =
                        serde_json::to_vec(&json!({ "type": entry.event, "object": entry.object })).unwrap();
                    line.push(b'\n');
                    let frame: Result<Frame<Bytes>, Infallible> = Ok(Frame::data(Bytes::from(line)));
                    return Some((frame, (state_lock, rx, cursor, kind, namespace)));
                }
                if rx.changed().await.is_err() {
                    return None;
                }
            }
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(StreamBody::new(stream).boxed_unsync())
        .unwrap()
}

fn handle_secret(state: &mut State, call: &Call, body: &Bytes) -> Response<FakeBody> {
    let prefix = format!("/api/v1/namespaces/{}/secrets/", state.namespace);
    let Some(name) = call
        .path
        .strip_prefix(&prefix)
        .filter(|n| !n.is_empty() && !n.contains('/'))
    else {
        return status_error(403, "Forbidden", FORBIDDEN);
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
        _ => status_error(403, "Forbidden", FORBIDDEN),
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

fn json(status: u16, value: &impl serde::Serialize) -> Response<FakeBody> {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(value).unwrap())).boxed_unsync())
        .unwrap()
}

fn not_found(kind: Kind, name: &str) -> Response<FakeBody> {
    status_error(
        404,
        "NotFound",
        &format!("{} \"{name}\" not found", kind.resource()),
    )
}

fn status_error(code: u16, reason: &str, message: &str) -> Response<FakeBody> {
    json(
        code,
        &json!({
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
