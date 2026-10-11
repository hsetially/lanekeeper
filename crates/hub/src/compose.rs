//! Composition (T1): the one place where the hub's routers, layers and listeners' routers are put together.
//!
//! Other crates plug in as a [`Module`]: it adds routes to a [`GuardedRouter`], which is the only way to register one, so
//! every route has a declared guard and the route table below cannot disagree with what is mounted (S4). Everything about
//! the process (headers, limits, static files, errors, metrics, shutdown) is applied here, once, for every module.

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::IntoResponse;
use hub_platform::{GuardedRouter, RouteError, RouteSpec, SessionAuth, UserThrottle};

use crate::client_ip::TrustedProxies;
use crate::config::HubSettings;
use crate::edge::{ContextState, drain_gate, normalize_errors, record_route, request_context};
use crate::headers::{HeaderPolicy, security_headers};
use crate::limits::{Limits, request_limits};
use crate::metrics::Metrics;
use crate::ops::{self, Readiness, ReadyCheck};
use crate::shutdown::Drain;
use crate::static_files::{self, StaticFiles};

/// A crate's contribution to the web router: its routes, each with a guard.
pub trait Module: Send + Sync {
    /// A fixed name, for logs.
    fn name(&self) -> &'static str;

    /// Add this module's routes.
    fn mount(&self, router: GuardedRouter) -> GuardedRouter;
}

/// What a hub is built from. The ports the modules need are inside the modules; this holds what is about the process.
pub struct Deps {
    pub settings: HubSettings,
    /// Who the request is (03a's implementation in production; a fake in tests).
    pub auth: Arc<dyn SessionAuth>,
    pub modules: Vec<Arc<dyn Module>>,
    /// The built web app. `None` serves only the API (tests, and nothing else).
    pub static_files: Option<Arc<StaticFiles>>,
    pub checks: Vec<(&'static str, Arc<dyn ReadyCheck>)>,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("modules", &self.modules.len())
            .finish_non_exhaustive()
    }
}

/// Why a hub could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("module {module} registered a route that is not valid: {error}")]
    Route { module: &'static str, error: RouteError },
}

/// A composed hub: the two routers and what the listeners and the tests need.
pub struct Hub {
    /// The web router, with every layer applied.
    pub web: Router,
    /// The operations router (`/healthz`, `/readyz`, `/metrics`).
    pub ops: Router,
    /// The route table of every module: what the S4 test checks against `api/openapi.yaml`.
    pub routes: Vec<RouteSpec>,
    pub drain: Drain,
    pub metrics: Arc<Metrics>,
    pub readiness: Readiness,
    pub limits: Arc<Limits>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("routes", &self.routes.len())
            .finish_non_exhaustive()
    }
}

impl Hub {
    /// Compose the hub.
    pub fn build(deps: Deps) -> Result<Self, BuildError> {
        let Deps {
            settings,
            auth,
            modules,
            static_files,
            checks,
        } = deps;
        let drain = Drain::new();
        let metrics = Arc::new(Metrics::new());
        let readiness = Readiness::new(drain.clone());
        for (name, check) in checks {
            readiness.register(name, check);
        }
        let limits = Limits::new(&settings.limits, TrustedProxies::new(settings.trusted_proxy_hops));
        let throttle: Arc<dyn UserThrottle> = Arc::clone(&limits) as Arc<dyn UserThrottle>;

        let mut table = Vec::new();
        let mut web: Router = Router::new();
        for module in &modules {
            let mounted = module.mount(GuardedRouter::new(Arc::clone(&auth), Some(Arc::clone(&throttle))));
            let (router, specs) = mounted.into_parts().map_err(|error| BuildError::Route {
                module: module.name(),
                error,
            })?;
            web = web.merge(router);
            table.extend(specs);
        }

        web = match static_files {
            Some(files) => web.fallback(move |request: Request| {
                let files = Arc::clone(&files);
                async move { static_files::serve(State(files), request).await }
            }),
            None => web.fallback(|| async { hub_platform::ApiError::NotFound.into_response() }),
        };

        let policy = Arc::new(HeaderPolicy {
            trusted_types: settings.trusted_types.clone(),
        });
        // Innermost first. Every one of these runs after routing, so `MatchedPath` is set for all of them.
        let web = web
            .layer(from_fn_with_state(Arc::clone(&limits), request_limits))
            .layer(from_fn_with_state(drain.clone(), drain_gate))
            .layer(from_fn(normalize_errors))
            .layer(from_fn_with_state(policy, security_headers))
            .layer(from_fn(record_route))
            .layer(from_fn_with_state(
                ContextState {
                    metrics: Arc::clone(&metrics),
                },
                request_context,
            ))
            // The limits layer enforces the body cap, so axum's own 2 MiB extractor limit must not undercut it.
            .layer(DefaultBodyLimit::disable());

        let ops = ops::router(readiness.clone(), Arc::clone(&metrics));
        Ok(Self {
            web,
            ops,
            routes: table,
            drain,
            metrics,
            readiness,
            limits,
        })
    }
}
