//! The REST routes through which a signed-in user makes, lists and revokes their personal MCP tokens (D52, S7, S9).
//!
//! | Route | Does |
//! |---|---|
//! | `GET /api/v1/me/mcp-tokens` | the caller's own tokens, newest first, with label and times; **never** a token value |
//! | `POST /api/v1/me/mcp-tokens` | makes a token, shown once in this response; audited |
//! | `DELETE /api/v1/me/mcp-tokens/{id}` | revokes the caller's token; revoking twice is 204 and writes nothing; audited |
//!
//! **Registered in both modes**, so the route table matches the `OpenAPI` spec whichever mode a deployment runs, and **answering 404
//! unless personal-token mode is on** (the operations say so). The routes ride the hub's *session* router (a signed-in user,
//! Viewer), not the bearer one: a personal token cannot make or revoke personal tokens.
//!
//! **S3.** These routes change state, so the hub's CSRF layer must cover them (a double-submit token and an exact `Origin`). That
//! layer is composition, not this crate's; as defence in depth the two state-changing routes here also refuse a request whose
//! `Origin` is missing or is not the public origin, before they read the body.
//!
//! **`Idempotency-Key`** is accepted and ignored here, as 03a's routes do (B17): a revoke answers 204 on a repeat, and the hub-level
//! idempotency layer (D68) wraps these routes later. That layer must not store the response of `POST`, which holds the token.
//!
//! **Responses** carry `Cache-Control: no-store`. Errors are `application/problem+json` with a fixed code and no input (S21).

use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::{RequestId, Role};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use http_body_util::BodyExt;
use hub_platform::{AuthenticatedUser, GuardedRouter};
use serde::{Deserialize, Serialize};

use super::pat::PersonalTokens;
use super::store::{Label, TokenId, TokenRecord};
use crate::config::PublicOrigin;
use crate::error::McpError;

/// `GET` and `POST`.
pub const COLLECTION_PATH: &str = "/api/v1/me/mcp-tokens";
/// `DELETE`.
pub const ITEM_PATH: &str = "/api/v1/me/mcp-tokens/{id}";

/// The page size when the request does not say, and the most it may ask for (the `OpenAPI` `Limit`).
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
/// A body larger than this is not a label.
const MAX_BODY_BYTES: usize = 1024;
/// The longest cursor (the `OpenAPI` `Cursor` pattern).
const MAX_CURSOR_LEN: usize = 512;

/// What the routes need. `tokens` is `None` in OAuth mode.
#[derive(Debug, Clone)]
pub struct TokenRoutes {
    tokens: Option<Arc<PersonalTokens>>,
    origin: PublicOrigin,
}

impl TokenRoutes {
    #[must_use]
    pub fn new(tokens: Option<Arc<PersonalTokens>>, origin: PublicOrigin) -> Self {
        Self { tokens, origin }
    }
}

/// Register the three routes at Viewer.
#[must_use]
pub fn mount(router: GuardedRouter, state: &TokenRoutes) -> GuardedRouter {
    let list_state = state.clone();
    let create_state = state.clone();
    let revoke_state = state.clone();
    router
        .route(
            &Method::GET,
            COLLECTION_PATH,
            Role::Viewer,
            move |Extension(user): Extension<AuthenticatedUser>, uri: Uri| {
                let state = list_state.clone();
                async move { answer(list(&state, &user, &uri).await) }
            },
        )
        .route(
            &Method::POST,
            COLLECTION_PATH,
            Role::Viewer,
            move |Extension(user): Extension<AuthenticatedUser>, request: Request| {
                let state = create_state.clone();
                async move { answer(create(&state, &user, request).await) }
            },
        )
        .route(
            &Method::DELETE,
            ITEM_PATH,
            Role::Viewer,
            move |Extension(user): Extension<AuthenticatedUser>,
                  Path(id): Path<String>,
                  headers: HeaderMap| {
                let state = revoke_state.clone();
                async move { answer(revoke(&state, &user, &id, &headers).await) }
            },
        )
}

fn answer(result: Result<Response, McpError>) -> Response {
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.api_error().into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn json<T: Serialize>(status: StatusCode, body: &T) -> Result<Response, McpError> {
    let bytes = serde_json::to_vec(body).map_err(|_| McpError::Internal)?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}

/// The tokens service, or 404 in OAuth mode.
fn tokens(state: &TokenRoutes) -> Result<&Arc<PersonalTokens>, McpError> {
    state.tokens.as_ref().ok_or(McpError::NotFound)
}

/// Defence in depth for S3: the `Origin` of a state-changing request is the public origin, and is present.
fn check_origin(state: &TokenRoutes, headers: &HeaderMap) -> Result<(), McpError> {
    match headers.get(header::ORIGIN).map(HeaderValue::as_bytes) {
        Some(value) if value == state.origin.as_str().as_bytes() => Ok(()),
        _ => Err(McpError::Forbidden),
    }
}

/// A new id for the audit event of this request.
fn request_id() -> Option<RequestId> {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).ok()?;
    let mut text = String::from("mcp-");
    for byte in bytes {
        text.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        text.push(char::from(b"0123456789abcdef"[usize::from(byte & 15)]));
    }
    RequestId::parse(&text).ok()
}

// ---------------------------------------------------------------------------------------------------- wire types

#[derive(Serialize)]
struct TokenView {
    id: i64,
    label: String,
    created_at: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
    last_used_at: Option<i64>,
}

