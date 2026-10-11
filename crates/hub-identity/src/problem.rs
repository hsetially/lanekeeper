//! `application/problem+json` answers for the layers and routes of this crate (RFC 9457).
//!
//! Every body is a fixed type, a fixed title and the status, plus the request id to quote for the logs. Nothing in it comes
//! from the request or from an error: no SQL, no path, no header value, no token (S10, S21). Which check refused a request is
//! logged by variant, never told to the caller.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use domain::RequestId;

/// The media type of every error body.
pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// A problem response. `kind` is the last part of `urn:lanekeeper:problem:<kind>`.
pub fn problem(
    status: StatusCode,
    kind: &'static str,
    title: &'static str,
    request_id: Option<&RequestId>,
) -> Response {
    let mut body = serde_json::json!({
        "type": format!("urn:lanekeeper:problem:{kind}"),
        "title": title,
        "status": status.as_u16(),
    });
    if let Some(id) = request_id {
        body["request_id"] = serde_json::Value::String(id.to_string());
    }
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROBLEM_CONTENT_TYPE),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_problem_has_a_fixed_shape_and_the_request_id() {
        let id = RequestId::parse("req-1").unwrap();
        let r = problem(StatusCode::FORBIDDEN, "csrf", "Request not allowed", Some(&id));
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            r.headers().get(header::CONTENT_TYPE).unwrap(),
            PROBLEM_CONTENT_TYPE
        );
        let bytes = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "urn:lanekeeper:problem:csrf",
                "title": "Request not allowed",
                "status": 403,
                "request_id": "req-1",
            })
        );
        let none = problem(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Try again", None);
        let bytes = axum::body::to_bytes(none.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v.get("request_id").is_none());
    }
}
