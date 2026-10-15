//! A small in-memory GitHub for the pull-request path (T6): branches, commits, trees and blobs, the contents of a file at a commit,
//! and pull requests, behind the same loopback server the client tests use (`fake_github::LocalGithub`).
//!
//! It answers the calls of the Git Data API that the hub makes, with the shapes GitHub documents (a base64 body with a line break
//! every 60 characters, `201` on a create, `422` for a ref that exists), keeps what was created, and checks the token of every call,
//! so a test can say what a pull request left in the repository and which token did it. It checks the tree it is given as GitHub
//! does not: a tree entry has to be a regular file whose blob exists.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::fake_github::{Handler, Reply, Seen};

#[derive(Clone)]
struct SimCommit {
    tree: String,
    parents: Vec<String>,
    message: String,
}

/// A pull request the simulator holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimPull {
    pub number: u64,
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub url: String,
}

#[derive(Default)]
struct Repo {
    branches: BTreeMap<String, String>,
    commits: BTreeMap<String, SimCommit>,
    trees: BTreeMap<String, BTreeMap<String, String>>,
    blobs: BTreeMap<String, Vec<u8>>,
    pulls: Vec<SimPull>,
}

type Fault = Box<dyn FnMut(&Seen) -> Option<Reply> + Send>;

#[derive(Default)]
struct State {
    repos: BTreeMap<String, Repo>,
    tokens: Vec<String>,
    faults: Vec<Fault>,
    /// The contents call leaves the bytes out of a file longer than this (GitHub does over 1 MB).
    none_over: Option<usize>,
    counter: u64,
}

/// The simulator. Clone it freely: the clones share one state.
#[derive(Clone, Default)]
pub struct GitSim(Arc<Mutex<State>>);

