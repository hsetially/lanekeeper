use bytes::Bytes;
use domain::{CommitId, GitRef, PathChangeKind, RepoKind, RepoPath};

use crate::{GitError, GitReader, content_hash};

/// What the repository handed to [`git_reader`] must contain. A real implementation's harness builds a
/// repository with exactly this shape (for example with `git fast-import`).
#[derive(Debug, Clone)]
pub struct GitExpectation {
    pub repo: RepoKind,
    pub branch: String,
    /// The files at the head of `branch`, exactly.
    pub files: Vec<(RepoPath, Bytes)>,
    /// A file that exists at the head and was changed by exactly `history_commits` commits on `branch`
    /// (counting the one that created it), the oldest of which is not the first commit's only change.
    pub history_path: RepoPath,
    pub history_commits: usize,
    /// The tag on an ancestor of the head, and the number of commits from the head back to it.
    pub tag: Option<(GitRef, u32)>,
}

fn unknown_commit() -> CommitId {
    CommitId::parse(&"0".repeat(40)).unwrap()
}

/// - `head`, `branches` and `tags` agree with each other; unknown branches are `NotFound`, malformed ones
///   `InvalidRef`.
/// - `tree_index` lists exactly the expected files with their SHA-256 and size, sorted by path.
/// - `read` returns exact bytes (no line-ending changes), `Ok(None)` for a missing path and `NotFound` for
///   an unknown commit.
/// - `history` is newest first and honours `limit`; `diff_trees` of a commit with itself is empty and an
///   older commit shows the later change; `describe` reports the nearest tag and distance.
pub fn git_reader<G: GitReader + ?Sized>(g: &G, x: &GitExpectation) {
    let head = g.head(x.repo, &x.branch).expect("head of the expected branch");

    assert_eq!(g.head(x.repo, "no-such-branch"), Err(GitError::NotFound));
    assert_eq!(g.head(x.repo, "bad..ref"), Err(GitError::InvalidRef));
    assert_eq!(g.head(x.repo, "-rf"), Err(GitError::InvalidRef));

    let branches = g.branches(x.repo).unwrap();
    let listed = branches
        .iter()
        .find(|b| b.name.as_str() == x.branch)
        .expect("branches() lists the expected branch");
    assert_eq!(listed.head, head, "branches() and head() agree");

    let index = g.tree_index(x.repo, &head).unwrap();
    let mut expected = x.files.clone();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let got: Vec<_> = index
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.sha256, e.size))
        .collect();
    let want: Vec<_> = expected
        .iter()
        .map(|(p, b)| (p.clone(), content_hash(b), b.len() as u64))
        .collect();
    assert_eq!(got, want, "tree index of the head commit");
    assert!(
        index.entries().windows(2).all(|w| w[0].path < w[1].path),
        "entries are sorted by path"
    );

    for (path, bytes) in &x.files {
        assert_eq!(
            g.read(x.repo, &head, path).unwrap().as_ref(),
            Some(bytes),
            "read {path}"
        );
    }
    let missing = RepoPath::parse("no/such/file.yml").unwrap();
    assert_eq!(g.read(x.repo, &head, &missing).unwrap(), None);
    assert_eq!(
        g.read(x.repo, &unknown_commit(), &missing),
        Err(GitError::NotFound)
    );
    assert_eq!(
        g.tree_index(x.repo, &unknown_commit()).err(),
        Some(GitError::NotFound)
    );

    let all = g.history(x.repo, &x.branch, &x.history_path, 1000).unwrap();
    assert_eq!(
        all.len(),
        x.history_commits,
        "commits that changed the history path"
    );
    assert!(
        all.windows(2).all(|w| w[0].time >= w[1].time),
        "history is newest first"
    );
    assert_eq!(g.history(x.repo, &x.branch, &x.history_path, 1).unwrap().len(), 1);
    assert!(
        g.history(x.repo, &x.branch, &x.history_path, 0)
            .unwrap()
            .is_empty()
    );
    assert!(g.history(x.repo, &x.branch, &missing, 10).unwrap().is_empty());
    assert_eq!(
        g.history(x.repo, "no-such-branch", &x.history_path, 10),
        Err(GitError::NotFound)
    );

    assert!(g.diff_trees(x.repo, &head, &head).unwrap().is_empty());
    if let Some(oldest) = all.last() {
        let changes = g.diff_trees(x.repo, &oldest.id, &head).unwrap();
        let ours = changes.iter().find(|c| c.path == x.history_path);
        if x.history_commits > 1 {
            assert_eq!(
                ours.map(|c| c.kind),
                Some(PathChangeKind::Modified),
                "the history path changed since its first commit"
            );
        }
        assert!(
            changes.windows(2).all(|w| w[0].path < w[1].path),
            "changes are sorted by path"
        );
    }

    let label = g.describe(x.repo, &head).unwrap();
    assert_eq!(label.commit, head);
    match &x.tag {
        Some((tag, distance)) => {
            assert_eq!(label.tag.as_ref(), Some(tag));
            assert_eq!(label.distance, *distance);
            let tags = g.tags(x.repo).unwrap();
            assert!(tags.iter().any(|t| &t.name == tag), "tags() lists the tag");
        }
        None => assert_eq!(label.tag, None),
    }
}
