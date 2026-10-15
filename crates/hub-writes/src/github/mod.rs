//! The GitHub leg of the write path (T5, plan A9): typed values, the parsers of GitHub's answers, and the client.
//!
//! T5 uses it to check a token before it is stored: who it belongs to, when it expires and whether it can push to both
//! repositories. The Git Data calls of the pull-request path (T6) are added to the same client.

pub mod api;
pub mod client;
pub mod types;

pub use api::{GithubError, MAX_BODY_BYTES};
pub use client::{GithubApi, GithubClient, GithubClientConfig, GithubUser};
pub use types::{FINE_GRAINED_PREFIX, GithubLogin, MAX_TOKEN_LEN, NameError, RepoRef, TokenShapeError};