fn sha(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_be_bytes());
        h.update(p);
    }
    let mut hex = String::new();
    for b in h.finalize().iter().take(20) {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn json(status: u16, v: &serde_json::Value) -> Reply {
    Reply::json(status, &v.to_string())
}

fn not_found() -> Reply {
    json(404, &serde_json::json!({ "message": "Not Found" }))
}

fn invalid(why: &str) -> Reply {
    json(
        422,
        &serde_json::json!({ "message": "Validation Failed", "errors": [why] }),
    )
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn wrapped(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in base64::engine::general_purpose::STANDARD
        .encode(bytes)
        .as_bytes()
        .chunks(60)
    {
        out.push_str(&String::from_utf8_lossy(c));
        out.push('\n');
    }
    out
}

impl GitSim {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every call has to carry `Bearer <token>` for one of the tokens given here; any other gets 401. With none given, no call is
    /// checked.
    pub fn expect_token(&self, token: &str) {
        self.state().tokens.push(token.to_owned());
    }

    /// A fault hook, run first on every request: `Some(reply)` answers instead of the simulator.
    pub fn inject(&self, f: impl FnMut(&Seen) -> Option<Reply> + Send + 'static) {
        self.state().faults.push(Box::new(f));
    }

    /// Contents answers leave out the bytes of a file longer than `n` (as GitHub does over 1 MB).
    pub fn contents_without_bytes_over(&self, n: usize) {
        self.state().none_over = Some(n);
    }

    /// `repo` (`owner/name`) has `branch` with these files at its head. Returns the commit.
    pub fn branch(&self, repo: &str, branch: &str, files: &[(&str, &[u8])]) -> String {
        let mut st = self.state();
        st.counter += 1;
        let n = st.counter;
        let r = st.repos.entry(repo.to_owned()).or_default();
        let mut tree = BTreeMap::new();
        for (path, bytes) in files {
            let blob = sha(&[b"blob", bytes]);
            r.blobs.insert(blob.clone(), bytes.to_vec());
            tree.insert((*path).to_owned(), blob);
        }
        let tree_sha = tree_id(&tree);
        r.trees.insert(tree_sha.clone(), tree);
        let commit = sha(&[
            b"commit",
            tree_sha.as_bytes(),
            branch.as_bytes(),
            &n.to_be_bytes(),
        ]);
        r.commits.insert(
            commit.clone(),
            SimCommit {
                tree: tree_sha,
                parents: Vec::new(),
                message: "initial".to_owned(),
            },
        );
        r.branches.insert(branch.to_owned(), commit.clone());
        commit
    }

    /// The commit at the head of a branch.
    pub fn head(&self, repo: &str, branch: &str) -> Option<String> {
        self.state().repos.get(repo)?.branches.get(branch).cloned()
    }

    /// The branches of a repository, ordered.
    pub fn branches(&self, repo: &str) -> Vec<String> {
        self.state()
            .repos
            .get(repo)
            .map(|r| r.branches.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// A file at the head of a branch.
    pub fn file(&self, repo: &str, branch: &str, path: &str) -> Option<Vec<u8>> {
        let st = self.state();
        let r = st.repos.get(repo)?;
        let commit = r.commits.get(r.branches.get(branch)?)?;
        r.blobs.get(r.trees.get(&commit.tree)?.get(path)?).cloned()
    }

    /// Every path in the tree at the head of a branch.
    pub fn paths(&self, repo: &str, branch: &str) -> Vec<String> {
        let st = self.state();
        let Some(r) = st.repos.get(repo) else {
            return Vec::new();
        };
        r.branches
            .get(branch)
            .and_then(|c| r.commits.get(c))
            .and_then(|c| r.trees.get(&c.tree))
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The parents and the message of a branch's head commit.
    pub fn head_commit(&self, repo: &str, branch: &str) -> Option<(Vec<String>, String)> {
        let st = self.state();
        let r = st.repos.get(repo)?;
        let c = r.commits.get(r.branches.get(branch)?)?;
        Some((c.parents.clone(), c.message.clone()))
    }

    pub fn pulls(&self, repo: &str) -> Vec<SimPull> {
        self.state()
            .repos
            .get(repo)
            .map(|r| r.pulls.clone())
            .unwrap_or_default()
    }

    /// How many blobs, trees and commits the repository holds (so a test can say a call made none).
    pub fn object_count(&self, repo: &str) -> usize {
        self.state()
            .repos
            .get(repo)
            .map_or(0, |r| r.blobs.len() + r.trees.len() + r.commits.len())
    }

    /// The handler to give `LocalGithub::start`.
    pub fn handler(&self) -> Handler {
        let sim = self.clone();
        Arc::new(move |seen, _| sim.answer(seen))
    }

    fn answer(&self, seen: &Seen) -> Reply {
        let mut st = self.state();
        for fault in &mut st.faults {
            if let Some(reply) = fault(seen) {
                return reply;
            }
        }
        if !st.tokens.is_empty()
            && !st
                .tokens
                .iter()
                .any(|t| seen.header("authorization") == Some(format!("Bearer {t}").as_str()))
        {
            return json(401, &serde_json::json!({ "message": "Bad credentials" }));
        }
        let (path, query) = seen.path.split_once('?').unwrap_or((seen.path.as_str(), ""));
        let Some(rest) = path.strip_prefix("/repos/") else {
            return not_found();
        };
        let mut parts = rest.splitn(3, '/');
        let (Some(owner), Some(name), Some(tail)) = (parts.next(), parts.next(), parts.next()) else {
            return not_found();
        };
        let key = format!("{owner}/{name}");
        st.counter += 1;
        let counter = st.counter;
        let none_over = st.none_over;
        let Some(repo) = st.repos.get_mut(&key) else {
            return not_found();
        };
        route(repo, &key, seen, tail, query, counter, none_over)
    }
}

fn tree_id(tree: &BTreeMap<String, String>) -> String {
    let mut parts: Vec<Vec<u8>> = vec![b"tree".to_vec()];
    for (p, b) in tree {
        parts.push(p.as_bytes().to_vec());
        parts.push(b.as_bytes().to_vec());
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    sha(&refs)
}

#[allow(clippy::too_many_lines)]
fn route(
    repo: &mut Repo,
    key: &str,
    seen: &Seen,
    tail: &str,
    query: &str,
    counter: u64,
    none_over: Option<usize>,
) -> Reply {
    let body = seen.json();
    let field = |name: &str| {
        body.get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    match (seen.method.as_str(), tail) {
        ("GET", t) if t.starts_with("git/ref/heads/") => {
            let branch = unescape(&t["git/ref/heads/".len()..]);
            match repo.branches.get(&branch) {
                Some(c) => json(
                    200,
                    &serde_json::json!({ "ref": format!("refs/heads/{branch}"), "object": { "sha": c, "type": "commit" } }),
                ),
                None => not_found(),
            }
        }
        ("GET", t) if t.starts_with("git/commits/") => match repo.commits.get(&t["git/commits/".len()..]) {
            Some(c) => json(
                200,
                &serde_json::json!({ "sha": &t["git/commits/".len()..], "tree": { "sha": c.tree }, "message": c.message }),
            ),
            None => not_found(),
        },
        ("GET", t) if t.starts_with("git/blobs/") => match repo.blobs.get(&t["git/blobs/".len()..]) {
            Some(b) => json(
                200,
                &serde_json::json!({ "sha": &t["git/blobs/".len()..], "size": b.len(), "encoding": "base64", "content": wrapped(b) }),
            ),
            None => not_found(),
        },
        ("GET", t) if t.starts_with("contents/") => {
            let path = unescape(&t["contents/".len()..]);
            let at = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("ref="))
                .unwrap_or_default();
            let found = repo
                .commits
                .get(at)
                .and_then(|c| repo.trees.get(&c.tree))
                .and_then(|tree| tree.get(&path));
            match found.and_then(|blob| repo.blobs.get(blob).map(|b| (blob, b))) {
                Some((blob, bytes)) => {
                    if none_over.is_some_and(|n| bytes.len() > n) {
                        json(
                            200,
                            &serde_json::json!({ "type": "file", "encoding": "none", "size": bytes.len(), "sha": blob, "content": "" }),
                        )
                    } else {
                        json(
                            200,
                            &serde_json::json!({ "type": "file", "encoding": "base64", "size": bytes.len(), "sha": blob, "content": wrapped(bytes) }),
                        )
                    }
                }
                None => not_found(),
            }
        }
        ("POST", "git/blobs") => {
            if field("encoding").as_deref() != Some("base64") {
                return invalid("encoding");
            }
            let Some(bytes) =
                field("content").and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok())
            else {
                return invalid("content");
            };
            let id = sha(&[b"blob", &bytes]);
            repo.blobs.insert(id.clone(), bytes);
            json(201, &serde_json::json!({ "sha": id }))
        }
        ("POST", "git/trees") => {
            let Some(base) = field("base_tree").and_then(|b| repo.trees.get(&b).cloned()) else {
                return invalid("base_tree");
            };
            let mut tree = base;
            for entry in body
                .get("tree")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default()
            {
                let get = |n: &str| {
                    entry
                        .get(n)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                };
                let (Some(path), Some(blob)) = (get("path"), get("sha")) else {
                    return invalid("entry");
                };
                if get("mode").as_deref() != Some("100644")
                    || get("type").as_deref() != Some("blob")
                    || !repo.blobs.contains_key(&blob)
                {
                    return invalid("entry");
                }
                tree.insert(path, blob);
            }
            let id = tree_id(&tree);
            repo.trees.insert(id.clone(), tree);
            json(201, &serde_json::json!({ "sha": id }))
        }
        ("POST", "git/commits") => {
            let (Some(message), Some(tree)) = (field("message"), field("tree")) else {
                return invalid("commit");
            };
            let parents: Vec<String> = body
                .get("parents")
                .and_then(|p| p.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            if !repo.trees.contains_key(&tree) || parents.iter().any(|p| !repo.commits.contains_key(p)) {
                return invalid("tree or parents");
            }
            // A real commit carries a time; two commits of the same tree are two commits.
            let id = sha(&[
                b"commit",
                tree.as_bytes(),
                message.as_bytes(),
                &counter.to_be_bytes(),
            ]);
            repo.commits.insert(
                id.clone(),
                SimCommit {
                    tree: tree.clone(),
                    parents,
                    message,
                },
            );
            json(201, &serde_json::json!({ "sha": id, "tree": { "sha": tree } }))
        }
        ("POST", "git/refs") => {
            let (Some(full), Some(commit)) = (field("ref"), field("sha")) else {
                return invalid("ref");
            };
            let Some(branch) = full.strip_prefix("refs/heads/") else {
                return invalid("ref");
            };
            if repo.branches.contains_key(branch) {
                return json(422, &serde_json::json!({ "message": "Reference already exists" }));
            }
            if !repo.commits.contains_key(&commit) {
                return invalid("sha");
            }
            repo.branches.insert(branch.to_owned(), commit.clone());
            json(
                201,
                &serde_json::json!({ "ref": full, "object": { "sha": commit, "type": "commit" } }),
            )
        }
        ("POST", "pulls") => {
            let (Some(head), Some(base), Some(title)) = (field("head"), field("base"), field("title")) else {
                return invalid("pull");
            };
            if !repo.branches.contains_key(&head) || !repo.branches.contains_key(&base) {
                return invalid("head or base");
            }
            if repo.pulls.iter().any(|p| p.head == head) {
                return json(
                    422,
                    &serde_json::json!({ "message": "Validation Failed", "errors": [{ "message": "A pull request already exists" }] }),
                );
            }
            let number = repo.pulls.len() as u64 + 1;
            let url = format!("https://github.com/{key}/pull/{number}");
            repo.pulls.push(SimPull {
                number,
                head,
                base,
                title,
                body: field("body").unwrap_or_default(),
                url: url.clone(),
            });
            json(
                201,
                &serde_json::json!({ "number": number, "html_url": url, "state": "open" }),
            )
        }
        ("GET", "pulls") => {
            let head = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("head="))
                .map(unescape)
                .and_then(|h| h.split_once(':').map(|(_, b)| b.to_owned()));
            let list: Vec<serde_json::Value> = repo
                .pulls
                .iter()
                .filter(|p| head.as_deref().is_none_or(|h| p.head == h))
                .map(|p| serde_json::json!({ "number": p.number, "html_url": p.url, "head": { "ref": p.head } }))
                .collect();
            json(200, &serde_json::Value::Array(list))
        }
        ("PATCH", t) if t.starts_with("pulls/") => {
            let Ok(n) = t["pulls/".len()..].parse::<u64>() else {
                return not_found();
            };
            match repo.pulls.iter_mut().find(|p| p.number == n) {
                Some(p) => {
                    if let Some(b) = field("body") {
                        p.body = b;
                    }
                    json(200, &serde_json::json!({ "number": p.number, "html_url": p.url }))
                }
                None => not_found(),
            }
        }
        _ => not_found(),
    }
}
