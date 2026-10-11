//! The layers every web request passes through, outermost first (T1):
//!
//! 1. [`request_context`]: a request id, a tracing span, and the RED metrics;
//! 2. the security headers (`headers`), so they cover everything below, errors included;
//! 3. [`normalize_errors`]: every 4xx and 5xx becomes `application/problem+json` with a stable code and the request id,
//!    whoever produced it (a handler, the router, an extractor, a limit) and whatever text they put in it (S21);
//! 4. [`drain_gate`]: new requests are refused once shutdown has begun;
//! 5. the limits (`limits`), then the router.
//!
//! The span carries the request id, the method and the route template, never the URI, the query, a header or a body (S10).

use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::{HeaderName, HeaderValue, StatusCode, header};
use http_body_util::{BodyExt, Limited};
use hub_platform::ApiError;
use hub_platform::error::{PROBLEM_CONTENT_TYPE, problem_body};
use std::sync::Arc;
use tracing::Instrument;

use crate::metrics::Metrics;
use crate::random::random_hex;
use crate::shutdown::{Drain, Phase};

/// The id of this request, in the extensions and in the `x-request-id` response header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// The most a problem body that a handler produced may weigh for the edge to read it and add the request id.
const PROBLEM_READ_CAP: usize = 16 * 1024;

/// What the first layer needs.
#[derive(Debug, Clone)]
pub struct ContextState {
    pub metrics: Arc<Metrics>,
}

/// The route label of a request: the matched template, or `unmatched`.
fn route_of(request: &Request) -> String {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |m| m.as_str().to_owned())
}

/// Request id, span and metrics.
pub async fn request_context(
    State(state): State<ContextState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(id) = random_hex(8) else {
        return ApiError::Internal.into_response();
    };
    request.extensions_mut().insert(RequestId(id.clone()));
    let method = request.method().clone();
    let started = Instant::now();
    let in_flight = state.metrics.start();
    // The template is known only after routing, so it is read from the response's extensions below; the span uses the
    // method and id, which are known now.
    let span = tracing::info_span!("request", request_id = %id, method = %method);
    let mut response = next.run(request).instrument(span).await;
    drop(in_flight);
    let route = response
        .extensions()
        .get::<RouteTemplate>()
        .map_or_else(|| "unmatched".to_owned(), |t| t.0.clone());
    state.metrics.observe(
        &route,
        response.status().as_u16(),
        started.elapsed().as_secs_f64(),
    );
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

/// The route template of a response, copied from the request by [`record_route`] so the outer layers can label metrics.
#[derive(Debug, Clone)]
pub struct RouteTemplate(pub String);

/// An inner layer (it runs after routing, so `MatchedPath` is set) that tells the outer layers which route it was.
pub async fn record_route(request: Request, next: Next) -> Response {
    let route = route_of(&request);
    let mut response = next.run(request).await;
    response.extensions_mut().insert(RouteTemplate(route));
    response
}

/// Turn every error response into a problem with the request id (rule 3, S21).
pub async fn normalize_errors(request: Request, next: Next) -> Response {
    let id = request.extensions().get::<RequestId>().map(|r| r.0.clone());
    let response = next.run(request).await;
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let is_problem = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with(PROBLEM_CONTENT_TYPE));

    // A handler's own problem keeps its fields (a conflict carries the current hash); the edge only adds the request id.
    // Anything else is replaced: the status alone decides what the client reads.
    let rendered: Bytes = if is_problem {
        match read_problem(body).await {
            Some(mut object) => {
                if let Some(id) = &id {
                    object
                        .entry("request_id")
                        .or_insert_with(|| serde_json::Value::String(id.clone()));
                }
                serde_json::to_vec(&serde_json::Value::Object(object))
                    .map_or_else(|_| generic(status, id.as_deref()), Bytes::from)
            }
            None => generic(status, id.as_deref()),
        }
    } else {
        generic(status, id.as_deref())
    };
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROBLEM_CONTENT_TYPE),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::CONTENT_ENCODING);
    parts.headers.remove(header::ETAG);
    parts.headers.remove(header::CONTENT_DISPOSITION);
    Response::from_parts(parts, Body::from(rendered))
}

fn generic(status: StatusCode, request_id: Option<&str>) -> Bytes {
    let error = ApiError::from_status(status);
    problem_body(status, error.code(), error.title(), request_id)
}

async fn read_problem(body: Body) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = Limited::new(body, PROBLEM_READ_CAP)
        .collect()
        .await
        .ok()?
        .to_bytes();
    match serde_json::from_slice::<serde_json::Value>(&bytes).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Refuse new requests once shutdown has begun. `Connection: close` tells the client to find another instance.
pub async fn drain_gate(State(drain): State<Drain>, request: Request, next: Next) -> Response {
    if drain.phase() == Phase::Stopping {
        let mut response = ApiError::ShuttingDown.into_response();
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return response;
    }
    next.run(request).await
}
