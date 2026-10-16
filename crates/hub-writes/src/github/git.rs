//! The Git Data calls of the pull-request path (T6, D43): read a branch and a file, make blobs, a tree and a commit, open a branch,
//! open a pull request.
//!
//! Every call takes the user's own token (D41) as a [`Secret`] and keeps no copy. The commit goes through the Git Data API and not
//! the Contents API, so that the bytes of a file are exactly the ones sent: a blob is sent as base64, which cannot change a line
//! ending, a byte order mark or an encoding (rule 11, D17).
//!
//! Which calls are repeated after a failure whose outcome is unknown:
//!
//! - **Reads, blobs, trees and commits are.** A blob, a tree and a commit are content-addressed objects that nothing points at
//!   until a branch does, so making one twice leaves the repository as making it once did.
//! - **A branch, a pull request and an edit of one are not.** A second try after a lost answer could meet its own first try (a
//!   branch that already exists, a pull request that already exists). The caller finds out which: [`GitDataApi::branch_head`]
//!   and [`GitDataApi::find_pull`] are the questions to ask. A rate limit is waited out for every call, because GitHub has done
//!   nothing yet.

use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use domain::{RepoPath, Secret};
use serde_json::json;

use super::api::{
    GithubError, MAX_CONTENT_BODY_BYTES, NodeKind, PullInfo, TreeNode, parse_blob, parse_commit_tree,
    parse_created_sha, parse_pull, parse_pull_list, parse_ref_commit, parse_tree,
};
use super::client::{Call, GithubClient, Retry};
use super::types::{BranchName, GitSha, RepoRef};

/// A file as the repository has it at a commit.
#[derive(Clone, PartialEq, Eq)]
pub enum RepoFile {
    /// The path is not there at that commit.
    Absent,
    /// A regular file: the blob it is, and its bytes.
    File { blob: GitSha, bytes: Bytes },
}

impl std::fmt::Debug for RepoFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The content of a repository file is configuration: the size, never the text.
        match self {
            Self::Absent => f.write_str("Absent"),
            Self::File { blob, bytes } => f
                .debug_struct("File")
                .field("blob", blob)
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

/// The directory listings one plan has read, by the tree they are of, so that the files of one directory cost one listing. A listing
/// is of a tree id and so cannot go stale, but it is kept for one plan only and for the user whose token fetched it: it is a value
/// of the call that makes it, not a cache of the client.
#[derive(Debug, Default)]
pub struct TreeCache {
    listings: std::collections::HashMap<String, Vec<TreeNode>>,
}

/// One file of the tree a commit is built on: the path, and the blob that becomes its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: RepoPath,
    pub blob: GitSha,
}

/// The pull request to open: from a branch of the same repository to another.
#[derive(Debug, Clone)]
pub struct NewPull<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub head: &'a BranchName,
    pub base: &'a BranchName,
}