impl From<&TokenRecord> for TokenView {
    fn from(record: &TokenRecord) -> Self {
        Self {
            id: record.id.get(),
            label: record.label.as_str().to_owned(),
            created_at: record.created_at.unix_millis(),
            expires_at: record.expires_at.unix_millis(),
            revoked_at: record.revoked_at.map(domain::Timestamp::unix_millis),
            last_used_at: record.last_used_at.map(domain::Timestamp::unix_millis),
        }
    }
}

#[derive(Serialize)]
struct PageView {
    items: Vec<TokenView>,
    next: Option<String>,
}

#[derive(Serialize)]
struct CreatedView<'a> {
    id: i64,
    label: &'a str,
    created_at: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
    last_used_at: Option<i64>,
    /// Borrowed from the `Secret`, so the plaintext is not copied into a second string on its way to the response.
    token: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    label: String,
}

// ---------------------------------------------------------------------------------------------------- handlers

async fn list(state: &TokenRoutes, user: &AuthenticatedUser, uri: &Uri) -> Result<Response, McpError> {
    let tokens = tokens(state)?;
    let (before, limit) = list_query(uri.query())?;
    // One more than a page, to know whether there is another.
    let mut rows = tokens
        .list(&user.0.id, before, limit + 1)
        .await
        .map_err(McpError::from)?;
    let next = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().map(|last| encode_cursor(last.id))
    } else {
        None
    };
    json(
        StatusCode::OK,
        &PageView {
            items: rows.iter().map(TokenView::from).collect(),
            next,
        },
    )
}

async fn create(
    state: &TokenRoutes,
    user: &AuthenticatedUser,
    request: Request,
) -> Result<Response, McpError> {
    let tokens = tokens(state)?;
    let (parts, body) = request.into_parts();
    check_origin(state, &parts.headers)?;
    let is_json = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
        });
    if !is_json {
        return Err(McpError::UnsupportedMediaType);
    }
    let bytes = http_body_util::Limited::new(body, MAX_BODY_BYTES)
        .collect()
        .await
        .map_err(|_| McpError::PayloadTooLarge)?
        .to_bytes();
    let CreateBody { label } = serde_json::from_slice(&bytes).map_err(|_| McpError::InvalidRequest)?;
    let label = Label::parse(&label).map_err(|_| McpError::InvalidRequest)?;
    let created = tokens
        .create(&user.0.id, label, request_id())
        .await
        .map_err(McpError::from)?;
    let record = &created.record;
    json(
        StatusCode::CREATED,
        &CreatedView {
            id: record.id.get(),
            label: record.label.as_str(),
            created_at: record.created_at.unix_millis(),
            expires_at: record.expires_at.unix_millis(),
            revoked_at: None,
            last_used_at: None,
            token: created.token.expose(),
        },
    )
}

async fn revoke(
    state: &TokenRoutes,
    user: &AuthenticatedUser,
    id: &str,
    headers: &HeaderMap,
) -> Result<Response, McpError> {
    let tokens = tokens(state)?;
    check_origin(state, headers)?;
    let id = parse_id(id)?;
    tokens
        .revoke(&user.0.id, id, request_id())
        .await
        .map_err(McpError::from)?;
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}

// ---------------------------------------------------------------------------------------------------- parsing

/// A canonical positive decimal: no sign, no leading zero, no whitespace.
fn parse_id(text: &str) -> Result<TokenId, McpError> {
    let canonical = !text.is_empty()
        && text.len() <= 19
        && text.bytes().all(|b| b.is_ascii_digit())
        && !text.starts_with('0');
    canonical
        .then(|| text.parse::<i64>().ok().and_then(TokenId::new))
        .flatten()
        .ok_or(McpError::InvalidRequest)
}

/// The cursor that resumes after token `id`: base64url of the decimal id, which fits the `OpenAPI` `Cursor` pattern.
#[must_use]
pub fn encode_cursor(id: TokenId) -> String {
    URL_SAFE_NO_PAD.encode(id.to_string())
}

/// The id a cursor resumes after.
///
/// # Errors
///
/// [`McpError::InvalidRequest`] for anything that [`encode_cursor`] does not produce.
pub fn decode_cursor(cursor: &str) -> Result<TokenId, McpError> {
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_LEN {
        return Err(McpError::InvalidRequest);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| McpError::InvalidRequest)?;
    let text = String::from_utf8(bytes).map_err(|_| McpError::InvalidRequest)?;
    parse_id(&text)
}

/// `cursor` and `limit`, each at most once. Any other parameter, a repeat, an escape or a bad value is a 400 (S11).
fn list_query(query: Option<&str>) -> Result<(Option<TokenId>, usize), McpError> {
    let mut before = None;
    let mut limit = None;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').ok_or(McpError::InvalidRequest)?;
        match name {
            "cursor" if before.is_none() => before = Some(decode_cursor(value)?),
            "limit" if limit.is_none() => {
                let canonical = !value.is_empty()
                    && value.len() <= 4
                    && value.bytes().all(|b| b.is_ascii_digit())
                    && !value.starts_with('0');
                let n: usize = canonical
                    .then(|| value.parse().ok())
                    .flatten()
                    .ok_or(McpError::InvalidRequest)?;
                if n > MAX_LIMIT {
                    return Err(McpError::InvalidRequest);
                }
                limit = Some(n);
            }
            _ => return Err(McpError::InvalidRequest),
        }
    }
    Ok((before, limit.unwrap_or(DEFAULT_LIMIT)))
}
