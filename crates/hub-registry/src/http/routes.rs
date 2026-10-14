//! The fifteen read routes (T6, S4, S11, S12, S21; plan A9).
//!
//! Every route is registered through hub-platform's [`GuardedRouter`] at Viewer, so a request without an active session, from a pending
//! or a disabled user, or from one without a role never reaches a handler (S4); each handler then calls the service, which checks the
//! user *again* (the port refuses a user without a role whatever route reached it). The table [`registry_route_specs`] is what the
//! route-guard harness compares with `api/openapi.yaml`, and a test of this module compares it with what [`registry_routes`]
//! mounted.
//!
//! **Answers.** A success is JSON, `Cache-Control: private` and `nosniff`. The four `content` operations (compare, effective config,
//! file content and the served file) carry the `ETag` the contract declares and answer a matching `If-None-Match` with 304 and no body;
//! the others carry none, because the contract declares none. A failure is `application/problem+json` from [`ApiError`]: a code and a
//! status, never the cause, a path or SQL (S21).
//!
//! **Tenants (A30).** Where one tenant stands for a swimlane that has several (the grid, the single-file compare and the whole-tree
//! compare), the answer says which in the header `X-Lanekeeper-Tenants`, as `swimlane=tenant` pairs separated by commas.

use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::Path;
use axum::extract::rejection::PathRejection;
use axum::response::Response;
use bytes::Bytes;
use domain::{FileContent, Role, SwimlaneId, TenantId, User};
use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH, X_CONTENT_TYPE_OPTIONS};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use hub_platform::{ApiError, AuthenticatedUser, GuardedRouter, RouteGuard, RouteSpec};
use ports::ReadError;
use serde::Serialize;

use super::dto::{ComparisonDto, FileContentDto};
use super::query;
use crate::read::{Named, RegistryApi, Tagged};

pub const SWIMLANES: &str = "/api/v1/swimlanes";
pub const SWIMLANE: &str = "/api/v1/swimlanes/{id}";
pub const TREE: &str = "/api/v1/swimlanes/{id}/tree";
pub const DRIFT: &str = "/api/v1/swimlanes/{id}/drift";
pub const CONTENT: &str = "/api/v1/swimlanes/{id}/files/content";
pub const HISTORY: &str = "/api/v1/swimlanes/{id}/files/history";
pub const EFFECTIVE: &str = "/api/v1/swimlanes/{id}/effective";
pub const PENDING_RESTARTS: &str = "/api/v1/swimlanes/{id}/pending-restarts";
pub const SERVED: &str = "/api/v1/swimlanes/{id}/served";
pub const COMPARE: &str = "/api/v1/compare";
pub const COMPARE_TREE: &str = "/api/v1/compare/tree";
pub const GRID: &str = "/api/v1/grid";
pub const SEARCH_SETTINGS: &str = "/api/v1/search/settings";
pub const SEARCH_TEXT: &str = "/api/v1/search/text";
pub const FINDINGS: &str = "/api/v1/findings";

/// The header that names the tenant standing for each swimlane (A30).
pub const TENANTS_HEADER: &str = "x-lanekeeper-tenants";

/// Every path this module mounts, all `GET` at Viewer.
pub const PATHS: [&str; 15] = [
    SWIMLANES,
    SWIMLANE,
    TREE,
    DRIFT,
    CONTENT,
    HISTORY,
    EFFECTIVE,
    PENDING_RESTARTS,
    SERVED,
    COMPARE,
    COMPARE_TREE,
    GRID,
    SEARCH_SETTINGS,
    SEARCH_TEXT,
    FINDINGS,
];

/// The route table of this module, for the route-guard harness: what [`registry_routes`] mounts, each with its guard.
#[must_use]
pub fn registry_route_specs() -> Vec<RouteSpec> {
    PATHS
        .iter()
        .map(|path| RouteSpec::new(&Method::GET, path, RouteGuard::Role(Role::Viewer)))
        .collect()
}

/// What a handler is given: the user the guard let through, the path's swimlane id (still text), the URI and the headers.
struct Call {
    user: User,
    /// `None` when the route has no `{id}` or the path parameter would not decode.
    id: Option<String>,
    uri: Uri,
    headers: HeaderMap,
}

impl Call {
    fn swimlane(&self) -> Result<SwimlaneId, ApiError> {
        let id = self.id.as_deref().ok_or(ApiError::BadRequest)?;
        SwimlaneId::parse(id).map_err(|_| ApiError::BadRequest)
    }

    fn query(&self) -> Option<&str> {
        self.uri.query()
    }

    /// The `If-None-Match` value; one that is not text is as good as none.
    fn if_none_match(&self) -> Option<&str> {
        self.headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok())
    }
}

type Reply = Result<Response, ApiError>;

/// What a failed read is to the client. The cause was logged where it happened.
fn api_error(error: ReadError) -> ApiError {
    match error {
        ReadError::Forbidden => ApiError::Forbidden,
        ReadError::NotFound => ApiError::NotFound,
        ReadError::BadRequest => ApiError::BadRequest,
        ReadError::Unavailable => ApiError::Unavailable,
    }
}

