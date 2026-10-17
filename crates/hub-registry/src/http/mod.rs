//! The REST edge of the read API (T6, S4, S11, S12, S21): the fifteen read operations of `api/openapi.yaml`.
//!
//! - [`query`]: typed, bounded parsing of every query string. A parameter that is not in the operation's list, a parameter given twice, a
//!   value that does not parse, a list over its bound: a 400 that names nothing of the input.
//! - [`dto`]: the response bodies where the domain type is not already the contract's (the content of a file as text, a setting change
//!   with its severity, the swimlane detail, the file as the config-server serves it).
//! - [`routes`]: [`routes::registry_routes`], the handlers, registered through hub-platform's `GuardedRouter` at Viewer, and
//!   [`routes::registry_route_specs`], the table the route-guard harness checks against the spec.

pub mod dto;
pub mod query;
pub mod routes;

pub use routes::{registry_route_specs, registry_routes};
