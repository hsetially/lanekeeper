//! Real Git repositories, built with `git fast-import` (plan Q20).
//!
//! `fast-import` writes objects and refs directly: there is no index and no working tree, the identities and dates are
//! the ones in the stream, and no hook or user configuration runs. So the same stream gives the same blob, tree and
//! commit ids on every machine, and usually the same packfile bytes too on the same `git`. `git` is a development tool
//! here (it is not a product dependency).

use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use super::FixtureError;

const AUTHOR: &str = "Lanekeeper Fixtures <fixtures@lanekeeper.invalid>";

fn git_command(repo: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    if let Some(r) = repo {
        c.arg("-C").arg(r);
    }
    // Nothing from the machine's Git configuration may leak into the repositories.
    c.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    c
}

fn git_error(what: &str, e: &std::io::Error) -> FixtureError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FixtureError::Git("`git` was not found on PATH; gen-fixtures needs it (a development tool, not a product dependency)".to_owned())
    } else {
        FixtureError::Git(format!("{what}: {e}"))
    }
}

/// A bare repository being filled by one `git fast-import` process.
pub struct Import {
    child: Child,
    out: BufWriter<ChildStdin>,
}

impl std::fmt::Debug for Import {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Import")
    }
}

impl Import {
    /// Creates the bare repository at `repo` and starts `fast-import` in it.
    ///
    /// # Errors
    /// When `git` is missing or refuses.
    pub fn init(repo: &Path, initial_branch: &str) -> Result<Self, FixtureError> {
        std::fs::create_dir_all(repo)?;
        let status = git_command(None)
            .args([
                "init",
                "--bare",
                "--quiet",
                "--template=",
                &format!("--initial-branch={initial_branch}"),
            ])
            .arg(repo)
            .status()
            .map_err(|e| git_error("git init", &e))?;
        if !status.success() {
            return Err(FixtureError::Git(format!("git init failed: {status}")));
        }
        let mut child = git_command(Some(repo))
            .args(["fast-import", "--quiet", "--done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| git_error("git fast-import", &e))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| FixtureError::Git("fast-import has no stdin".to_owned()))?;
        Ok(Self {
            child,
            out: BufWriter::with_capacity(1 << 20, stdin),
        })
    }

    /// Starts a commit on `refname` (a branch continues from its tip). Follow with [`Self::file`] calls and
    /// [`Self::end_commit`].
    ///
    /// # Errors
    /// On a write error to `git`.
    pub fn begin_commit(
        &mut self,
        refname: &str,
        mark: u32,
        timestamp: i64,
        message: &str,
    ) -> Result<(), FixtureError> {
        let who = format!("{AUTHOR} {timestamp} +0000");
        writeln!(self.out, "commit {refname}")?;
        writeln!(self.out, "mark :{mark}")?;
        writeln!(self.out, "author {who}")?;
        writeln!(self.out, "committer {who}")?;
        writeln!(self.out, "data {}", message.len())?;
        self.out.write_all(message.as_bytes())?;
        writeln!(self.out)?;
        Ok(())
    }

    /// Adds or replaces a regular file. `path` must not contain spaces, quotes or newlines (fixture paths never do).
    ///
    /// # Errors
    /// On a write error to `git`.
    pub fn file(&mut self, path: &str, bytes: &[u8]) -> Result<(), FixtureError> {
        debug_assert!(!path.contains([' ', '"', '\n']), "unsupported path {path}");
        writeln!(self.out, "M 100644 inline {path}")?;
        writeln!(self.out, "data {}", bytes.len())?;
        self.out.write_all(bytes)?;
        writeln!(self.out)?;
        Ok(())
    }

    /// # Errors
    /// On a write error to `git`.
    pub fn end_commit(&mut self) -> Result<(), FixtureError> {
        writeln!(self.out)?;
        Ok(())
    }

    /// Points `refname` (for example a tag) at the commit with `mark`.
    ///
    /// # Errors
    /// On a write error to `git`.
    pub fn reset(&mut self, refname: &str, mark: u32) -> Result<(), FixtureError> {
        writeln!(self.out, "reset {refname}")?;
        writeln!(self.out, "from :{mark}")?;
        writeln!(self.out)?;
        Ok(())
    }

    /// Closes the stream and waits for `git`.
    ///
    /// # Errors
    /// When `git` reports a failure.
    pub fn finish(mut self) -> Result<(), FixtureError> {
        writeln!(self.out, "done")?;
        self.out.flush()?;
        drop(self.out);
        let status = self.child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(FixtureError::Git(format!("git fast-import failed: {status}")))
        }
    }
}

/// `refname -> object id` for every ref of the repository.
///
/// # Errors
/// When `git` fails.
pub fn refs(repo: &Path) -> Result<BTreeMap<String, String>, FixtureError> {
    let out = git_command(Some(repo))
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .map_err(|e| git_error("git for-each-ref", &e))?;
    if !out.status.success() {
        return Err(FixtureError::Git(format!(
            "git for-each-ref failed: {}",
            out.status
        )));
    }
    let text =
        String::from_utf8(out.stdout).map_err(|_| FixtureError::Git("non-UTF-8 ref name".to_owned()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(r, id)| (r.to_owned(), id.to_owned()))
        .collect())
}
