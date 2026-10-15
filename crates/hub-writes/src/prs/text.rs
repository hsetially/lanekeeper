//! The words of a pull request (T6): the commit message, the body, and the short id in the branch name.
//!
//! The body carries what makes the pull request traceable (D77): the swimlane, the NFS hash of each file, who raised it and the
//! audit event. It never carries file content. What the person wrote is theirs and goes first, cut to a size that leaves room for
//! the rest (GitHub allows 65,536 characters in a body).

use std::fmt::Write as _;

use domain::{AuditId, ContentHash};
use ports::content_hash;

use super::plan::PrPlan;

/// How much of the person's own text goes into the body. The generated part follows it and takes at most the rest of GitHub's
/// 65,536 characters (100 files at about 450 bytes each, and the notes).
pub const MAX_TEXT_BYTES: usize = 8 * 1024;

/// The most files named one by one in a commit message.
const MAX_LISTED: usize = 20;

/// The characters of the short id in the branch name.
pub const SHORT_ID_LEN: usize = 10;

/// The short id of a pull request's branch: ten hexadecimal digits of a digest of the request and of the files it takes, so the
/// same request makes the same branch name and two requests do not.
#[must_use]
pub fn short_id(plan: &PrPlan) -> String {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(b"lanekeeper-pr-v1\0");
    preimage.extend_from_slice(plan.request_id.as_str().as_bytes());
    preimage.push(0);
    preimage.extend_from_slice(plan.swimlane.as_str().as_bytes());
    for f in &plan.files {
        preimage.push(0);
        preimage.extend_from_slice(f.nfs.as_str().as_bytes());
        preimage.push(0);
        preimage.extend_from_slice(f.nfs_hash.to_string().as_bytes());
    }
    let digest: ContentHash = content_hash(&preimage);
    digest.to_string().chars().take(SHORT_ID_LEN).collect()
}

/// The commit message: the title, then what it is.
#[must_use]
pub fn commit_message(plan: &PrPlan) -> String {
    let mut out = format!(
        "{}\n\nRaised by Lanekeeper from the NFS files of swimlane {}.\n\n",
        plan.title, plan.swimlane
    );
    for f in plan.files.iter().take(MAX_LISTED) {
        let _ = writeln!(out, "- {}", f.repo_path);
    }
    if plan.files.len() > MAX_LISTED {
        let _ = writeln!(out, "- and {} more", plan.files.len() - MAX_LISTED);
    }
    out
}

/// Text that goes into a table cell or a code span: the characters that would end either are replaced.
fn cell(text: &str) -> String {
    text.chars()
        .map(|c| if matches!(c, '`' | '|') { '\'' } else { c })
        .collect()
}