/// The headers of a success besides the body's own.
#[derive(Default)]
struct Meta<'a> {
    etag: Option<&'a str>,
    tenants: &'a [(SwimlaneId, TenantId)],
}

fn json<T: Serialize>(value: &T, meta: &Meta<'_>) -> Reply {
    let body = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    match meta.etag {
        // A tagged answer may be kept and must be asked about again.
        Some(etag) => {
            headers.insert(ETAG, HeaderValue::from_str(etag).map_err(|_| ApiError::Internal)?);
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        }
        None => {
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
        }
    }
    if !meta.tenants.is_empty() {
        headers.insert(
            HeaderName::from_static(TENANTS_HEADER),
            tenants_value(meta.tenants).ok_or(ApiError::Internal)?,
        );
    }
    Ok(response)
}

/// `swimlane=tenant` pairs, sorted, each swimlane once.
fn tenants_value(tenants: &[(SwimlaneId, TenantId)]) -> Option<HeaderValue> {
    let mut pairs: Vec<String> = tenants.iter().map(|(s, t)| format!("{s}={t}")).collect();
    pairs.sort();
    pairs.dedup();
    HeaderValue::from_str(&pairs.join(",")).ok()
}

/// 304: the tag and nothing else.
fn not_modified(etag: &str) -> Reply {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    let headers = response.headers_mut();
    headers.insert(ETAG, HeaderValue::from_str(etag).map_err(|_| ApiError::Internal)?);
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(response)
}

/// A file's content as the contract has it. Decoding the text of a large file is not done on the async thread (rule 4).
async fn content_body(api: &dyn RegistryApi, content: FileContent) -> Result<FileContentDto, ApiError> {
    const INLINE_BYTES: usize = 64 * 1024;
    let limits = api.limits().clone();
    if content.bytes.as_ref().map_or(0, Bytes::len) <= INLINE_BYTES {
        return Ok(FileContentDto::new(&content, &limits));
    }
    tokio::task::spawn_blocking(move || FileContentDto::new(&content, &limits))
        .await
        .map_err(|_| ApiError::Internal)
}

// ------------------------------------------------------------------------------------------------ handlers

