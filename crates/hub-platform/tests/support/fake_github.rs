//! A local stand-in for GitHub (T7): a bare repository that is the "remote" of a mirror over the file transport, with commits made
//! by `git fast-import` (no index, no working tree, no user configuration, fixed identities and increasing times), and a sender of
//! signed webhook deliveries.
//!
//! `git` is a development tool here, as in `cargo xtask gen-fixtures`: the hub itself never runs it over the network.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};

use aws_lc_rs::hmac;
use tempfile::TempDir;

const AUTHOR: &str = "Test Author <author@lanekeeper.invalid>";
/// 2025-01-01T00:00:00Z.
const EPOCH: i64 = 1_735_689_600;

fn git(dir: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    if let Some(d) = dir {
        c.arg("-C").arg(d);
    }
    c.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "author@lanekeeper.invalid")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    c
}

pub fn run(mut c: Command) -> String {
    let out = c.output().expect("git runs");
    assert!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A bare repository that plays the part of the repository on GitHub.
pub struct GitRemote {
    _dir: TempDir,
    pub path: PathBuf,
    clock: AtomicI64,
}

impl std::fmt::Debug for GitRemote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GitRemote")
    }
}

impl GitRemote {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("remote.git");
        run({
            let mut c = git(None);
            c.args([
                "init",
                "--bare",
                "--quiet",
                "--template=",
                "--initial-branch=main",
            ])
            .arg(&path);
            c
        });
        Self {
            _dir: dir,
            path,
            clock: AtomicI64::new(EPOCH),
        }
    }

    /// A copy of the bare repository at `src` (the fixture repositories of `cargo xtask gen-fixtures`), which can then be pushed to
    /// without touching the original. New commits are made a year after the fixtures' own.
    pub fn copy_of(src: &Path) -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("remote.git");
        run({
            let mut c = Command::new("cp");
            c.arg("-r").arg(src).arg(&path);
            c
        });
        Self {
            _dir: dir,
            path,
            clock: AtomicI64::new(EPOCH + 365 * 86_400),
        }
    }

    /// `file://` URL of the repository.
    pub fn url(&self) -> String {
        format!("file://{}", self.path.display())
    }

    fn rev(&self, refname: &str) -> Option<String> {
        let out = git(Some(&self.path))
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{refname}^{{commit}}"),
            ])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
    }

    /// The commit a branch points at.
    pub fn head(&self, branch: &str) -> Option<String> {
        self.rev(&format!("refs/heads/{branch}"))
    }

    /// Commit `changes` (a path and its new bytes, or `None` to delete it) on top of `branch`, creating the branch (as an empty
    /// tree plus the changes) if it does not exist. Every commit is one minute after the one before. Returns the commit id.
    pub fn commit(&self, branch: &str, changes: &[(&str, Option<&[u8]>)], message: &str) -> String {
        let time = self.clock.fetch_add(60, Ordering::SeqCst) + 60;
        let refname = format!("refs/heads/{branch}");
        let mut stream = Vec::new();
        let who = format!("{AUTHOR} {time} +0000");
        writeln!(stream, "commit {refname}").unwrap();
        writeln!(stream, "author {who}").unwrap();
        writeln!(stream, "committer {who}").unwrap();
        writeln!(stream, "data {}", message.len()).unwrap();
        stream.extend_from_slice(message.as_bytes());
        writeln!(stream).unwrap();
        if self.rev(&refname).is_some() {
            writeln!(stream, "from {refname}^0").unwrap();
        }
        for (path, content) in changes {
            match content {
                Some(bytes) => {
                    writeln!(stream, "M 100644 inline {path}").unwrap();
                    writeln!(stream, "data {}", bytes.len()).unwrap();
                    stream.extend_from_slice(bytes);
                    writeln!(stream).unwrap();
                }
                None => writeln!(stream, "D {path}").unwrap(),
            }
        }
        writeln!(stream).unwrap();
        writeln!(stream, "done").unwrap();
        let mut child = git(Some(&self.path))
            .args(["fast-import", "--quiet", "--done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&stream).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "fast-import failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.head(branch).expect("the branch exists after a commit")
    }

    /// Like [`GitRemote::commit`], with an explicit file mode per path (`100644`, `100755`, `120000` for a symlink whose content
    /// is its target) and `None` to delete.
    pub fn commit_modes(
        &self,
        branch: &str,
        changes: &[(&str, &str, Option<&[u8]>)],
        message: &str,
    ) -> String {
        let time = self.clock.fetch_add(60, Ordering::SeqCst) + 60;
        let refname = format!("refs/heads/{branch}");
        let mut stream = Vec::new();
        let who = format!("{AUTHOR} {time} +0000");
        writeln!(stream, "commit {refname}").unwrap();
        writeln!(stream, "author {who}").unwrap();
        writeln!(stream, "committer {who}").unwrap();
        writeln!(stream, "data {}", message.len()).unwrap();
        stream.extend_from_slice(message.as_bytes());
        writeln!(stream).unwrap();
        if self.rev(&refname).is_some() {
            writeln!(stream, "from {refname}^0").unwrap();
        }
        for (mode, path, content) in changes {
            match content {
                Some(bytes) => {
                    writeln!(stream, "M {mode} inline {path}").unwrap();
                    writeln!(stream, "data {}", bytes.len()).unwrap();
                    stream.extend_from_slice(bytes);
                    writeln!(stream).unwrap();
                }
                None => writeln!(stream, "D {path}").unwrap(),
            }
        }
        writeln!(stream).unwrap();
        writeln!(stream, "done").unwrap();
        let mut child = git(Some(&self.path))
            .args(["fast-import", "--quiet", "--done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&stream).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "fast-import failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        self.head(branch).expect("the branch exists after a commit")
    }

    /// A lightweight tag.
    pub fn tag(&self, name: &str, commit: &str) {
        run({
            let mut c = git(Some(&self.path));
            c.args(["tag", name, commit]);
            c
        });
    }

    /// An annotated tag (an object of its own that points at the commit).
    pub fn annotated_tag(&self, name: &str, commit: &str) {
        run({
            let mut c = git(Some(&self.path));
            c.args(["tag", "-a", "-m", "release", name, commit]);
            c
        });
    }

    pub fn delete_branch(&self, branch: &str) {
        run({
            let mut c = git(Some(&self.path));
            c.args(["update-ref", "-d", &format!("refs/heads/{branch}")]);
            c
        });
    }

    /// Rewrite a branch to an unrelated commit (a forced push).
    pub fn force_branch(&self, branch: &str, commit: &str) {
        run({
            let mut c = git(Some(&self.path));
            c.args(["update-ref", &format!("refs/heads/{branch}"), commit]);
            c
        });
    }
}

/// `sha256=<hex>` over `body`, as GitHub signs a delivery.
pub fn signature(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    format!("sha256={}", hex::encode(hmac::sign(&key, body).as_ref()))
}

/// The JSON body of a `push` event.
pub fn push_body(repo: &str, git_ref: &str, before: &str, after: &str) -> Vec<u8> {
    serde_json::json!({
        "ref": git_ref,
        "before": before,
        "after": after,
        "deleted": after.bytes().all(|c| c == b'0'),
        "repository": {"full_name": repo, "private": true},
        "pusher": {"name": "someone"},
        "commits": [],
    })
    .to_string()
    .into_bytes()
}

/// A signed webhook delivery: the headers GitHub sends and the body.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub event: String,
    pub id: String,
    pub signature: String,
    pub body: Vec<u8>,
}

impl Delivery {
    pub fn signed(secret: &str, event: &str, delivery_id: &str, body: Vec<u8>) -> Self {
        Self {
            event: event.to_owned(),
            id: delivery_id.to_owned(),
            signature: signature(secret, &body),
            body,
        }
    }

    pub fn request(&self) -> http::Request<axum::body::Body> {
        http::Request::builder()
            .method("POST")
            .uri("/hooks/github")
            .header("content-type", "application/json")
            .header("x-github-event", &self.event)
            .header("x-github-delivery", &self.id)
            .header("x-hub-signature-256", &self.signature)
            .body(axum::body::Body::from(self.body.clone()))
            .unwrap()
    }
}
