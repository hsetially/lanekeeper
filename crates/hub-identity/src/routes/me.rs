//! `GET /api/v1/me` and `POST /api/v1/me/access-request` (T4; S1, S4): the signed-in user, and a request for another role.
//!
//! Both are viewer routes: a pending or disabled user is turned away before the handler (D36), so `/me` never answers for
//! someone who has no access. The user is read from the database, not from the session cache, so a role change shows at once.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use domain::{RequestId, Role, ShortText};
use ports::Users;
use serde::Deserialize;

use super::json::{json_response, read_json};
use super::{IdentityState, access_problem, unavailable};
use crate::guard::{GuardedRouter, RequireRole, Viewer};
use crate::users::Actor;
use crate::users::onboarding::needs_github_token;

pub(super) fn register(g: GuardedRouter, state: IdentityState) -> GuardedRouter {
    g.get("/api/v1/me", Role::Viewer, state.clone(), get_me).post(
        "/api/v1/me/access-request",
        Role::Viewer,
        state,
        create_access_request,
    )
}

async fn get_me(State(s): State<IdentityState>, who: RequireRole<Viewer>) -> Response {
    let id = who.context.request_id.as_ref();
    let user = match s.users().get(&who.context.user).await {
        Ok(Some(user)) => user,
        // A session for a user who is gone: nothing to show, and the session is worthless.
        Ok(None) => return unauthenticated(id),
        Err(_) => return unavailable(id),
    };
    let Ok(needs_token) = needs_github_token(&user, s.presence()).await else {
        return unavailable(id);
    };
    let Ok(mut body) = serde_json::to_value(&user) else {
        return unavailable(id);
    };
    body["swimlane_access"] = serde_json::Value::from("all");
    body["needs_github_token"] = serde_json::Value::from(needs_token);
    json_response(StatusCode::OK, &body, id)
}

fn unauthenticated(id: Option<&RequestId>) -> Response {
    crate::guard::problem::denied(crate::guard::Denied::Unauthenticated, id)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessRequestBody {
    requested_role: Role,
    reason: ShortText,
}

async fn create_access_request(
    State(s): State<IdentityState>,
    who: RequireRole<Viewer>,
    req: Request,
) -> Response {
    let id = who.context.request_id.clone();
    let body: AccessRequestBody = match read_json(req, id.as_ref()).await {
        Ok(b) => b,
        Err(response) => return response,
    };
    let actor = Actor {
        user: who.context.user.clone(),
        request_id: id.clone(),
    };
    match s
        .admin()
        .create_request(&actor, body.requested_role, &body.reason)
        .await
    {
        Ok(request) => json_response(StatusCode::ACCEPTED, &request, id.as_ref()),
        Err(e) => access_problem(e, id.as_ref()),
    }
}
