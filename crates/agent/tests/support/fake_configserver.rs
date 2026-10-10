//! A fake config-server (T12, Q37): it takes the unauthenticated `POST /update-resources` and serves files for
//! `GET /{application}/{tenant},default/master[/{channel}]/{file}`, records every request, and can be made to fail the way
//! a real one does (an error status, a hang, a dropped connection, a huge body).
//!
//! The form decoder here is written independently of the agent's encoder on purpose: a test that decoded with the
//! encoder's own inverse would pass whatever the encoder did.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use super::raw_server::{RawServer, RecordedRequest, Reply};

/// One `POST /update-resources` as the config-server received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub method: String,
    pub target: String,
    pub backend: Option<String>,
    pub content_type: Option<String>,
    /// The `path` fields of the form, in order, percent-decoded.
    pub paths: Vec<String>,
    /// The fields of the form that are not `path`.
    pub other_fields: Vec<String>,
}

#[derive(Default)]
struct Script {
    /// What `POST /update-resources` answers. Taken one at a time; the last one repeats.
    notify: VecDeque<Reply>,
    /// What a `GET` answers, by the exact request target.
    files: BTreeMap<String, Reply>,
}

pub struct FakeConfigServer {
    server: Option<RawServer>,
    url: String,
    script: Arc<Mutex<Script>>,
}

impl FakeConfigServer {
    /// Answers `200` to a notify and `404` to any file it was not told about. It lives in memory: connect to it with
    /// [`FakeConfigServer::dialer`]. Nothing touches a socket, so a test can run under a paused clock.
    pub fn start() -> Self {
        let script = Arc::new(Mutex::new(Script::default()));
        let server = RawServer::in_memory({
            let script = Arc::clone(&script);
            move |request| answer(&script, request)
        });
        Self {
            url: server.url(),
            server: Some(server),
            script,
        }
    }

    /// As [`FakeConfigServer::start`], on a loopback port. For a test of the real TCP path; it must not run under a paused
    /// clock.
    pub async fn start_tcp() -> Self {
        let script = Arc::new(Mutex::new(Script::default()));
        let server = RawServer::start({
            let script = Arc::clone(&script);
            move |request| answer(&script, request)
        })
        .await;
        Self {
            url: server.url(),
            server: Some(server),
            script,
        }
    }

    /// Nothing listens: a connection is refused.
    pub fn absent() -> Self {
        let server = RawServer::in_memory_refusing();
        Self {
            url: server.url(),
            server: Some(server),
            script: Arc::default(),
        }
    }

    /// How to connect to an in-memory server (not for [`FakeConfigServer::start_tcp`]).
    pub fn dialer(&self) -> std::sync::Arc<dyn agent::transport::dial::Dialer> {
        self.server.as_ref().expect("a server").dialer()
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The `host:port` the requests must be addressed to.
    pub fn authority(&self) -> &str {
        self.url.trim_start_matches("http://")
    }

    /// A client for this server: its URL, the shared deny list and the clock, connecting through the pipe.
    pub fn client(
        &self,
        deny: agent::deny::DenyList,
        clock: std::sync::Arc<dyn agent::clock::Clock>,
    ) -> agent::configserver::ConfigServerClient {
        agent::configserver::ConfigServerClient::new(
            agent::config::BaseUrl::parse(&self.url, "http").unwrap(),
            deny,
            clock,
        )
        .with_dialer(self.dialer())
    }

    /// What the next notifies answer, one reply each; the last repeats.
    pub fn script_notify(&self, replies: impl IntoIterator<Item = Reply>) {
        self.script.lock().unwrap().notify = replies.into_iter().collect();
    }

    /// Answer a notify with this status from now on.
    pub fn notify_status(&self, status: u16) {
        self.script_notify([Reply::status(status, "")]);
    }

    /// Serve `body` for `GET target` (the request target exactly as the agent must write it).
    pub fn serve(&self, target: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.script
            .lock()
            .unwrap()
            .files
            .insert(target.to_owned(), Reply::status(status, body));
    }

    /// Serve any reply (a hang, a drop) for `GET target`.
    pub fn serve_reply(&self, target: &str, reply: Reply) {
        self.script.lock().unwrap().files.insert(target.to_owned(), reply);
    }

    /// The `(host, port)` of every connection the agent made.
    pub fn dialed(&self) -> Vec<(String, u16)> {
        self.server.as_ref().map(RawServer::dialed).unwrap_or_default()
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.server.as_ref().map(RawServer::requests).unwrap_or_default()
    }

    /// The notifies, decoded.
    pub fn notifications(&self) -> Vec<Notification> {
        self.requests()
            .iter()
            .filter(|r| r.method == "POST")
            .map(notification)
            .collect()
    }

    /// The `GET`s, as the request targets that were written.
    pub fn fetches(&self) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "GET")
            .collect()
    }
}

fn answer(script: &Mutex<Script>, request: &RecordedRequest) -> Reply {
    let mut script = script.lock().unwrap();
    if request.method == "POST" && request.target == "/update-resources" {
        return match script.notify.len() {
            0 => Reply::ok(""),
            1 => script.notify[0].clone(),
            _ => script.notify.pop_front().unwrap(),
        };
    }
    script
        .files
        .get(&request.target)
        .cloned()
        .unwrap_or_else(|| Reply::status(404, "not found"))
}

fn notification(request: &RecordedRequest) -> Notification {
    let body = String::from_utf8_lossy(&request.body).into_owned();
    let mut paths = Vec::new();
    let mut other_fields = Vec::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if form_decode(name) == "path" {
            paths.push(form_decode(value));
        } else {
            other_fields.push(form_decode(name));
        }
    }
    Notification {
        method: request.method.clone(),
        target: request.target.clone(),
        backend: request.header("backend").map(str::to_owned),
        content_type: request.header("content-type").map(str::to_owned),
        paths,
        other_fields,
    }
}

/// `application/x-www-form-urlencoded` decoding: `+` is a space, `%XX` is a byte.
pub fn form_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() && hex(bytes[i + 1]).is_some() && hex(bytes[i + 2]).is_some() => {
                out.push(hex(bytes[i + 1]).unwrap() * 16 + hex(bytes[i + 2]).unwrap());
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("a form value that is not UTF-8")
}

/// Percent-decoding of one URL path segment (no `+` rule).
pub fn segment_decode(text: &str) -> String {
    form_decode(&text.replace('+', "%2B"))
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
