//! The admin routes for users and access requests (T4; S1, S4, S9): list, approve, reject and update.
//!
//! The role gate has already checked `admin` when a handler runs. The rules that bind an admin (the last active admin stays;
//! nobody changes their own role, status or flag) are in [`crate::users::rules`] and are applied inside the transaction that
//! makes the change, which also ends the sessions of the user and writes the one audit event.
//!
//! Bodies are strict JSON with unknown fields refused; query parameters are each read once; a bad value is a `400` that
//! repeats nothing. Lists are keyset pages of at most 500.

use std::str::FromStr;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use domain::{Role, ShortText, UserId, UserStatus};
use serde::{Deserialize, Deserializer};

use super::json::{json_response, query_params, read_json};
use super::{IdentityState, access_problem, bad_request};
use crate::guard::{Admin, GuardedRouter, RequireRole};
use crate::users::rules::Change;
use crate::users::{AccessRequestStatus, Actor, RequestQuery, UserQuery};

pub(super) fn register(g: GuardedRouter, state: IdentityState) -> GuardedRouter {
    g.get("/api/v1/admin/users", Role::Admin, state.clone(), list_users)
        .patch(
            "/api/v1/admin/users/{id}",
            Role::Admin,
            state.clone(),
            update_user,
        )
        .get(
            "/api/v1/admin/access-requests",
            Role::Admin,
            state.clone(),
            list_requests,
        )
        .post(
            "/api/v1/admin/access-requests/{id}/approve",
            Role::Admin,
            state.clone(),
            approve,
        )
        .post(
            "/api/v1/admin/access-requests/{id}/reject",
            Role::Admin,
            state,
            reject,
        )
}

fn actor(who: &RequireRole<Admin>) -> Actor {
    Actor {
        user: who.context.user.clone(),
        request_id: who.context.request_id.clone(),
    }
}

/// `limit` from the query: 1 or more, as the spec says; the store clamps it to 500.
fn parse_limit(v: Option<&str>) -> Result<Option<u32>, ()> {
    match v {
        None => Ok(None),
        Some(text) => match text.parse::<u32>() {
            Ok(0) | Err(_) => Err(()),
            Ok(n) => Ok(Some(n)),
        },
    }
}

fn parse_user_query(raw: Option<&str>) -> Result<UserQuery, ()> {
    let [status, q, cursor, limit] = query_params(raw, &["status", "q", "cursor", "limit"])
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    Ok(UserQuery {
        status: status
            .as_deref()
            .map(UserStatus::from_str)
            .transpose()
            .map_err(|_| ())?,
        q: q.as_deref().map(ShortText::parse).transpose().map_err(|_| ())?,
        cursor,
        limit: parse_limit(limit.as_deref())?,
    })
}

fn parse_request_query(raw: Option<&str>) -> Result<RequestQuery, ()> {
    let [status, cursor, limit] = query_params(raw, &["status", "cursor", "limit"])
        .ok_or(())?
        .try_into()
        .map_err(|_| ())?;
    Ok(RequestQuery {
        status: status.as_deref().map(AccessRequestStatus::from_str).transpose()?,
        cursor,
        limit: parse_limit(limit.as_deref())?,
    })
}

