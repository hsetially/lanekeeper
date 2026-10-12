//! Request and response bodies for the JSON routes: strict, bounded, and never echoed back.

use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use domain::RequestId;
use http_body_util::LengthLimitError;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{bad_request, unavailable};
use crate::problem::problem;

/// The largest request body any identity route reads.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// A client that sends its body slower than this is cut off.
const BODY_TIMEOUT: Duration = Duration::from_secs(10);

fn is_json(req: &Request) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"))
}

/// Read the whole body (at most [`MAX_BODY_BYTES`]) as JSON of type `T`. `T` should refuse unknown fields. The error is the
/// response to send: `415`, `413` or `400`, none of which repeats anything the client sent.
pub async fn read_json<T: DeserializeOwned>(req: Request, id: Option<&RequestId>) -> Result<T, Response> {
    if !is_json(&req) {
        return Err(problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
            "Send application/json",
            id,
        ));
    }
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let too_large = || {
        problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload-too-large",
            "The request body is too large",
            id,
        )
    };
    if declared.is_some_and(|n| n > MAX_BODY_BYTES) {
        return Err(too_large());
    }
    let bytes = match tokio::time::timeout(
        BODY_TIMEOUT,
        axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            let over =
                std::error::Error::source(&e).is_some_and(|s| s.downcast_ref::<LengthLimitError>().is_some());
            return Err(if over { too_large() } else { bad_request(id) });
        }
        Err(_) => return Err(bad_request(id)),
    };
    serde_json::from_slice(&bytes).map_err(|_| bad_request(id))
}

/// A JSON answer that browsers and proxies must not keep: it holds personal data.
pub fn json_response<T: Serialize>(status: StatusCode, value: &T, id: Option<&RequestId>) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => unavailable(id),
    }
}

/// The query parameters of a request, each of `known` at most once. Unknown parameters are ignored. `None` for a repeated
/// known parameter or a query over 2 KiB.
pub fn query_params(raw: Option<&str>, known: &[&str]) -> Option<Vec<Option<String>>> {
    let mut out = vec![None; known.len()];
    let raw = raw.unwrap_or("");
    if raw.len() > 2048 {
        return None;
    }
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        if let Some(i) = known.iter().position(|name| *name == k) {
            if out[i].is_some() {
                return None;
            }
            out[i] = Some(v.into_owned());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_params_take_each_known_name_once() {
        let known = ["status", "limit"];
        assert_eq!(
            query_params(Some("limit=5&other=1&status=active"), &known),
            Some(vec![Some("active".to_owned()), Some("5".to_owned())])
        );
        assert_eq!(query_params(None, &known), Some(vec![None, None]));
        assert_eq!(query_params(Some("limit=1&limit=2"), &known), None);
        assert_eq!(query_params(Some(&"a=b&".repeat(600)), &known), None);
        // An unknown name may repeat: it is ignored.
        assert!(query_params(Some("x=1&x=2"), &known).is_some());
    }
}
