//! Entra sign-in, sessions, users and roles, CSRF, default-deny authorization, hash-chained audit.
//!
//! Owned by prompt 03a. Read `AGENTS.md` and `prompts/03a*.md` before changing this crate.
//!
//! T1 (this module set): Entra sign-in as a backend-for-frontend (S1, S7).
//!
//! - [`oidc::login`] runs the authorization-code flow with PKCE against the single-tenant v2 endpoint. Login state
//!   lives in a sealed cookie ([`oidc::login::LoginSealer`]), so any replica can finish a login another began.
//! - [`oidc::jwt`] verifies RS256 ID tokens: header and size checks before any parsing, an algorithm allowlist, the
//!   signature against the cached key set ([`oidc::jwks`]), then issuer, audience, tenant, nonce and the time claims.
//! - [`oidc::client_auth`] authenticates the hub to Entra: a federated assertion read from the projected
//!   service-account token on every exchange (Q25), or a client secret from `SecretSource`.
//! - [`http`] is the one outbound HTTPS client: TLS 1.3 only, a host allowlist, a timeout and a body cap on every call.
//!
//! T2: users, roles and access requests (S1, S4).
//!
//! - [`users::PgUsers`] is the `Users` port over Postgres and records the first sign-in: a new user is `pending` with an
//!   access request, an event, an audit record and a notification, unless their object id is a configured bootstrap admin.
//! - [`users::UserAdmin`] approves, rejects and changes users under [`users::rules`] (the last active admin stays, nobody
//!   changes their own role), ends the sessions of a user whose role or status changed, and lists with keyset cursors.
//! - [`users::onboarding`] answers `needs_github_token` for `/me` without reading a ciphertext; [`notify`] tells the admins.
//!
//! Nothing here is public beyond what the later tasks and the hub binary need, and no error carries input text (S10, S21).
#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod clock;
pub mod config;
pub mod error;
pub mod http;
pub mod notify;
pub mod oidc;
pub mod users;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::{ClientAuth, ConfigError, EntraConfig, IdentityConfig};
pub use error::{DiscoveryError, IdentityError, TokenError};