async fn list_users(State(s): State<IdentityState>, who: RequireRole<Admin>, req: Request) -> Response {
    let id = who.context.request_id.clone();
    let Ok(query) = parse_user_query(req.uri().query()) else {
        return bad_request(id.as_ref());
    };
    drop(req);
    match s.admin().list_users(&query).await {
        Ok(page) => json_response(StatusCode::OK, &page, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}

async fn list_requests(State(s): State<IdentityState>, who: RequireRole<Admin>, req: Request) -> Response {
    let id = who.context.request_id.clone();
    let Ok(query) = parse_request_query(req.uri().query()) else {
        return bad_request(id.as_ref());
    };
    drop(req);
    match s.admin().list_requests(&query).await {
        Ok(page) => json_response(StatusCode::OK, &page, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}

/// `Some(Some(x))` for a value, `Some(None)` for an explicit `null`, `None` for a field that is absent. The three cases are
/// the point: `null` removes a role and an absent field leaves it.
#[allow(clippy::option_option)]
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserUpdateBody {
    #[serde(default, deserialize_with = "present")]
    #[allow(clippy::option_option)]
    role: Option<Option<Role>>,
    status: Option<UserStatus>,
    requires_approval: Option<bool>,
}

async fn update_user(
    State(s): State<IdentityState>,
    who: RequireRole<Admin>,
    path: Path<String>,
    req: Request,
) -> Response {
    let id = who.context.request_id.clone();
    let Ok(target) = UserId::from_str(&path.0) else {
        return bad_request(id.as_ref());
    };
    let body: UserUpdateBody = match read_json(req, id.as_ref()).await {
        Ok(b) => b,
        Err(response) => return response,
    };
    let change = Change {
        role: body.role,
        status: body.status,
        requires_approval: body.requires_approval,
    };
    match s.admin().update_user(&actor(&who), &target, change).await {
        Ok(user) => json_response(StatusCode::OK, &user, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}

/// The numeric id in `/access-requests/{id}/...`.
fn request_number(path: &str) -> Option<i64> {
    path.parse::<i64>().ok().filter(|n| *n > 0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalBody {
    role: Role,
}

async fn approve(
    State(s): State<IdentityState>,
    who: RequireRole<Admin>,
    path: Path<String>,
    req: Request,
) -> Response {
    let id = who.context.request_id.clone();
    let Some(number) = request_number(&path.0) else {
        return bad_request(id.as_ref());
    };
    let body: ApprovalBody = match read_json(req, id.as_ref()).await {
        Ok(b) => b,
        Err(response) => return response,
    };
    match s.admin().approve(&actor(&who), number, body.role).await {
        Ok(user) => json_response(StatusCode::OK, &user, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectBody {
    reason: ShortText,
}

async fn reject(
    State(s): State<IdentityState>,
    who: RequireRole<Admin>,
    path: Path<String>,
    req: Request,
) -> Response {
    let id = who.context.request_id.clone();
    let Some(number) = request_number(&path.0) else {
        return bad_request(id.as_ref());
    };
    let body: RejectBody = match read_json(req, id.as_ref()).await {
        Ok(b) => b,
        Err(response) => return response,
    };
    match s.admin().reject(&actor(&who), number, &body.reason).await {
        Ok(request) => json_response(StatusCode::OK, &request, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_update_distinguishes_null_from_absent() {
        let absent: UserUpdateBody = serde_json::from_str(r#"{"status":"active"}"#).unwrap();
        assert_eq!(absent.role, None);
        let null: UserUpdateBody = serde_json::from_str(r#"{"role":null}"#).unwrap();
        assert_eq!(null.role, Some(None));
        let set: UserUpdateBody = serde_json::from_str(r#"{"role":"admin"}"#).unwrap();
        assert_eq!(set.role, Some(Some(Role::Admin)));
        assert!(serde_json::from_str::<UserUpdateBody>(r#"{"role":"root"}"#).is_err());
        assert!(serde_json::from_str::<UserUpdateBody>(r#"{"email":"x"}"#).is_err());
    }

    #[test]
    fn list_queries_are_strict() {
        assert!(parse_user_query(Some("status=active&limit=5&q=ada")).is_ok());
        for bad in [
            "limit=0",
            "limit=-1",
            "limit=x",
            "status=root",
            "limit=1&limit=2",
            "status=a&status=b",
        ] {
            assert!(parse_user_query(Some(bad)).is_err(), "{bad}");
        }
        assert!(parse_request_query(Some("status=pending")).is_ok());
        assert!(
            parse_request_query(Some("status=active")).is_err(),
            "'active' is not a request status"
        );
    }
}
