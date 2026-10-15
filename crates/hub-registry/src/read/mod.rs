//! The read API (T6, P7, P12, S4, S10, S11): `RegistryService`, the `RegistryRead` port over Postgres, and the hub-registry-only
//! superset (`RegistryApi`) that the REST handlers need for the parameters the spec has and the port lacks (plan A5).
//!
//! # What a read does
//!
//! - **Authorizes first.** Every method begins with [`authz::require_viewer`] on the user's own record, before any parameter is
//!   looked at (S4). REST adds the route guard in front; MCP (07) reaches the port with no router.
//! - **Reads facts, never computes them** (D33). Lists are `file_state`, `findings`, `file_observations`, `settings_index`,
//!   `effective_files` and `effective_tree_hashes`, each through an index; the engine does the rest (effective config, diff, grid,
//!   text search) on content that is read once and cached by its hash.
//! - **Is bounded.** Every list is a keyset page of at most 500 (never an OFFSET), every query has a LIMIT, every caller-supplied path
//!   is a parsed `NfsPath` or `LogicalFile`, every cache is weighed in bytes, every database call has a timeout, and a text search
//!   has a deadline (rule 5).
//! - **Logs ids, never content** (S10): a read writes nothing but a counter, and the errors that reach a caller are the closed
//!   [`ports::ReadError`] set; the cause goes to the log as a fixed word.
//!
//! # Modules
//!
//! - [`cursor`]: keyset cursors bound to their query (A31). [`etag`]: tags of the content reads. [`authz`]: the Viewer check.
//! - [`swimlanes`], [`tree`], [`history`], [`findings`], [`restarts`], [`content`]: plain reads of the registry tables.
//! - [`settings`]: the setting rows of a blob (from `settings_index`, on demand for a blob that is not indexed yet), cached by hash.
//! - [`effective`], [`grid`], [`compare`], [`compare_tree`]: the engine's views over those rows.
//! - [`search_settings`], [`search_text`]: indexed SQL, and the engine's text search over a cached corpus.
//! - [`served`]: the config-server's own answer, through the agent.
//! - [`cache`]: the weighed moka caches. [`api`]: the superset trait and the service.

pub mod api;
pub(crate) mod array_bin;
pub mod authz;
pub mod cache;
pub mod compare;
pub mod compare_tree;
pub mod content;
pub mod corpora;
pub mod cursor;
pub mod effective;
pub mod etag;
pub mod findings;
pub mod grid;
pub mod history;
pub mod resource;
pub mod restarts;
pub mod rows;
pub mod search_settings;
pub mod search_text;
pub mod served;
pub mod service;
pub mod settings;
pub mod swimlanes;
pub mod tree;
pub mod warmup;

pub use api::{Named, RegistryApi, Tagged};
pub use service::{ReadDeps, RegistryService};
