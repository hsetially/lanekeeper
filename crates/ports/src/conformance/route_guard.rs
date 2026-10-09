//! The route-table harness for S4: every route has a declared guard, and it agrees with the `OpenAPI` spec.
//!
//! Prompt 03a registers routes with `#[guarded(role = ...)]` and exports its route table; prompts 03a, 05 and
//! 07 each call [`assert_route_table`] against `api/openapi.yaml` for the routes they add. The harness knows
//! nothing about axum: callers turn their route table into [`RegisteredRoute`]s.
//!
//! Rules:
//! - A route with [`Guard::Unguarded`] fails, always.
//! - The guard's role must equal the spec's `x-required-role`. `x-required-role: none` is allowed only for a
//!   route registered as [`Guard::Public`] with a reason, and the other way round.
//! - A registered route that is not in the spec, or a spec operation with no registered route, fails unless it
//!   is listed in [`Allowances`] with a reason.
//! - Paths are compared as written, after `{id}` and `:id` parameters are normalised. The spec lists full
//!   paths (including `/api/v1`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use domain::{Role, User, UserStatus};

/// How a registered route is protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guard {
    /// Requires an active user with at least this role.
    Role(Role),
    /// Open to everyone by design (sign-in, the GitHub webhook that checks its own HMAC), with the reason.
    Public { reason: String },
    /// No guard declared. Never acceptable.
    Unguarded,
}

/// A route in the running hub's route table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredRoute {
    pub method: String,
    pub path: String,
    pub guard: Guard,
}

impl RegisteredRoute {
    pub fn new(method: &str, path: &str, guard: Guard) -> Self {
        Self {
            method: method.to_owned(),
            path: path.to_owned(),
            guard,
        }
    }
}

/// The `x-required-role` of a spec operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecRole {
    Role(Role),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecOperation {
    pub method: String,
    pub path: String,
    pub required: SpecRole,
}

/// The spec could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError(pub String);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "openapi spec: {}", self.0)
    }
}

impl std::error::Error for SpecError {}

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "patch", "head", "options", "trace",
];

/// Read every operation and its `x-required-role` from an `OpenAPI` document. An operation without a valid
/// `x-required-role` (`viewer`, `editor`, `operator`, `admin` or `none`) is an error.
pub fn spec_operations_from_openapi(yaml: &str) -> Result<Vec<SpecOperation>, SpecError> {
    #[derive(serde::Deserialize)]
    struct Doc {
        paths: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    }
    let doc: Doc = serde_saphyr::from_str(yaml).map_err(|e| SpecError(format!("cannot parse: {e}")))?;
    let mut out = Vec::new();
    for (path, item) in doc.paths {
        for (method, op) in item {
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let role = op
                .get("x-required-role")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    SpecError(format!("{} {path} has no x-required-role", method.to_uppercase()))
                })?;
            let required = if role == "none" {
                SpecRole::None
            } else {
                SpecRole::Role(role.parse().map_err(|_| {
                    SpecError(format!(
                        "{} {path} has an invalid x-required-role",
                        method.to_uppercase()
                    ))
                })?)
            };
            out.push(SpecOperation {
                method: method.to_uppercase(),
                path: path.clone(),
                required,
            });
        }
    }
    Ok(out)
}

/// Routes that may exist on one side only, each with the reason.
#[derive(Debug, Clone, Default)]
pub struct Allowances {
    /// Spec operations that this binary does not register (yet), for example another crate's routes.
    pub spec_only: Vec<(String, String, String)>,
    /// Registered routes that are not API operations (health checks, metrics, static assets).
    pub route_only: Vec<(String, String, String)>,
}

impl Allowances {
    #[must_use]
    pub fn spec_only(mut self, method: &str, path: &str, reason: &str) -> Self {
        self.spec_only
            .push((method.to_owned(), path.to_owned(), reason.to_owned()));
        self
    }

    #[must_use]
    pub fn route_only(mut self, method: &str, path: &str, reason: &str) -> Self {
        self.route_only
            .push((method.to_owned(), path.to_owned(), reason.to_owned()));
        self
    }
}

/// One thing wrong with the route table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unguarded {
        method: String,
        path: String,
    },
    RoleMismatch {
        method: String,
        path: String,
        registered: Role,
        spec: Role,
    },
    /// The route says public and the spec names a role, or the other way round.
    PublicityMismatch {
        method: String,
        path: String,
    },
    PublicWithoutReason {
        method: String,
        path: String,
    },
    NotInSpec {
        method: String,
        path: String,
    },
    NotRegistered {
        method: String,
        path: String,
    },
    AllowanceWithoutReason {
        method: String,
        path: String,
    },
    DuplicateRoute {
        method: String,
        path: String,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unguarded { method, path } => write!(f, "{method} {path} has no guard"),
            Self::RoleMismatch {
                method,
                path,
                registered,
                spec,
            } => write!(
                f,
                "{method} {path} is guarded as {registered} but the spec says {spec}"
            ),
            Self::PublicityMismatch { method, path } => {
                write!(
                    f,
                    "{method} {path}: guard and spec disagree on whether the route is public"
                )
            }
            Self::PublicWithoutReason { method, path } => {
                write!(f, "{method} {path} is public but gives no reason")
            }
            Self::NotInSpec { method, path } => {
                write!(f, "{method} {path} is registered but not in the spec")
            }
            Self::NotRegistered { method, path } => {
                write!(f, "{method} {path} is in the spec but no route is registered")
            }
            Self::AllowanceWithoutReason { method, path } => {
                write!(f, "allowance for {method} {path} gives no reason")
            }
            Self::DuplicateRoute { method, path } => write!(f, "{method} {path} is registered twice"),
        }
    }
}