/// What the pull-request path asks of GitHub. [`GithubClient`] implements it; the tests use it with a local server.
#[async_trait]
pub trait GitDataApi: Send + Sync + 'static {
    /// The commit at the head of `branch`; `None` when there is no such branch.
    async fn branch_head(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
    ) -> Result<Option<GitSha>, GithubError>;

    /// The tree of a commit.
    async fn commit_tree(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        commit: &GitSha,
    ) -> Result<GitSha, GithubError>;

    /// The file at `path` in `tree` (the tree of a commit, not of a branch, which may move while the plan is made). It is read
    /// through the tree's own listings and not the contents call, which follows a symbolic link and answers with the file it points
    /// at: a link is [`GithubError::Unexpected`], like a directory or a submodule, and so is a name that is a file where a directory
    /// is needed. A path that is not there is [`RepoFile::Absent`]. `cache` holds the listings of this plan.
    async fn file_at(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        tree: &GitSha,
        path: &RepoPath,
        cache: &mut TreeCache,
    ) -> Result<RepoFile, GithubError>;

    /// A blob holding exactly `content`.
    async fn create_blob(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        content: &Bytes,
    ) -> Result<GitSha, GithubError>;

    /// A tree that is `base_tree` with each entry's path set to its blob (a regular, non-executable file).
    async fn create_tree(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        base_tree: &GitSha,
        entries: &[TreeEntry],
    ) -> Result<GitSha, GithubError>;

    /// A commit of `tree` on top of `parent`.
    async fn create_commit(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        message: &str,
        tree: &GitSha,
        parent: &GitSha,
    ) -> Result<GitSha, GithubError>;

    /// A new branch at `commit`. [`GithubError::Unprocessable`] when the branch exists.
    async fn create_branch(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
        commit: &GitSha,
    ) -> Result<(), GithubError>;

    /// Open a pull request. [`GithubError::Unprocessable`] when one is open for the branch already.
    async fn open_pull(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        pull: &NewPull<'_>,
    ) -> Result<PullInfo, GithubError>;

    /// The pull request, open or not, whose head is `branch` of the repository.
    async fn find_pull(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
    ) -> Result<Option<PullInfo>, GithubError>;

    /// Replace the text of a pull request.
    async fn update_pull_body(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        number: u32,
        body: &str,
    ) -> Result<(), GithubError>;
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[async_trait]
impl GitDataApi for GithubClient {
    async fn branch_head(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
    ) -> Result<Option<GitSha>, GithubError> {
        let path = format!("{}/git/ref/heads/{branch}", repo.api_path());
        match self.send(token, &Call::get(&path, self.cfg.max_body)).await {
            Ok(reply) => parse_ref_commit(&reply.body).map(Some),
            Err(GithubError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn commit_tree(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        commit: &GitSha,
    ) -> Result<GitSha, GithubError> {
        let path = format!("{}/git/commits/{commit}", repo.api_path());
        let reply = self.send(token, &Call::get(&path, self.cfg.max_body)).await?;
        parse_commit_tree(&reply.body)
    }

    async fn file_at(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        tree: &GitSha,
        path: &RepoPath,
        cache: &mut TreeCache,
    ) -> Result<RepoFile, GithubError> {
        let mut dir = tree.clone();
        let mut names = path.as_str().split('/').peekable();
        let node = loop {
            let Some(name) = names.next() else {
                return Err(GithubError::Unexpected);
            };
            if !cache.listings.contains_key(dir.as_str()) {
                let url = format!("{}/git/trees/{dir}", repo.api_path());
                let reply = self.send(token, &Call::get(&url, MAX_CONTENT_BODY_BYTES)).await?;
                cache
                    .listings
                    .insert(dir.as_str().to_owned(), parse_tree(&reply.body)?);
            }
            let listing = cache.listings.get(dir.as_str()).map_or(&[][..], Vec::as_slice);
            let Some(found) = listing.iter().find(|n| n.name == name) else {
                // Not there: a listing is the whole directory.
                return Ok(RepoFile::Absent);
            };
            if names.peek().is_none() {
                break found.clone();
            }
            // On the way to the file: only a directory can hold it.
            if found.kind != NodeKind::Tree {
                return Err(GithubError::Unexpected);
            }
            dir = found.sha.clone();
        };
        if node.kind != NodeKind::File {
            return Err(GithubError::Unexpected);
        }
        let url = format!("{}/git/blobs/{}", repo.api_path(), node.sha);
        let reply = self.send(token, &Call::get(&url, MAX_CONTENT_BODY_BYTES)).await?;
        Ok(RepoFile::File {
            blob: node.sha,
            bytes: parse_blob(&reply.body)?,
        })
    }

    async fn create_blob(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        content: &Bytes,
    ) -> Result<GitSha, GithubError> {
        let path = format!("{}/git/blobs", repo.api_path());
        let body = json!({ "content": b64(content), "encoding": "base64" });
        let reply = self
            .send(token, &Call::post(&path, &body, Retry::Transient))
            .await?;
        parse_created_sha(&reply.body)
    }

    async fn create_tree(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        base_tree: &GitSha,
        entries: &[TreeEntry],
    ) -> Result<GitSha, GithubError> {
        let path = format!("{}/git/trees", repo.api_path());
        let tree: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| json!({ "path": e.path.as_str(), "mode": "100644", "type": "blob", "sha": e.blob.as_str() }))
            .collect();
        let body = json!({ "base_tree": base_tree.as_str(), "tree": tree });
        let reply = self
            .send(token, &Call::post(&path, &body, Retry::Transient))
            .await?;
        parse_created_sha(&reply.body)
    }

    async fn create_commit(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        message: &str,
        tree: &GitSha,
        parent: &GitSha,
    ) -> Result<GitSha, GithubError> {
        let path = format!("{}/git/commits", repo.api_path());
        let body = json!({ "message": message, "tree": tree.as_str(), "parents": [parent.as_str()] });
        let reply = self
            .send(token, &Call::post(&path, &body, Retry::Transient))
            .await?;
        parse_created_sha(&reply.body)
    }

    async fn create_branch(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
        commit: &GitSha,
    ) -> Result<(), GithubError> {
        let path = format!("{}/git/refs", repo.api_path());
        let body = json!({ "ref": format!("refs/heads/{branch}"), "sha": commit.as_str() });
        let reply = self.send(token, &Call::post(&path, &body, Retry::Once)).await?;
        // The answer names the commit the ref now points at: it must be the one asked for.
        if parse_ref_commit(&reply.body)? == *commit {
            Ok(())
        } else {
            Err(GithubError::Unexpected)
        }
    }

    async fn open_pull(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        pull: &NewPull<'_>,
    ) -> Result<PullInfo, GithubError> {
        let path = format!("{}/pulls", repo.api_path());
        let body = json!({
            "title": pull.title,
            "body": pull.body,
            "head": pull.head.as_str(),
            "base": pull.base.as_str(),
            "maintainer_can_modify": false,
            "draft": false,
        });
        let reply = self.send(token, &Call::post(&path, &body, Retry::Once)).await?;
        parse_pull(&reply.body)
    }

    async fn find_pull(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        branch: &BranchName,
    ) -> Result<Option<PullInfo>, GithubError> {
        // `head` is `owner:branch`; both parts are restricted to URL-safe characters.
        let path = format!(
            "{}/pulls?head={}:{branch}&state=all&per_page=5",
            repo.api_path(),
            repo.owner()
        );
        let reply = self.send(token, &Call::get(&path, self.cfg.max_body)).await?;
        parse_pull_list(&reply.body, branch)
    }

    async fn update_pull_body(
        &self,
        token: &Secret<String>,
        repo: &RepoRef,
        number: u32,
        body: &str,
    ) -> Result<(), GithubError> {
        let path = format!("{}/pulls/{number}", repo.api_path());
        let body = json!({ "body": body });
        let reply = self.send(token, &Call::patch(&path, &body)).await?;
        parse_pull(&reply.body).map(|_| ())
    }
}
