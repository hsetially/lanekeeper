//! `PUT` and `DELETE /swimlanes/{id}/files`, `POST .../files/revert` and `POST .../restart`.

use axum::response::Response;
use domain::User;
use http::HeaderMap;
use hub_platform::ApiError;
use serde::Deserialize;

use super::problem::{outcome, write_error};
use super::wire;
use super::{Stop, WriteState, context, is_json, refusal, stop, swimlane};

/// What the three phases of every handler share: the swimlane of the path and the context of the call, or the response that
/// ends the request.
fn start(id: &str, user: User, headers: &HeaderMap) -> Result<(domain::SwimlaneId, domain::WriteCtx), Stop> {
    Ok((swimlane(id)?, context(user, headers)?))
}

/// A JSON body must say so: anything else is a 415.
fn json_only(headers: &HeaderMap) -> Result<(), Stop> {
    if is_json(headers) {
        Ok(())
    } else {
        Err(stop(ApiError::UnsupportedMediaType))
    }
}

pub(super) async fn edit(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let (swimlane, ctx) = match start(id, user, headers) {
        Ok(x) => x,
        Err(e) => return *e,
    };
    if let Err(e) = json_only(headers) {
        return *e;
    }
    let request = match wire::edit_request(swimlane, body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.edit(&ctx, request).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}

/// The query of `DELETE /files`. Every value is parsed into its type before the service sees it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteQuery {
    path: String,
    expected: String,
    #[serde(default)]
    preview_id: Option<String>,
}

pub(super) async fn delete(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    query: DeleteQuery,
) -> Response {
    let (swimlane, ctx) = match start(id, user, headers) {
        Ok(x) => x,
        Err(e) => return *e,
    };
    let request = match wire::delete_request(
        swimlane,
        &query.path,
        &query.expected,
        query.preview_id.as_deref(),
    ) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.delete(&ctx, request).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}

pub(super) async fn revert(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let (swimlane, ctx) = match start(id, user, headers) {
        Ok(x) => x,
        Err(e) => return *e,
    };
    if let Err(e) = json_only(headers) {
        return *e;
    }
    let request = match wire::revert_request(swimlane, body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.revert(&ctx, request).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}

pub(super) async fn restart(
    state: &WriteState,
    id: &str,
    user: User,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let (swimlane, ctx) = match start(id, user, headers) {
        Ok(x) => x,
        Err(e) => return *e,
    };
    if let Err(e) = json_only(headers) {
        return *e;
    }
    let request = match wire::restart_request(swimlane, body) {
        Ok(r) => r,
        Err(why) => return refusal(why),
    };
    match state.service.restart(&ctx, request).await {
        Ok(done) => outcome(&done),
        Err(e) => write_error(e),
    }
}