/// Reduce path parameters to `{}` so that `/swimlanes/{id}` and `/swimlanes/:sid` match.
fn normalise(path: &str) -> String {
    let segments: Vec<String> = path
        .trim_end_matches('/')
        .split('/')
        .map(|seg| {
            if (seg.starts_with('{') && seg.ends_with('}')) || seg.starts_with(':') || seg == "*" {
                "{}".to_owned()
            } else {
                seg.to_owned()
            }
        })
        .collect();
    segments.join("/")
}

type Key = (String, String);

fn key(method: &str, path: &str) -> Key {
    (method.to_uppercase(), normalise(path))
}

/// Compare the route table with the spec and return every violation (empty when all is well).
pub fn check_route_table(
    routes: &[RegisteredRoute],
    spec: &[SpecOperation],
    allow: &Allowances,
) -> Vec<Violation> {
    let mut out = Vec::new();
    let v = |method: &str, path: &str| (method.to_uppercase(), path.to_owned());

    for (m, p, reason) in allow.spec_only.iter().chain(&allow.route_only) {
        if reason.trim().is_empty() {
            let (method, path) = v(m, p);
            out.push(Violation::AllowanceWithoutReason { method, path });
        }
    }
    let spec_only: BTreeSet<Key> = allow.spec_only.iter().map(|(m, p, _)| key(m, p)).collect();
    let route_only: BTreeSet<Key> = allow.route_only.iter().map(|(m, p, _)| key(m, p)).collect();

    let spec_by_key: BTreeMap<Key, &SpecOperation> =
        spec.iter().map(|o| (key(&o.method, &o.path), o)).collect();
    let mut seen = BTreeSet::new();

    for r in routes {
        let k = key(&r.method, &r.path);
        let (method, path) = v(&r.method, &r.path);
        if !seen.insert(k.clone()) {
            out.push(Violation::DuplicateRoute {
                method: method.clone(),
                path: path.clone(),
            });
        }
        match &r.guard {
            Guard::Unguarded => out.push(Violation::Unguarded { method, path }),
            guard => match spec_by_key.get(&k) {
                None => {
                    if !route_only.contains(&k) {
                        out.push(Violation::NotInSpec { method, path });
                    }
                }
                Some(op) => match (guard, op.required) {
                    (Guard::Role(reg), SpecRole::Role(want)) => {
                        if *reg != want {
                            out.push(Violation::RoleMismatch {
                                method,
                                path,
                                registered: *reg,
                                spec: want,
                            });
                        }
                    }
                    (Guard::Public { reason }, SpecRole::None) => {
                        if reason.trim().is_empty() {
                            out.push(Violation::PublicWithoutReason { method, path });
                        }
                    }
                    _ => out.push(Violation::PublicityMismatch { method, path }),
                },
            },
        }
    }

    let registered: BTreeSet<Key> = routes.iter().map(|r| key(&r.method, &r.path)).collect();
    for o in spec {
        let k = key(&o.method, &o.path);
        if !registered.contains(&k) && !spec_only.contains(&k) {
            out.push(Violation::NotRegistered {
                method: o.method.to_uppercase(),
                path: o.path.clone(),
            });
        }
    }
    out
}

/// Panic with every violation, one per line, if the route table does not match the spec.
pub fn assert_route_table(routes: &[RegisteredRoute], spec: &[SpecOperation], allow: &Allowances) {
    let violations = check_route_table(routes, spec, allow);
    assert!(
        violations.is_empty(),
        "route table violates S4:\n{}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Check the user gate behind `RequireRole<R>` and `ActiveUser`: `allowed(user, min)` must be false for
/// pending and disabled users whatever their role, and for active users exactly when their role is below
/// `min` (or they have none). The gate runs before any handler, so a denied user never reaches one.
pub fn assert_user_gate(allowed: impl Fn(&User, Role) -> bool) {
    use super::sample::user;
    for min in Role::ALL {
        for status in [UserStatus::Pending, UserStatus::Disabled] {
            for role in Role::ALL {
                assert!(
                    !allowed(&user(1, Some(role), status), min),
                    "{status:?} user with role {role} must be denied for {min}"
                );
            }
            assert!(
                !allowed(&user(1, None, status), min),
                "{status:?} user without a role must be denied"
            );
        }
        assert!(
            !allowed(&user(1, None, UserStatus::Active), min),
            "an active user without a role must be denied for {min}"
        );
        for role in Role::ALL {
            assert_eq!(
                allowed(&user(1, Some(role), UserStatus::Active), min),
                role >= min,
                "active {role} user against a {min} route"
            );
        }
    }
}
