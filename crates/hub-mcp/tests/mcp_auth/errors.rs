//! S21, rule 3: the errors this crate answers with are a closed set with fixed text, and they mean what the hub's own guard means.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use domain::{Role, User, UserId};
use http::request::Parts;
use hub_mcp::McpError;
use hub_mcp::auth::pat::TokenError;
use hub_mcp::auth::store::{LabelError, StoreError};
use hub_platform::{GuardedRouter, SessionAuth};
use ports::AuthError;
use tower::ServiceExt;

const ALL: [McpError; 10] = [
    McpError::Unauthenticated,
    McpError::Forbidden,
    McpError::AuthUnavailable,
    McpError::NotFound,
    McpError::InvalidRequest,
    McpError::PayloadTooLarge,
    McpError::UnsupportedMediaType,
    McpError::TokenLimit,
    McpError::Unavailable,
    McpError::Internal,
];

#[test]
fn errors_have_stable_statuses_and_codes() {
    let table = [
        (McpError::Unauthenticated, 401, "unauthorized"),
        (McpError::Forbidden, 403, "forbidden"),
        (McpError::AuthUnavailable, 503, "unavailable"),
        (McpError::NotFound, 404, "not_found"),
        (McpError::InvalidRequest, 400, "bad_request"),
        (McpError::PayloadTooLarge, 413, "payload_too_large"),
        (McpError::UnsupportedMediaType, 415, "unsupported_media_type"),
        (McpError::TokenLimit, 409, "conflict"),
        (McpError::Unavailable, 503, "unavailable"),
        (McpError::Internal, 500, "internal"),
    ];
    assert_eq!(table.len(), ALL.len());
    for (error, status, code) in table {
        assert_eq!(error.status().as_u16(), status, "{error:?}");
        assert_eq!(error.code(), code, "{error:?}");
    }
}

#[test]
fn error_messages_are_fixed_text_not_input() {
    // Display is a literal for every variant: it holds no field, so it cannot hold input.
    let mut seen = std::collections::BTreeSet::new();
    for error in ALL {
        let text = error.to_string();
        assert!(
            !text.is_empty() && text.len() < 64 && !text.contains(['{', '}', '"']),
            "{text}"
        );
        seen.insert(text);
    }
    assert_eq!(seen.len(), ALL.len(), "every variant says something different");
    // The errors of the layers below are fixed text as well.
    for text in [
        TokenError::LimitReached.to_string(),
        TokenError::NotFound.to_string(),
        TokenError::Unavailable.to_string(),
        TokenError::Internal.to_string(),
        StoreError::LimitReached.to_string(),
        StoreError::Unavailable.to_string(),
        LabelError::Empty.to_string(),
        LabelError::TooLong.to_string(),
        LabelError::Control.to_string(),
        hub_mcp::auth::token::KeyError::Encoding.to_string(),
        hub_mcp::auth::token::RandomError.to_string(),
    ] {
        assert!(
            !text.is_empty() && text.len() < 80 && !text.contains(['{', '}', '"']),
            "{text}"
        );
    }
}

#[test]
fn token_errors_map_onto_mcp_errors() {
    assert_eq!(McpError::from(TokenError::LimitReached), McpError::TokenLimit);
    assert_eq!(McpError::from(TokenError::NotFound), McpError::NotFound);
    assert_eq!(McpError::from(TokenError::Unavailable), McpError::Unavailable);
    assert_eq!(McpError::from(TokenError::Internal), McpError::Internal);
}

struct Fails(AuthError);

#[async_trait]
impl SessionAuth for Fails {
    async fn authenticate(&self, _parts: &Parts) -> Result<User, AuthError> {
        Err(self.0)
    }

    async fn still_valid(&self, _user: &UserId) -> bool {
        false
    }
}

#[tokio::test]
async fn auth_errors_map_to_the_statuses_the_hub_guard_gives() {
    // The gate answers a failed authentication as the hub's own route guard does, so one user gets one verdict on both doors.
    for error in [
        AuthError::InvalidToken,
        AuthError::Expired,
        AuthError::WrongAudience,
        AuthError::UnknownUser,
        AuthError::Pending,
        AuthError::Disabled,
        AuthError::InsufficientRole,
        AuthError::Unavailable,
    ] {
        let (router, _) = GuardedRouter::new(Arc::new(Fails(error)), None)
            .route(&http::Method::GET, "/x", Role::Viewer, || async { "ok" })
            .into_parts()
            .unwrap();
        let response = router
            .oneshot(Request::builder().uri("/x").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), McpError::from(error).status(), "{error:?}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json",
            "{error:?}"
        );
    }
    assert_eq!(McpError::Unauthenticated.status(), StatusCode::UNAUTHORIZED);
}
