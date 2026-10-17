//! The synthetic repositories of the fixtures (`repos/*.git`, rule 12) loaded into the in-memory Git (plan A41: `FakeGit` loaded from the
//! generated repositories, never `GixReader`): the head of the base repository's default branch and of every branch of the tenant
//! repository, one commit each. The pipeline only reads heads, so history is not needed.
//!
//! Reads go through the `git` command line (the generator needs `git` too): `ls-tree -r` for the paths and object ids, and one
//! `cat-file --batch` for the content.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use domain::RepoKind;
use ports::fakes::FakeGit;

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("--git-dir").arg(dir);
    c
}

/// The names of the branches of a bare repository.
fn branches(repo: &Path) -> Vec<String> {
    let out = git(repo)
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git for-each-ref failed");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Every file of a branch's tree: path and content.
fn files(repo: &Path, branch: &str) -> Vec<(String, Vec<u8>)> {
    let tree = git(repo)
        .args(["ls-tree", "-r", "-z", branch])
        .output()
        .expect("git runs");
    assert!(tree.status.success(), "git ls-tree failed");
    // "<mode> <type> <oid>\t<path>\0" for each entry.
    let mut entries: Vec<(String, String)> = Vec::new();
    for record in tree.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let (meta, path) = text.split_once('\t').expect("an ls-tree record");
        let mut parts = meta.split(' ');
        let (_mode, kind, oid) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        if kind == "blob" {
            entries.push((oid.to_owned(), path.to_owned()));
        }
    }
    let mut child = git(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git cat-file runs");
    let mut stdin = child.stdin.take().unwrap();
    let oids: Vec<String> = entries.iter().map(|(o, _)| o.clone()).collect();
    // Written on a thread of its own so that a full pipe cannot stop either side.
    let writer = std::thread::spawn(move || {
        for oid in oids {
            writeln!(stdin, "{oid}").unwrap();
        }
    });
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut result = Vec::with_capacity(entries.len());
    for (_, path) in entries {
        let mut header = String::new();
        out.read_line(&mut header).unwrap();
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .expect("a cat-file header")
            .parse()
            .unwrap();
        let mut content = vec![0_u8; size];
        out.read_exact(&mut content).unwrap();
        let mut newline = [0_u8; 1];
        out.read_exact(&mut newline).unwrap();
        result.push((path, content));
    }
    writer.join().unwrap();
    assert!(child.wait().unwrap().success());
    result
}

/// The fixtures' repositories in an in-memory Git: `master` of the base repository, every branch of the tenant repository.
pub fn load(root: &Path) -> FakeGit {
    let fake = FakeGit::new();
    let repos = root.join("repos");
    for (kind, dir, only) in [
        (RepoKind::Base, "configuration-base-saas.git", Some("master")),
        (RepoKind::Tenant, "csp-tenant-data.git", None),
    ] {
        let repo = repos.join(dir);
        let names = match only {
            Some(name) => vec![name.to_owned()],
            None => branches(&repo),
        };
        for branch in names {
            let content = files(&repo, &branch);
            let changes: Vec<(&str, Option<&[u8]>)> = content
                .iter()
                .map(|(p, c)| (p.as_str(), Some(c.as_slice())))
                .collect();
            fake.commit(kind, &branch, &changes, "load").unwrap();
        }
    }
    fake
}
