//! The GitHub leg of the write path (T5, T6, plan A9): typed values, the parsers of GitHub's answers, and the client.
//!
//! T5 uses it to check a token before it is stored: who it belongs to, when it expires and whether it can push to both
//! repositories. T6 adds the Git Data calls of the pull-request path ([`git`]) to the same client.

pub mod api;
pub mod client;
pub mod git;
pub mod types;

pub use api::{GithubError, MAX_BODY_BYTES, PullInfo};
pub use client::{GithubApi, GithubClient, GithubClientConfig, GithubUser};
pub use git::{GitDataApi, NewPull, RepoFile, TreeCache, TreeEntry};
pub use types::{
    BranchName, FINE_GRAINED_PREFIX, GitSha, GithubLogin, MAX_TOKEN_LEN, NameError, RepoRef, TokenShapeError,
};