fn cut(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The body of the pull request. `audit` is the audit event of the pull request once it has one; a pull request is opened before
/// its audit event exists (the event records the pull request's address), so the first body says so and the body is replaced when
/// the event is written.
#[must_use]
pub fn pull_body(plan: &PrPlan, audit: Option<AuditId>) -> String {
    let mut out = String::new();
    if let Some(text) = plan.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        out.push_str(cut(text, MAX_TEXT_BYTES));
        if text.len() > MAX_TEXT_BYTES {
            out.push_str("\n\n(The text was cut to fit.)");
        }
        out.push_str("\n\n---\n\n");
    }
    let _ = writeln!(
        out,
        "Raised by Lanekeeper from the NFS files of swimlane `{}`.\n",
        cell(plan.swimlane.as_str())
    );
    out.push_str("| NFS path | Repository path | NFS hash |\n|---|---|---|\n");
    for f in &plan.files {
        let _ = writeln!(
            out,
            "| `{}` | `{}` | `{}` |",
            cell(f.nfs.as_str()),
            cell(f.repo_path.as_str()),
            f.nfs_hash
        );
    }
    out.push('\n');
    let _ = writeln!(out, "- Raised by: `{}`", cell(&plan.user.to_string()));
    let _ = writeln!(out, "- Request: `{}`", cell(plan.request_id.as_str()));
    if let Some(job) = plan.job {
        let _ = writeln!(out, "- Lanekeeper job: {job}");
    }
    match audit {
        Some(id) => {
            let _ = writeln!(out, "- Audit event: {id}");
        }
        None => out.push_str("- Audit event: added when the pull request is recorded\n"),
    }
    if !plan.warnings.is_empty() {
        out.push_str("\nNotes:\n\n");
        for w in &plan.warnings {
            let _ = writeln!(out, "- {}", w.as_str().replace('\n', " "));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use domain::{NfsPath, RepoPath, RequestId, ShortText, SwimlaneId, UserId};

    use super::*;
    use crate::github::{BranchName, GitSha, RepoRef};
    use crate::prs::plan::PlannedFile;
    use crate::prs::target::{Destination, Side};

    fn sha(c: char) -> GitSha {
        GitSha::parse(&c.to_string().repeat(40)).unwrap()
    }

    fn plan(files: &[(&str, &str, &str)]) -> PrPlan {
        PrPlan {
            swimlane: SwimlaneId::parse("sit1").unwrap(),
            dest: Destination {
                side: Side::Tenant,
                branch: BranchName::parse("sit1").unwrap(),
            },
            repo: RepoRef::parse("acme/csp-tenant-data").unwrap(),
            head: sha('a'),
            tree: sha('b'),
            branch: BranchName::parse("lanekeeper/sit1/0123456789").unwrap(),
            title: ShortText::parse("Sync the NFS changes").unwrap(),
            text: Some("Please look at the receipt text.".to_owned()),
            files: files
                .iter()
                .map(|(nfs, repo, content)| PlannedFile {
                    nfs: NfsPath::parse(nfs).unwrap(),
                    nfs_hash: content_hash(content.as_bytes()),
                    repo_path: RepoPath::parse(repo).unwrap(),
                    content: Bytes::copy_from_slice(content.as_bytes()),
                    exists: true,
                })
                .collect(),
            unchanged: Vec::new(),
            warnings: vec![ShortText::parse("a note").unwrap()],
            user: "11111111-1111-1111-1111-111111111111:22222222-2222-2222-2222-222222222222"
                .parse::<UserId>()
                .unwrap(),
            request_id: RequestId::parse("req-1").unwrap(),
            job: None,
        }
    }

    #[test]
    fn the_body_names_the_swimlane_the_hashes_the_user_and_the_audit_event() {
        let p = plan(&[("a/x-sit1.yml", "data/config/a/x.yml", "k: v\n")]);
        let body = pull_body(&p, Some(AuditId::from(42)));
        assert!(body.contains("swimlane `sit1`"), "{body}");
        assert!(body.contains(&content_hash(b"k: v\n").to_string()), "{body}");
        assert!(
            body.contains("`a/x-sit1.yml`") && body.contains("`data/config/a/x.yml`"),
            "{body}"
        );
        assert!(body.contains(&p.user.to_string()), "{body}");
        assert!(body.contains("Audit event: 42"), "{body}");
        assert!(body.contains("- a note"), "{body}");
        assert!(body.starts_with("Please look at the receipt text."), "{body}");
        // Before the event exists, the body says that it will be added.
        assert!(pull_body(&p, None).contains("added when the pull request is recorded"));
        // Never the content.
        assert!(!body.contains("k: v"));
    }

    #[test]
    fn the_job_is_named_once_there_is_one() {
        let p = plan(&[("a/x.yml", "config/a/x.yml", "k: v\n")]).for_job(domain::PrJobId::from(7));
        assert!(pull_body(&p, None).contains("Lanekeeper job: 7"));
    }

    #[test]
    fn what_the_person_wrote_is_cut_to_leave_room() {
        let mut p = plan(&[("a/x.yml", "config/a/x.yml", "k: v\n")]);
        p.text = Some("é".repeat(MAX_TEXT_BYTES));
        let body = pull_body(&p, None);
        assert!(body.contains("(The text was cut to fit.)"));
        assert!(body.len() < MAX_TEXT_BYTES + 2_000);
    }

    #[test]
    fn a_hundred_files_fit_in_githubs_limit() {
        let files: Vec<(String, String, String)> = (0..100)
            .map(|i| {
                let dir = "d".repeat(180);
                (
                    format!("{dir}/f{i}-sit1.yml"),
                    format!("data/config/{dir}/f{i}.yml"),
                    format!("k: {i}\n"),
                )
            })
            .collect();
        let refs: Vec<(&str, &str, &str)> = files
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
            .collect();
        let mut p = plan(&refs);
        p.text = Some("x".repeat(60_000));
        p.warnings = (0..20)
            .map(|_| ShortText::parse(&"w".repeat(256)).unwrap())
            .collect();
        assert!(pull_body(&p, Some(AuditId::from(1))).chars().count() < 65_536);
    }

    #[test]
    fn a_name_cannot_break_out_of_a_table_cell() {
        let p = plan(&[(
            "a/we`ird|name-sit1.yml",
            "data/config/a/we`ird|name.yml",
            "k: v\n",
        )]);
        let body = pull_body(&p, None);
        let row = body.lines().find(|l| l.contains("ird")).unwrap();
        // Three cells: four pipes, and no more.
        assert_eq!(row.matches('|').count(), 4, "{row}");
        assert_eq!(row.matches('`').count(), 6, "{row}");
    }

    #[test]
    fn the_short_id_follows_the_request_and_the_files() {
        let p = plan(&[("a/x.yml", "config/a/x.yml", "k: v\n")]);
        let id = short_id(&p);
        assert_eq!(id.len(), SHORT_ID_LEN);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(id, short_id(&p), "the same request makes the same branch");
        let mut other = p.clone();
        other.request_id = RequestId::parse("req-2").unwrap();
        assert_ne!(id, short_id(&other));
        assert_ne!(id, short_id(&plan(&[("a/x.yml", "config/a/x.yml", "k: w\n")])));
        assert!(BranchName::for_pull(&p.swimlane, &id).is_ok());
    }

    #[test]
    fn the_commit_message_has_the_title_and_the_files() {
        let p = plan(&[("a/x-sit1.yml", "data/config/a/x.yml", "k: v\n")]);
        let m = commit_message(&p);
        assert!(m.starts_with("Sync the NFS changes\n\n"));
        assert!(m.contains("swimlane sit1") && m.contains("- data/config/a/x.yml"));
        assert!(!m.contains("k: v"));
    }
}
