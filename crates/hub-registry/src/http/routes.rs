//! Routes (stub).
use crate::read::RegistryApi;
use hub_platform::{GuardedRouter, RouteSpec};
use std::sync::Arc;

pub fn registry_route_specs() -> Vec<RouteSpec> {
    Vec::new()
}
pub fn registry_routes(router: GuardedRouter, _api: Arc<dyn RegistryApi>) -> GuardedRouter {
    router
}
