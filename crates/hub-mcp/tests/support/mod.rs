//! A small MCP client that drives the T0 stub's router in process: no sockets, so the tests run on a paused clock.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, Response, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

/// The `Host` header the stub's default configuration accepts.
pub const HOST: &str = "127.0.0.1:8799";

/// An unsigned JWT-shaped token with these claims. The stub reads claims and verifies nothing, so this is all T0 needs.
pub fn jwt(claims: &Value) -> String {
    let part = |value: &Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap());
    format!(
        "{}.{}.{}",
        part(&json!({"alg": "RS256", "typ": "JWT"})),
        part(claims),
        URL_SAFE_NO_PAD.encode(b"sig")
    )
}

/// The claims a well-formed Entra v2 access token for the hub carries (names and shapes only; every value is invented).
pub fn good_claims() -> Value {
    json!({
        "aud": "api://hub-api",
        "scp": "mcp.access",
        "iss": "https://login.microsoftonline.com/00000000-0000-0000-0000-000000000000/v2.0",
        "tid": "00000000-0000-0000-0000-000000000000",
        "ver": "2.0",
        "oid": "11111111-2222-3333-4444-555555555555",
        "upn": "someone@example.invalid",
        "name": "Some One"
    })
}

pub fn good_token() -> String {
    jwt(&good_claims())
}

/// A second token, for a second caller.
pub fn other_token() -> String {
    let mut claims = good_claims();
    claims["oid"] = json!("99999999-8888-7777-6666-555555555555");
    claims["upn"] = json!("someone.else@example.invalid");
    jwt(&claims)
}

/// A reader over a Server-Sent Events body that yields one JSON-RPC message at a time, as the bytes arrive.
pub struct Sse {
    body: Body,
    buffer: String,
    ended: bool,
}

impl Sse {
    pub fn new(body: Body) -> Self {
        Self {
            body,
            buffer: String::new(),
            ended: false,
        }
    }

    /// The next JSON message. Comments (keep-alive pings), priming events and empty events are skipped. `None` at the end.
    pub async fn next(&mut self) -> Option<Value> {
        loop {
            while let Some(end) = self.buffer.find("\n\n") {
                let event: String = self.buffer.drain(..end + 2).collect();
                let data: Vec<&str> = event
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(|data| data.strip_prefix(' ').unwrap_or(data))
                    .collect();
                let joined = data.join("\n");
                if joined.trim().is_empty() {
                    continue;
                }
                if let Ok(message) = serde_json::from_str::<Value>(&joined) {
                    return Some(message);
                }
            }
            if self.ended {
                return None;
            }
            match self.body.frame().await {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.buffer
                            .push_str(&String::from_utf8_lossy(&data).replace("\r\n", "\n"));
                    }
                }
                _ => self.ended = true,
            }
        }
    }

    /// Everything up to the end of the stream.
    pub async fn all(mut self) -> Vec<Value> {
        let mut messages = Vec::new();
        while let Some(message) = self.next().await {
            messages.push(message);
        }
        messages
    }

    /// Messages until the response to request `id`, which is returned last. `None` if the stream ends first.
    pub async fn until_response(&mut self, id: i64) -> Option<(Vec<Value>, Value)> {
        let mut before = Vec::new();
        while let Some(message) = self.next().await {
            if message.get("id") == Some(&json!(id))
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                return Some((before, message));
            }
            before.push(message);
        }
        None
    }
}

/// The text of the first content block of a `tools/call` response.
pub fn result_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

pub struct Client {
    pub app: Router,
    pub bearer: Option<String>,
    pub session: Option<String>,
    next_id: i64,
}

impl Client {
    pub fn new(app: Router, bearer: Option<String>) -> Self {
        Self {
            app,
            bearer,
            session: None,
            next_id: 0,
        }
    }

    pub fn with_session(&self, session: Option<String>) -> Self {
        Self {
            app: self.app.clone(),
            bearer: self.bearer.clone(),
            session,
            next_id: 1000,
        }
    }