async fn list_swimlanes(api: &dyn RegistryApi, call: Call) -> Reply {
    let page = query::page_only(call.query())?;
    let out = api.swimlanes_page(&call.user, page).await.map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn get_swimlane(api: &dyn RegistryApi, call: Call) -> Reply {
    query::none(call.query())?;
    let out = api
        .swimlane(&call.user, &call.swimlane()?)
        .await
        .map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn list_tree(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::tree(call.query())?;
    let out = api
        .tree_filtered(&call.user, &call.swimlane()?, &q.prefix, q.drift, q.page)
        .await
        .map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn list_drift(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::drift(call.query())?;
    let out = api
        .drift(&call.user, &call.swimlane()?, q.state, &q.prefix, q.page)
        .await
        .map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn get_content(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::content(call.query())?;
    let swimlane = call.swimlane()?;
    let tagged = api
        .content_tagged(&call.user, &swimlane, &q.path, q.source, call.if_none_match())
        .await
        .map_err(api_error)?;
    match tagged {
        Tagged::NotModified { etag } => not_modified(&etag),
        Tagged::Fresh { etag, value } => {
            let body = content_body(api, value).await?;
            json(
                &body,
                &Meta {
                    etag: Some(&etag),
                    tenants: &[],
                },
            )
        }
    }
}

async fn list_history(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::history(call.query())?;
    let out = api
        .history(&call.user, &call.swimlane()?, &q.path, q.page)
        .await
        .map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn get_effective(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::effective(call.query())?;
    let swimlane = call.swimlane()?;
    let tagged = api
        .effective_tagged(
            &call.user,
            &swimlane,
            &q.file,
            Some(&q.tenant),
            q.channel.as_ref(),
            call.if_none_match(),
        )
        .await
        .map_err(api_error)?;
    match tagged {
        Tagged::NotModified { etag } => not_modified(&etag),
        Tagged::Fresh { etag, value } => json(
            &value,
            &Meta {
                etag: Some(&etag),
                tenants: &[],
            },
        ),
    }
}

async fn list_pending_restarts(api: &dyn RegistryApi, call: Call) -> Reply {
    let page = query::page_only(call.query())?;
    let out = api
        .pending_restarts_page(&call.user, &call.swimlane()?, page)
        .await
        .map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn get_served(api: &dyn RegistryApi, call: Call) -> Reply {
    let request = query::served(call.query())?;
    let swimlane = call.swimlane()?;
    let tagged = api
        .served(&call.user, &swimlane, request, call.if_none_match())
        .await
        .map_err(api_error)?;
    match tagged {
        Tagged::NotModified { etag } => not_modified(&etag),
        Tagged::Fresh { etag, value } => json(
            &value,
            &Meta {
                etag: Some(&etag),
                tenants: &[],
            },
        ),
    }
}

async fn compare_file(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::compare(call.query())?;
    let tagged = api
        .compare_tagged(
            &call.user,
            q.left,
            q.right,
            &q.file,
            q.channel.as_ref(),
            call.if_none_match(),
        )
        .await
        .map_err(api_error)?;
    match tagged {
        Tagged::NotModified { etag } => not_modified(&etag),
        Tagged::Fresh {
            etag,
            value: Named { value, tenants },
        } => json(
            &ComparisonDto::from(&value),
            &Meta {
                etag: Some(&etag),
                tenants: &tenants,
            },
        ),
    }
}

async fn compare_tree(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::compare_tree(call.query())?;
    let Named { value, tenants } = api
        .compare_tree_under(&call.user, q.left, q.right, &q.prefix, q.page)
        .await
        .map_err(api_error)?;
    json(
        &value,
        &Meta {
            etag: None,
            tenants: &tenants,
        },
    )
}

async fn get_grid(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::grid(call.query())?;
    let Named { value, tenants } = api.grid_named(&call.user, q).await.map_err(api_error)?;
    json(
        &value,
        &Meta {
            etag: None,
            tenants: &tenants,
        },
    )
}

async fn search_settings(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::search_settings(call.query())?;
    let out = api.search_settings(&call.user, q).await.map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn search_text(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::search_text(call.query())?;
    let out = api.search_text(&call.user, q).await.map_err(api_error)?;
    json(&out, &Meta::default())
}

async fn list_findings(api: &dyn RegistryApi, call: Call) -> Reply {
    let q = query::findings(call.query())?;
    let out = api.findings(&call.user, q).await.map_err(api_error)?;
    json(&out, &Meta::default())
}

// ------------------------------------------------------------------------------------------------ mounting

/// Register one `GET` route at Viewer whose handler is `$handler(api, call)`.
macro_rules! mount {
    ($router:expr, $api:expr, $path:expr, $handler:path) => {
        $router.route(&Method::GET, $path, Role::Viewer, {
            let api = Arc::clone($api);
            move |Extension(AuthenticatedUser(user)): Extension<AuthenticatedUser>,
                  id: Result<Path<String>, PathRejection>,
                  uri: Uri,
                  headers: HeaderMap| {
                let api = Arc::clone(&api);
                async move {
                    let call = Call {
                        user,
                        id: id.ok().map(|Path(id)| id),
                        uri,
                        headers,
                    };
                    $handler(api.as_ref(), call).await
                }
            }
        })
    };
}

/// Mount the fifteen read operations of the contract on `router`, served by `api`. All are `GET`, all at Viewer.
#[must_use]
pub fn registry_routes(router: GuardedRouter, api: Arc<dyn RegistryApi>) -> GuardedRouter {
    let handle = &api;
    let router = mount!(router, handle, SWIMLANES, list_swimlanes);
    let router = mount!(router, handle, SWIMLANE, get_swimlane);
    let router = mount!(router, handle, TREE, list_tree);
    let router = mount!(router, handle, DRIFT, list_drift);
    let router = mount!(router, handle, CONTENT, get_content);
    let router = mount!(router, handle, HISTORY, list_history);
    let router = mount!(router, handle, EFFECTIVE, get_effective);
    let router = mount!(router, handle, PENDING_RESTARTS, list_pending_restarts);
    let router = mount!(router, handle, SERVED, get_served);
    let router = mount!(router, handle, COMPARE, compare_file);
    let router = mount!(router, handle, COMPARE_TREE, compare_tree);
    let router = mount!(router, handle, GRID, get_grid);
    let router = mount!(router, handle, SEARCH_SETTINGS, search_settings);
    let router = mount!(router, handle, SEARCH_TEXT, search_text);
    let router = mount!(router, handle, FINDINGS, list_findings);
    // Every route holds a handle of its own; this one is not needed any more.
    drop(api);
    router
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tenants_header_is_sorted_pairs_with_each_swimlane_once() {
        let pair = |s: &str, t: &str| (SwimlaneId::parse(s).unwrap(), TenantId::parse(t).unwrap());
        let value = tenants_value(&[pair("b2", "sit2"), pair("a1", "sit1"), pair("a1", "sit1")]).unwrap();
        assert_eq!(value.to_str().unwrap(), "a1=sit1,b2=sit2");
    }

    #[test]
    fn every_path_is_listed_once_and_the_table_is_all_viewer_gets() {
        let specs = registry_route_specs();
        assert_eq!(specs.len(), 15);
        let mut paths: Vec<&str> = specs.iter().map(|s| s.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), 15);
        assert!(
            specs
                .iter()
                .all(|s| s.method == "GET" && s.guard == RouteGuard::Role(Role::Viewer))
        );
    }

    #[test]
    fn a_read_error_is_the_problem_of_its_kind() {
        assert_eq!(api_error(ReadError::Forbidden), ApiError::Forbidden);
        assert_eq!(api_error(ReadError::NotFound), ApiError::NotFound);
        assert_eq!(api_error(ReadError::BadRequest), ApiError::BadRequest);
        assert_eq!(api_error(ReadError::Unavailable), ApiError::Unavailable);
    }
}
