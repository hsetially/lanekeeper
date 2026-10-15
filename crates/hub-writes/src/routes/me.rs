//! The routes of the signed-in user's GitHub token: `PUT /me/github-token` and `DELETE /me/github-token` (S8).
//!
//! The token arrives in the body of a PUT, is read into a `Secret`, and goes to the vault; nothing here prints it, keeps it or
//! puts it in a response. The answer says whose token is stored and what was verified, never the token, and is not cached. The
//! vault's refusals are mapped to fixed problems: the words are this file's, not GitHub's and not the input's (S21).

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use domain::User;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use hub_platform::ApiError;
use serde_json::{Map, Value, json};

use super::problem::{problem, write_error};
use super::wire;
use super::{WriteState, context, is_json, refusal, stop};
use crate::github::TokenShapeError;
use crate::vault::{StoredToken, TokenError};

/// What the policy says, in the words the user sees (S8).
const POLICY: &str = "Only fine-grained personal access tokens (they start with github_pat_) are accepted. Classic tokens, \
OAuth tokens and app tokens are refused by policy. Create a fine-grained token that can push to both the base and the tenant \
repository.";

pub(super) async fn put(state: &WriteState, user: User, headers: &HeaderMap, body: &[u8]) -> Response {
    let ctx = match context(user, headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    if !is_json(headers) {
        return *stop(ApiError::UnsupportedMediaType);
    }
    let token = match wire::github_token_request(body) {
        Ok(t) => t,
        Err(why) => return refusal(why),
    };
    match state.tokens.put_token(&ctx, token).await {
        Ok(stored) => stored_response(&stored),
        Err(e) => token_error(e),
    }
}

pub(super) async fn delete(state: &WriteState, user: User, headers: &HeaderMap) -> Response {
    let ctx = match context(user, headers) {
        Ok(c) => c,
        Err(e) => return *e,
    };
    match state.tokens.delete_token(&ctx).await {
        Ok(()) => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        Err(e) => token_error(e),
    }
}

/// `GithubCredential`: the login, when it was verified and what was verified. The expiry the vault keeps is not in the contract's
/// object, so it is not here.
fn stored_response(stored: &StoredToken) -> Response {
    let body = json!({
        "github_login": stored.github_login,
        "verified_at": stored.verified_at,
        "scopes": stored.scopes,
    });
    match serde_json::to_vec(&body) {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            // Whose token it is, for the person who asked, now.
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(_) => ApiError::Internal.into_response(),
    }
}

fn unprocessable(
    code: &'static str,
    title: &'static str,
    detail: &'static str,
    mut extra: Map<String, Value>,
) -> Response {
    extra.insert("detail".to_owned(), Value::String(detail.to_owned()));
    problem(StatusCode::UNPROCESSABLE_ENTITY, code, title, extra)
}

/// A refusal of the vault, as the response the contract promises: 400 for a request that is not one, 403, 422 for a token that is
/// refused (with a code the web app can show), and 503 when GitHub, KMS or the store did not answer.
fn token_error(error: TokenError) -> Response {
    let none = Map::new;
    match error {
        TokenError::Forbidden => ApiError::Forbidden.into_response(),
        TokenError::IdempotencyConflict => write_error(ports::WriteError::IdempotencyConflict),
        TokenError::Shape(TokenShapeError::Empty | TokenShapeError::TooLong) => {
            ApiError::BadRequest.into_response()
        }
        TokenError::Shape(TokenShapeError::Classic) => {
            unprocessable("classic_token", "Classic tokens are not accepted", POLICY, none())
        }
        TokenError::Shape(TokenShapeError::NotFineGrained | TokenShapeError::BadChars) => unprocessable(
            "not_fine_grained_token",
            "Not a fine-grained token",
            POLICY,
            none(),
        ),
        TokenError::Rejected => unprocessable(
            "token_rejected",
            "GitHub does not accept the token",
            "GitHub did not accept this token. It may be wrong, expired or revoked.",
            none(),
        ),
        TokenError::NoPushAccess { base, tenant } => {
            let mut extra = Map::new();
            extra.insert("base".to_owned(), Value::Bool(base));
            extra.insert("tenant".to_owned(), Value::Bool(tenant));
            unprocessable(
                "token_missing_access",
                "The token cannot push to both repositories",
                "The token must be able to push to both the base and the tenant repository.",
                extra,
            )
        }
        TokenError::LoginMismatch => unprocessable(
            "github_account_mismatch",
            "The token belongs to another GitHub account",
            "This token belongs to another GitHub account than the one linked to your user. Ask an admin to reset the link.",
            none(),
        ),
        TokenError::Expired => unprocessable(
            "token_expired",
            "The stored token has expired",
            "The stored token has expired. Save a new one.",
            none(),
        ),
        TokenError::Undecryptable => unprocessable(
            "token_unreadable",
            "The stored token cannot be opened",
            "The stored token cannot be opened. Save it again.",
            none(),
        ),
        TokenError::NoToken => ApiError::NotFound.into_response(),
        TokenError::GithubUnavailable | TokenError::Unavailable => {
            let mut response = ApiError::Unavailable.into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            response
        }
    }
}