    /// A request with the headers every MCP call carries.
    pub fn request(&self, method: Method) -> axum::http::request::Builder {
        let mut builder = Request::builder()
            .method(method)
            .uri("/mcp")
            .header(header::HOST, HOST)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(bearer) = &self.bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
        }
        if let Some(session) = &self.session {
            builder = builder.header("mcp-session-id", session);
        }
        builder
    }

    pub async fn send(&self, request: Request<Body>) -> Response<Body> {
        self.app.clone().oneshot(request).await.unwrap()
    }

    pub async fn get(&self, path: &str) -> Response<Body> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap();
        self.send(request).await
    }

    pub async fn post(&self, body: &Value) -> Response<Body> {
        self.send(
            self.request(Method::POST)
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
    }

    pub fn id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    /// `initialize` with these client capabilities. On success the session id is stored and the result returned.
    pub async fn initialize(&mut self, capabilities: &Value) -> Result<Value, StatusCode> {
        self.initialize_as("2025-06-18", capabilities).await
    }

    pub async fn initialize_as(
        &mut self,
        protocol_version: &str,
        capabilities: &Value,
    ) -> Result<Value, StatusCode> {
        let id = self.id();
        let response = self
            .post(&json!({
                "jsonrpc": "2.0", "id": id, "method": "initialize",
                "params": {
                    "protocolVersion": protocol_version,
                    "capabilities": capabilities,
                    "clientInfo": {"name": "t0-test-client", "version": "0.0.0"}
                }
            }))
            .await;
        let status = response.status();
        if !status.is_success() {
            return Err(status);
        }
        self.session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut sse = Sse::new(response.into_body());
        let (_, message) = sse.until_response(id).await.ok_or(StatusCode::BAD_GATEWAY)?;
        let status = self.notify("notifications/initialized", &json!({})).await;
        if status != StatusCode::ACCEPTED {
            return Err(status);
        }
        Ok(message["result"].clone())
    }

    pub async fn notify(&self, method: &str, params: &Value) -> StatusCode {
        self.post(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
            .status()
    }

    /// Start a `tools/call` and return its id and the stream that carries the response (and any question the server asks).
    pub async fn call_tool(&mut self, name: &str, arguments: &Value) -> (i64, Result<Sse, StatusCode>) {
        let id = self.id();
        let response = self
            .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}))
            .await;
        let status = response.status();
        if status.is_success() {
            (id, Ok(Sse::new(response.into_body())))
        } else {
            (id, Err(status))
        }
    }

    /// A `tools/call` that asks nothing: the response to it.
    pub async fn call_tool_to_end(&mut self, name: &str, arguments: &Value) -> Value {
        let (id, sse) = self.call_tool(name, arguments).await;
        let mut sse = sse.unwrap();
        sse.until_response(id).await.unwrap().1
    }

    /// Answer a question the server asked (a JSON-RPC response to its request `id`).
    pub async fn answer(&self, question_id: &Value, result: &Value) -> StatusCode {
        self.post(&json!({"jsonrpc": "2.0", "id": question_id, "result": result}))
            .await
            .status()
    }

    pub async fn delete_session(&self) -> StatusCode {
        self.send(self.request(Method::DELETE).body(Body::empty()).unwrap())
            .await
            .status()
    }

    pub async fn open_event_stream(&self) -> StatusCode {
        self.open_stream().await.err().unwrap_or(StatusCode::OK)
    }

    /// The standalone server-to-client stream (`GET /mcp`), where the server puts what belongs to no request.
    pub async fn open_stream(&self) -> Result<Sse, StatusCode> {
        let response = self
            .send(self.request(Method::GET).body(Body::empty()).unwrap())
            .await;
        if response.status().is_success() {
            Ok(Sse::new(response.into_body()))
        } else {
            Err(response.status())
        }
    }
}

pub fn header_text(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

pub async fn json_body(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// The elicitation request in a list of server messages, if any.
pub fn find_question(messages: &[Value]) -> Option<&Value> {
    messages.iter().find(|m| m["method"] == "elicitation/create")
}

/// The capabilities of a client that can show a confirmation form.
pub fn elicitation_capabilities() -> Value {
    json!({"elicitation": {}})
}
