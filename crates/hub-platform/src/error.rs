//! `application/problem+json` for every error the hub answers (rule 3, S21).
//!
//! A problem names a status and a stable code from a closed set. It never carries text from a library, a path, SQL or a
//! stack trace: the cause goes to the log, with the request id, and the client gets the code and the request id to quote.

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::{HeaderValue, StatusCode, header};

/// The media type of every error body.
pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

/// The code of the problem a handler returned, kept in the response's extensions so that the edge can find it without
/// parsing the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemCode(pub &'static str);

/// What a handler can answer with when it fails. One variant per stable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum ApiError {
    #[error("bad request")]
    BadRequest,
    #[error("authentication is required")]
    Unauthorized,
    #[error("not allowed")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("the request took too long")]
    RequestTimeout,
    #[error("conflict")]
    Conflict,
    #[error("the request body is too large")]
    PayloadTooLarge,
    #[error("unsupported media type")]
    UnsupportedMediaType,
    #[error("the request is not acceptable")]
    Unprocessable,
    #[error("too many requests")]
    TooManyRequests,
    #[error("internal error")]
    Internal,
    #[error("the service is unavailable")]
    Unavailable,
    #[error("the hub is shutting down")]
    ShuttingDown,
    #[error("the upstream took too long")]
    GatewayTimeout,
}

impl ApiError {
    #[must_use]
    pub fn status(self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::Conflict => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable | Self::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
            Self::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    /// The stable machine-readable code. Clients may match on it; it never changes for a variant.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::RequestTimeout => "request_timeout",
            Self::Conflict => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::Unprocessable => "unprocessable",
            Self::TooManyRequests => "too_many_requests",
            Self::Internal => "internal",
            Self::Unavailable => "unavailable",
            Self::ShuttingDown => "shutting_down",
            Self::GatewayTimeout => "gateway_timeout",
        }
    }

    /// A short fixed title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::BadRequest => "Bad request",
            Self::Unauthorized => "Authentication required",
            Self::Forbidden => "Not allowed",
            Self::NotFound => "Not found",
            Self::MethodNotAllowed => "Method not allowed",
            Self::RequestTimeout => "Request timeout",
            Self::Conflict => "Conflict",
            Self::PayloadTooLarge => "Payload too large",
            Self::UnsupportedMediaType => "Unsupported media type",
            Self::Unprocessable => "Unprocessable request",
            Self::TooManyRequests => "Too many requests",
            Self::Internal => "Internal error",
            Self::Unavailable => "Service unavailable",
            Self::ShuttingDown => "Shutting down",
            Self::GatewayTimeout => "Gateway timeout",
        }
    }

    /// The problem for a status that some other layer chose (a router's 404 or 405, an extractor's 413). Library text is
    /// dropped: only the status decides the code. A status that has no variant becomes the generic one of its class.
    #[must_use]
    pub fn from_status(status: StatusCode) -> Self {
        match status.as_u16() {
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            405 => Self::MethodNotAllowed,
            408 => Self::RequestTimeout,
            409 => Self::Conflict,
            411 | 413 => Self::PayloadTooLarge,
            415 => Self::UnsupportedMediaType,
            422 => Self::Unprocessable,
            429 => Self::TooManyRequests,
            503 => Self::Unavailable,
            504 => Self::GatewayTimeout,
            s if (400..500).contains(&s) => Self::BadRequest,
            _ => Self::Internal,
        }
    }

    /// The body of this problem. `request_id` is what a person quotes to find the request in the logs.
    #[must_use]
    pub fn body(self, request_id: Option<&str>) -> Bytes {
        problem_body(self.status(), self.code(), self.title(), request_id)
    }
}

/// Serialise a problem. The `type` is a URN built from the code, so it is stable and resolves nowhere.
#[must_use]
pub fn problem_body(status: StatusCode, code: &str, title: &str, request_id: Option<&str>) -> Bytes {
    let mut object = serde_json::Map::new();
    object.insert(
        "type".to_owned(),
        serde_json::Value::String(format!("urn:lanekeeper:problem:{code}")),
    );
    object.insert("title".to_owned(), serde_json::Value::String(title.to_owned()));
    object.insert("status".to_owned(), serde_json::Value::from(status.as_u16()));
    object.insert("code".to_owned(), serde_json::Value::String(code.to_owned()));
    if let Some(id) = request_id {
        object.insert("request_id".to_owned(), serde_json::Value::String(id.to_owned()));
    }
    // A map of strings and one integer always serialises; the fallback is only there to keep this function total.
    serde_json::to_vec(&serde_json::Value::Object(object)).map_or_else(
        |_| {
            Bytes::from_static(
                br#"{"type":"urn:lanekeeper:problem:internal","title":"Internal error","status":500,"code":"internal"}"#,
            )
        },
        Bytes::from,
    )
}

/// A response that is a problem with this code, with no `request_id` (the edge adds it).
#[must_use]
pub fn problem_response(error: ApiError) -> Response {
    let mut response = Response::new(Body::from(error.body(None)));
    *response.status_mut() = error.status();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PROBLEM_CONTENT_TYPE),
    );
    response.extensions_mut().insert(ProblemCode(error.code()));
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        problem_response(self)
    }
}
