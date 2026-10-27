//! Composition (T1): the one place where the hub's routers, layers and listeners' routers are put together.
//!
//! Other crates plug in as a [`Module`]: it adds routes to a [`GuardedRouter`], which is the only way to register one, so
//! every route has a declared guard and the route table below cannot disagree with what is mounted (S4). Everything about
//! the process (headers, limits, authentication, CSRF, static files, errors, metrics, shutdown) is applied here, once, for
//! every module.
//!
//! **Authentication and CSRF are layers of the hub, not of a module** (plan 18, T4, G1). [`SessionEdge`] is a required part of
//! [`Deps`]: it carries the [`SessionAuth`] that every module's guard asks, and the two layers that make it work, which are applied
//! to the whole web router, inside the per-IP bucket and outside every route. [`AuthLayer`] turns the session cookie into an
//! `AuthContext`; [`CsrfLayer`] then refuses any state-changing request that carries a session without the double-submit token and
//! an exact `Origin`. Because they wrap the router and not the routes, a module cannot forget them, and a route added tomorrow
//! is covered the day it is added (S3, D109). A request with no session carries no ambient authority, so CSRF has nothing to
//! protect and the guard answers `401`, or the route proves the caller itself (the webhook's HMAC, MCP's bearer token).

use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::IntoResponse;
use hub_identity::csrf::{CsrfLayer, CsrfPolicy};
use hub_identity::middleware::AuthLayer;
use hub_identity::session::Sessions;
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

/// How a request becomes a caller, and the layers that make it so. Required: a hub cannot be built without one, and the only value
/// that applies no layers is behind the `testing` feature, which a release build does not have (`tests/release_build.rs`).
pub struct SessionEdge {
    auth: Arc<dyn SessionAuth>,
    layers: Option<SessionLayers>,
}

/// The two layers of the hub: who the caller is, and CSRF.
#[derive(Clone)]
struct SessionLayers {
    sessions: Arc<Sessions>,
    /// `https://host[:port]`, the one `Origin` a state-changing request may name.
    origin: String,
}

impl std::fmt::Debug for SessionEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEdge")
            .field("layers", &self.layers.is_some())
            .finish_non_exhaustive()
    }
}

impl SessionEdge {
    /// The cookie sessions of the identity module: `auth` reads the context that [`AuthLayer`] puts in the request, and the hub
    /// applies [`AuthLayer`] over `sessions` and a required [`CsrfLayer`] over `public_origin` to every route.
    #[must_use]
    pub fn cookie(sessions: Arc<Sessions>, public_origin: &str, auth: Arc<dyn SessionAuth>) -> Self {
        Self {
            auth,
            layers: Some(SessionLayers {
                sessions,
                origin: public_origin.trim_end_matches('/').to_owned(),
            }),
        }
    }

    /// The [`SessionAuth`] every module's guard asks.
    #[must_use]
    pub fn auth(&self) -> Arc<dyn SessionAuth> {
        Arc::clone(&self.auth)
    }

    /// Whether the hub-wide authentication and CSRF layers are applied.
    #[must_use]
    pub fn has_layers(&self) -> bool {
        self.layers.is_some()
    }
}

/// A router that a module built and guarded itself (identity's, which has its own guard type), with its route table. It is merged
/// into the web router like a [`Module`]'s routes, after its own fallback has been removed, so that the hub's alone answers an
/// unmatched path (`Router::reset_fallback`).
pub struct MountedRouter {
    pub name: &'static str,
    pub router: Router,
    /// Every route in `router`, with its guard. The composition checks that none of them is registered twice.
    pub specs: Vec<RouteSpec>,
}

impl std::fmt::Debug for MountedRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountedRouter")
            .field("name", &self.name)
            .field("routes", &self.specs.len())
            .finish_non_exhaustive()
    }
}

/// What a hub is built from. The ports the modules need are inside the modules; this holds what is about the process.
pub struct Deps {
    pub settings: HubSettings,
    /// Who the request is, and the layers that find out (the identity module's in production).
    pub session: SessionEdge,
    pub modules: Vec<Arc<dyn Module>>,
    /// Routers that were built and guarded elsewhere.
    pub mounted: Vec<MountedRouter>,
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

/// A path as the router sees it: the parameters' names do not matter, so `/a/{id}` and `/a/{name}` are one route.
fn shape(path: &str) -> String {
    path.split('/')
        .map(|segment| if segment.starts_with('{') { "{}" } else { segment })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `specs` register a route that `table` already holds. axum panics when two routers define one method and path; the
/// composition says which module did it instead.
fn first_overlap(table: &[RouteSpec], specs: &[RouteSpec]) -> Option<RouteError> {
    let taken: std::collections::HashSet<(&str, String)> = table
        .iter()
        .map(|spec| (spec.method.as_str(), shape(&spec.path)))
        .collect();
    specs
        .iter()
        .find(|spec| taken.contains(&(spec.method.as_str(), shape(&spec.path))))
        .map(|spec| RouteError::Duplicate {
            method: spec.method.clone(),
            path: spec.path.clone(),
        })
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
        Self::build_with(deps, Drain::new())
    }

    /// Compose the hub over a [`Drain`] that already exists. The process makes it first, because the platform's services take its
    /// `closing` signal and the platform has to be built before the hub it adds routes to.
    pub fn build_with(deps: Deps, drain: Drain) -> Result<Self, BuildError> {
        let Deps {
            settings,
            session,
            modules,
            mounted,
            static_files,
            checks,
        } = deps;
        let SessionEdge { auth, layers } = session;
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
            let guarded = module.mount(GuardedRouter::new(Arc::clone(&auth), Some(Arc::clone(&throttle))));
            let (router, specs) = guarded.into_parts().map_err(|error| BuildError::Route {
                module: module.name(),
                error,
            })?;
            if let Some(error) = first_overlap(&table, &specs) {
                return Err(BuildError::Route {
                    module: module.name(),
                    error,
                });
            }
            web = web.merge(router);
            table.extend(specs);
        }
        for MountedRouter { name, router, specs } in mounted {
            if let Some(error) = first_overlap(&table, &specs) {
                return Err(BuildError::Route { module: name, error });
            }
            // Whatever answered an unmatched path in `router` is dropped here too, whoever built it: `Router::merge` adopts a custom
            // fallback from one side and panics on two, and the hub's alone answers (below), for every module.
            web = web.merge(router.reset_fallback());
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
        // Authentication, then CSRF, around every route and the fallback, and inside the limits: the per-IP bucket refuses a flood of
        // unknown cookies before any of them costs a database read. `AuthLayer` is the outer of the two because `CsrfLayer` reads
        // what it puts in the request, and does nothing without it (`tests/guard.rs` holds the order).
        let web = match layers {
            Some(SessionLayers { sessions, origin }) => web
                .layer(CsrfLayer::new(&origin, CsrfPolicy::Required))
                .layer(AuthLayer::new(sessions)),
            None => web,
        };
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

// Everything below is compiled only with the crate's own `testing` feature, which a release build does not have, and sits after
// all the production code so that `tests/release_build.rs` can scan what comes before it for the names that must stay out.
#[cfg(feature = "testing")]
impl SessionEdge {
    /// A [`SessionAuth`] and no layers: for tests of the composition that have no database behind them. The guard asks `auth`
    /// and nothing puts a session in the request, so nothing is checked for CSRF.
    #[must_use]
    pub fn without_sessions(auth: Arc<dyn SessionAuth>) -> Self {
        Self { auth, layers: None }
    }
}
