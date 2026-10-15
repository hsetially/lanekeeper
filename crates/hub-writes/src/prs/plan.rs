//! What a pull request will contain (T6): the files in the form of the repository they go to, and the warnings the person raising
//! it should read first.
//!
//! The content of a file is put into the form of the repository file it replaces (D17, rule 11): its encoding, its byte order mark
//! and its line endings. NFS files are often CRLF while a repository file is LF, or the other way round; committing the NFS bytes as
//! they are would make the pull request a rewrite of the whole file instead of the lines that changed. A file that is new on the
//! branch keeps the form it has on NFS (plan A20). Binary files go untouched.
//!
//! Content is secret-scanned before it can reach GitHub (S13): a value the scan flags blocks the pull request and is reported by
//! line number and rule name only. Denied files (D79) never leave NFS.
//!
//! Everything CPU-bound here is synchronous (rule 4): the service runs it on the blocking pool.

use std::num::NonZeroU32;

use bytes::Bytes;
use domain::{
    ContentHash, FileClass, IssueKind, NfsPath, PrJobId, RejectReason, RepoPath, RequestId, ShortText,
    SwimlaneId, UserId, ValidationIssue,
};
use engine::{DenyMatcher, Limits, ScanConfig, classify, decode_text, secret_scan};

use super::target::{Destination, Routed, Side};
use crate::fidelity::{Basis, FileFormatFacts, PrepareError, prepare_content};
use crate::github::{BranchName, GitSha, RepoFile, RepoRef};

/// The most warnings one pull request carries (`PrJob.warnings` has `maxItems: 20`).
pub const MAX_WARNINGS: usize = 20;

/// One file of the pull request, ready to commit.
#[derive(Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub nfs: NfsPath,
    /// The hash of the NFS file the change comes from (D77: every pull request is linked to it).
    pub nfs_hash: ContentHash,
    /// The path in the destination repository.
    pub repo_path: RepoPath,
    /// The bytes to commit, in the form of the repository file.
    pub content: Bytes,
    /// Whether the repository already has the file on the destination branch.
    pub exists: bool,
}

impl std::fmt::Debug for PlannedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Paths and hashes identify a file; its content is configuration and is not printed (S10).
        f.debug_struct("PlannedFile")
            .field("nfs", &self.nfs)
            .field("repo_path", &self.repo_path)
            .field("nfs_hash", &self.nfs_hash)
            .field("len", &self.content.len())
            .field("exists", &self.exists)
            .finish()
    }
}

/// A pull request, planned and not yet made. Made by [`super::PrService::plan`]; the later steps read nothing else.
#[derive(Clone)]
pub struct PrPlan {
    pub swimlane: SwimlaneId,
    pub dest: Destination,
    pub repo: RepoRef,
    /// The head of the destination branch when the plan was made: the commit the pull request is built on.
    pub head: GitSha,
    /// The tree of [`head`](Self::head).
    pub tree: GitSha,
    /// The branch the commit goes to: `lanekeeper/<swimlane>/<short id>`.
    pub branch: BranchName,
    pub title: ShortText,
    /// The text the person wrote.
    pub text: Option<String>,
    pub files: Vec<PlannedFile>,
    /// Files whose repository copy already holds this content: left out of the pull request.
    pub unchanged: Vec<NfsPath>,
    /// What the person should know about where this goes (`PrJob.warnings`).
    pub warnings: Vec<ShortText>,
    /// Who raises it, and in which request: both go into the body.
    pub user: UserId,
    pub request_id: RequestId,
    /// The job that runs the pull request, once there is one (T9).
    pub job: Option<PrJobId>,
}

impl std::fmt::Debug for PrPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // What the person wrote in the body is theirs and may hold anything: its size, never the text (S10).
        f.debug_struct("PrPlan")
            .field("swimlane", &self.swimlane)
            .field("dest", &self.dest)
            .field("repo", &self.repo)
            .field("head", &self.head)
            .field("branch", &self.branch)
            .field("title", &self.title)
            .field("text_len", &self.text.as_ref().map(String::len))
            .field("files", &self.files)
            .field("unchanged", &self.unchanged)
            .field("warnings", &self.warnings.len())
            .field("job", &self.job)
            .finish_non_exhaustive()
    }
}

impl PrPlan {
    /// The same plan, run by this job.
    #[must_use]
    pub fn for_job(mut self, job: PrJobId) -> Self {
        self.job = Some(job);
        self
    }
}

/// Why a file cannot go into a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileReject {
    pub reason: RejectReason,
    /// The places, for a secret or a file that is not text.
    pub issues: Vec<ValidationIssue>,
}

impl FileReject {
    fn of(reason: RejectReason) -> Self {
        Self {
            reason,
            issues: Vec::new(),
        }
    }
}

/// A file made ready: the bytes to commit, or "nothing to change".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prepared {
    pub content: Bytes,
    /// The repository file already has exactly these bytes.
    pub unchanged: bool,
}

fn issue(kind: IssueKind, line: u32, rule: Option<&str>) -> ValidationIssue {
    ValidationIssue {
        kind,
        line: NonZeroU32::new(line),
        rule: rule.and_then(|r| ShortText::parse(r).ok()),
    }
}

/// Put the NFS bytes of `path` into the form of `repo`, and check them. Blocking.
pub(crate) fn prepare_file(
    path: &NfsPath,
    nfs: &Bytes,
    repo: &RepoFile,
    deny: &DenyMatcher,
    limits: &Limits,
    max_bytes: usize,
) -> Result<Prepared, FileReject> {
    if deny.is_denied(path) {
        return Err(FileReject::of(RejectReason::Denied));
    }
    let class = classify(path, Some(nfs), deny);
    if class == FileClass::Denied {
        return Err(FileReject::of(RejectReason::Denied));
    }
    let basis = match repo {
        RepoFile::File { bytes, .. } => Basis::Existing(FileFormatFacts::of(bytes)),
        RepoFile::Absent => Basis::New { folder: None },
    };
    let content = prepare_content(nfs, class, basis, None, limits)
        .map_err(|e: PrepareError| FileReject::of(e.reason()))?;
    if content.len() > max_bytes {
        return Err(FileReject::of(RejectReason::TooLarge));
    }

    if class != FileClass::Binary {
        let decoded =
            decode_text(&content, limits).map_err(|_| FileReject::of(RejectReason::InvalidContent))?;
        match secret_scan(&decoded.text, &ScanConfig::default(), limits) {
            Ok(found) if found.is_empty() => {}
            Ok(found) => {
                let mut issues: Vec<ValidationIssue> = found
                    .iter()
                    .map(|m| issue(IssueKind::SecretMatch, m.line, Some(m.rule.name())))
                    .collect();
                issues.truncate(ValidationIssue::MAX_PER_OUTCOME);
                return Err(FileReject {
                    reason: RejectReason::SecretDetected,
                    issues,
                });
            }
            // More matches than the scan reports: a file full of secrets, and a short list would hide the rest.
            Err(engine::ScanError::TooManyMatches) => {
                return Err(FileReject::of(RejectReason::SecretDetected));
            }
            Err(_) => return Err(FileReject::of(RejectReason::InvalidContent)),
        }
    }

    let unchanged = matches!(repo, RepoFile::File { bytes, .. } if *bytes == content);
    Ok(Prepared { content, unchanged })
}

/// The longest `ShortText`.
const NOTE_MAX: usize = 256;

/// Add a note that fits `ShortText`: cut at a character boundary when a long path makes it too long. A note with a control
/// character cannot be one (paths, branches and tenant ids hold none); it is left out rather than guessed at.
fn push(out: &mut Vec<ShortText>, text: &str) {
    let mut end = text.len().min(NOTE_MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Ok(t) = ShortText::parse(&text[..end]) {
        out.push(t);
    }
}

/// The shortest a path is shown when the room for it is small: enough of each end to tell which file it is.
const MIN_PATH_SHOWN: usize = 24;

/// `path` in at most `max` bytes: the beginning and the end of it with `…` between, cut at character boundaries. A path that fits
/// is returned as it is.
fn abbreviate(path: &str, max: usize) -> String {
    const ELLIPSIS: &str = "\u{2026}";
    if path.len() <= max {
        return path.to_owned();
    }
    let keep = max.saturating_sub(ELLIPSIS.len());
    let mut head = keep / 2 + keep % 2;
    while !path.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = path.len() - keep / 2;
    while !path.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{ELLIPSIS}{}", &path[..head], &path[tail..])
}

/// A piece of a note: words that stay as they are, or a path that is shortened when the note would be too long.
enum Part<'a> {
    Text(&'a str),
    Path(&'a str),
}

/// A note of the parts, within a [`ShortText`]: the words are kept whole (they say what happens), and the paths share what room
/// is left. Without this a long path filled the note and [`push`] cut off the end of it, which is the consequence.
fn fitted(parts: &[Part<'_>]) -> String {
    let fixed: usize = parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.len(),
            Part::Path(_) => 0,
        })
        .sum();
    let paths = parts.iter().filter(|p| matches!(p, Part::Path(_))).count();
    let each = NOTE_MAX
        .saturating_sub(fixed)
        .checked_div(paths)
        .unwrap_or(0)
        .max(MIN_PATH_SHOWN);
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => (*t).to_owned(),
            Part::Path(path) => abbreviate(path, each),
        })
        .collect()
}

/// What one routed file needs to be warned about.
pub(crate) struct Row<'a> {
    pub routed: &'a Routed,
    /// The repository has the file on the destination branch.
    pub exists: bool,
    /// The file is read as a whole by the config-server (a resource), and not merged setting by setting (D82).
    pub whole_file: bool,
}

/// The warnings of a pull request: where it goes and what that means (D43). `unchanged` are the repository paths left out.
pub(crate) fn warnings(
    dest: &Destination,
    repo: &RepoRef,
    rows: &[Row<'_>],
    unchanged: &[RepoPath],
) -> Vec<ShortText> {
    let mut out: Vec<ShortText> = Vec::new();
    if dest.side == Side::Base {
        push(
            &mut out,
            &format!(
                "This changes {repo} on {}: every swimlane that uses the base repository picks the change up.",
                dest.branch
            ),
        );
    }
    for row in rows {
        let r = row.routed;
        match (dest.side, &r.suffix) {
            (Side::Base, Some(tenant)) => push(
                &mut out,
                &fitted(&[
                    Part::Path(r.nfs.as_str()),
                    Part::Text(&format!(
                        " is the file of tenant {tenant}: this copies one tenant's content into base as "
                    )),
                    Part::Path(r.repo_path.as_str()),
                    Part::Text("."),
                ]),
            ),
            (Side::Tenant, Some(own)) if own.as_str() != dest.branch.as_str() => push(
                &mut out,
                &fitted(&[
                    Part::Path(r.nfs.as_str()),
                    Part::Text(&format!(
                        " is the file of tenant {own}: its content goes to the file of tenant {}.",
                        dest.branch
                    )),
                ]),
            ),
            (Side::Tenant, None) if row.exists => push(
                &mut out,
                &fitted(&[
                    Part::Path(r.nfs.as_str()),
                    Part::Text(" is a base file: its content replaces "),
                    Part::Path(r.repo_path.as_str()),
                    Part::Text(&format!(" of tenant {}.", dest.branch)),
                ]),
            ),
            _ => {}
        }
        if dest.side == Side::Tenant && !row.exists {
            let consequence = if row.whole_file {
                "it forks from the base file and will not follow later base changes (C1)."
            } else {
                "its settings override base for this tenant."
            };
            push(
                &mut out,
                &fitted(&[
                    Part::Path(r.repo_path.as_str()),
                    Part::Text(&format!(" is new on branch {}: {consequence}", dest.branch)),
                ]),
            );
        }
    }
    for path in unchanged {
        push(
            &mut out,
            &fitted(&[
                Part::Path(path.as_str()),
                Part::Text(&format!(
                    " on {} already holds this content: it is left out.",
                    dest.branch
                )),
            ]),
        );
    }
    if out.len() > MAX_WARNINGS {
        let more = out.len() - (MAX_WARNINGS - 1);
        out.truncate(MAX_WARNINGS - 1);
        push(&mut out, &format!("and {more} more notes"));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::TenantId;
    use proptest::prelude::*;

    use super::*;
    use crate::prs::target::route_all;

    const LIMITS: Limits = Limits::DEFAULT;

    fn nfs(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    fn deny() -> DenyMatcher {
        DenyMatcher::with_globs(std::iter::empty()).unwrap()
    }

    fn file(bytes: &[u8]) -> RepoFile {
        RepoFile::File {
            blob: GitSha::parse(&"a".repeat(40)).unwrap(),
            bytes: Bytes::copy_from_slice(bytes),
        }
    }

    fn prep(nfs_bytes: &[u8], repo: &RepoFile) -> Result<Prepared, FileReject> {
        prepare_file(
            &nfs("a/x.yml"),
            &Bytes::copy_from_slice(nfs_bytes),
            repo,
            &deny(),
            &LIMITS,
            2 * 1024 * 1024,
        )
    }

    #[test]
    fn a_crlf_nfs_file_takes_the_lf_of_the_repository_file() {
        let got = prep(b"a: 1\r\nb: 3\r\n", &file(b"a: 1\nb: 2\n")).unwrap();
        assert_eq!(&got.content[..], b"a: 1\nb: 3\n");
        assert!(!got.unchanged);
    }

    #[test]
    fn an_lf_nfs_file_takes_the_crlf_of_the_repository_file() {
        let got = prep(b"a: 1\nb: 3\n", &file(b"a: 1\r\nb: 2\r\n")).unwrap();
        assert_eq!(&got.content[..], b"a: 1\r\nb: 3\r\n");
    }

    #[test]
    fn a_new_file_keeps_the_form_it_has_on_nfs() {
        let got = prep(b"a: 1\r\nb: 3\r\n", &RepoFile::Absent).unwrap();
        assert_eq!(&got.content[..], b"a: 1\r\nb: 3\r\n");
        assert!(!got.unchanged);
    }

    #[test]
    fn the_byte_order_mark_of_the_repository_file_is_kept() {
        let got = prep(b"a: 2\n", &file(b"\xEF\xBB\xBFa: 1\n")).unwrap();
        assert_eq!(&got.content[..], b"\xEF\xBB\xBFa: 2\n");
    }

    #[test]
    fn a_file_that_differs_only_in_line_endings_is_unchanged() {
        let got = prep(b"a: 1\r\nb: 2\r\n", &file(b"a: 1\nb: 2\n")).unwrap();
        assert!(got.unchanged);
        assert_eq!(&got.content[..], b"a: 1\nb: 2\n");
    }

    #[test]
    fn binary_content_goes_untouched() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x01\r\n\x02";
        let got = prepare_file(
            &nfs("img/receipt.bmp"),
            &Bytes::copy_from_slice(png),
            &RepoFile::File {
                blob: GitSha::parse(&"b".repeat(40)).unwrap(),
                bytes: Bytes::from_static(b"old\n"),
            },
            &deny(),
            &LIMITS,
            1024,
        )
        .unwrap();
        assert_eq!(&got.content[..], png);
    }

    #[test]
    fn a_secret_blocks_the_file_and_is_reported_by_line_only() {
        let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        let text = format!("a: 1\nkey: {key}\n");
        let e = prep(text.as_bytes(), &file(b"a: 1\n")).unwrap_err();
        assert_eq!(e.reason, RejectReason::SecretDetected);
        assert_eq!(e.issues.len(), 1);
        assert_eq!(e.issues[0].kind, IssueKind::SecretMatch);
        assert_eq!(e.issues[0].line.map(NonZeroU32::get), Some(2));
        assert!(!format!("{e:?}").contains(&key));
    }

    #[test]
    fn a_denied_file_never_leaves_nfs() {
        let e = prepare_file(
            &nfs("keys/server.pem"),
            &Bytes::from_static(b"x"),
            &RepoFile::Absent,
            &deny(),
            &LIMITS,
            1024,
        )
        .unwrap_err();
        assert_eq!(e.reason, RejectReason::Denied);
    }

    #[test]
    fn a_file_over_the_limit_is_refused_once_it_is_in_the_repository_form() {
        // 200 lines of LF become 200 more bytes in a CRLF file.
        let lf = "x: 1\n".repeat(200);
        let e = prepare_file(
            &nfs("a/x.yml"),
            &Bytes::from(lf.clone()),
            &file("y: 1\r\n".as_bytes()),
            &deny(),
            &LIMITS,
            lf.len() + 100,
        )
        .unwrap_err();
        assert_eq!(e.reason, RejectReason::TooLarge);
    }

    proptest! {
        /// Whatever the NFS text and the repository text are, the prepared content has the repository file's line ending, and
        /// is the NFS text apart from that: the pull request changes the lines that differ and no others.
        #[test]
        fn the_prepared_text_differs_from_nfs_in_line_endings_only(
            lines in proptest::collection::vec("[a-z0-9: ]{0,12}", 1..8),
            nfs_crlf in any::<bool>(),
            repo_crlf in any::<bool>(),
        ) {
            let join = |crlf: bool| lines.join(if crlf { "\r\n" } else { "\n" }) + if crlf { "\r\n" } else { "\n" };
            let (n, r) = (join(nfs_crlf), join(repo_crlf));
            let got = prep(n.as_bytes(), &file(r.as_bytes())).unwrap();
            prop_assert_eq!(&got.content[..], r.as_bytes());
            prop_assert!(got.unchanged);
        }
    }

    // ------------------------------------------------------------------------------------------------------ warnings

    fn repo() -> RepoRef {
        RepoRef::parse("acme/configuration-base-saas").unwrap()
    }

    fn routed(ids: &[&str], paths: &[&str], target: Option<&domain::PrTarget>) -> Vec<Routed> {
        let tenants = domain::TenantSet::new(ids.iter().map(|t| TenantId::parse(t).unwrap())).unwrap();
        let paths: Vec<NfsPath> = paths.iter().map(|p| nfs(p)).collect();
        route_all(
            &domain::PathMappingRule::default(),
            &tenants,
            &BranchName::parse("master").unwrap(),
            &paths,
            target,
        )
        .unwrap()
    }

    fn texts(w: &[ShortText]) -> Vec<String> {
        w.iter().map(ToString::to_string).collect()
    }

    fn rows(r: &[Routed], exists: bool, whole_file: bool) -> Vec<Row<'_>> {
        r.iter()
            .map(|routed| Row {
                routed,
                exists,
                whole_file,
            })
            .collect()
    }

    #[test]
    fn a_new_tenant_file_carries_the_fork_warning() {
        let r = routed(&["sit1"], &["a/core-sit1.yml"], None);
        let w = warnings(&r[0].dest, &repo(), &rows(&r, false, true), &[]);
        let w = texts(&w);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("new on branch sit1") && w[0].contains("forks from the base file"),
            "{w:?}"
        );
    }

    #[test]
    fn a_new_property_source_is_not_called_a_fork() {
        let r = routed(&["sit1"], &["a/core-sit1.yml"], None);
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, false, false), &[]));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("override base") && !w[0].contains("fork"), "{w:?}");
    }

    #[test]
    fn an_existing_tenant_file_has_nothing_to_warn_about() {
        let r = routed(&["sit1"], &["a/core-sit1.yml"], None);
        assert!(warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]).is_empty());
    }

    #[test]
    fn a_base_destination_notes_every_swimlane_and_a_tenant_file_copied_into_base() {
        let r = routed(
            &["sit1"],
            &["a/core-sit1.yml", "a/other.yml"],
            Some(&domain::PrTarget::Base),
        );
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[0].contains("every swimlane") && w[0].contains("master"),
            "{w:?}"
        );
        assert!(
            w[1].contains("copies one tenant's content into base") && w[1].contains("config/a/core.yml"),
            "{w:?}"
        );
    }

    #[test]
    fn a_base_file_replacing_a_tenant_file_and_a_foreign_tenant_file_are_named() {
        let t = |b: &str| domain::PrTarget::Tenant {
            branch: TenantId::parse(b).unwrap(),
        };
        let r = routed(&["sit1", "sit2"], &["a/core.yml"], Some(&t("sit1")));
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert!(w.len() == 1 && w[0].contains("is a base file"), "{w:?}");
        let r = routed(&["sit1", "sit2"], &["a/core-sit1.yml"], Some(&t("sit2")));
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert!(
            w.len() == 1 && w[0].contains("tenant sit1") && w[0].contains("tenant sit2"),
            "{w:?}"
        );
    }

    #[test]
    fn unchanged_files_are_listed_and_the_list_is_capped() {
        let r = routed(&["sit1"], &["a/core-sit1.yml"], None);
        let gone = RepoPath::parse("data/config/a/same.yml").unwrap();
        let w = texts(&warnings(
            &r[0].dest,
            &repo(),
            &rows(&r, true, true),
            &[gone.clone()],
        ));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("data/config/a/same.yml") && w[0].contains("left out"));

        let many = vec![gone; 40];
        let w = warnings(&r[0].dest, &repo(), &rows(&r, true, true), &many);
        assert_eq!(w.len(), MAX_WARNINGS);
        assert!(
            w[MAX_WARNINGS - 1].as_str().starts_with("and 21 more"),
            "{:?}",
            w[MAX_WARNINGS - 1]
        );
    }

    #[test]
    fn a_note_is_as_long_as_a_short_text_may_be() {
        assert!(ShortText::parse(&"a".repeat(NOTE_MAX)).is_ok());
        assert!(ShortText::parse(&"a".repeat(NOTE_MAX + 1)).is_err());
    }

    #[test]
    fn a_long_path_still_fits_a_warning() {
        let long = format!("{}/x.yml", "d".repeat(200));
        let r = routed(&["sit1"], &[long.as_str()], Some(&domain::PrTarget::Base));
        let w = warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]);
        assert!(w.iter().all(|t| t.as_str().len() <= NOTE_MAX));
    }

    /// The consequence is the point of a warning. A path long enough to fill the note must be shortened in the middle, and not
    /// take the words that say what happens with it.
    #[test]
    fn a_long_path_is_abbreviated_and_the_consequence_is_kept() {
        let t = |b: &str| domain::PrTarget::Tenant {
            branch: TenantId::parse(b).unwrap(),
        };
        let dir = vec!["d".repeat(100); 4].join("/");
        let tenant_file = format!("{dir}/x-sit1.yml");
        let base_file = format!("{dir}/x.yml");

        // A tenant file copied into base.
        let r = routed(&["sit1"], &[tenant_file.as_str()], Some(&domain::PrTarget::Base));
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(
            w[1].ends_with('.') && w[1].contains("this copies one tenant's content into base as"),
            "{}",
            w[1]
        );
        assert!(w[1].contains('\u{2026}') && w[1].len() <= NOTE_MAX, "{}", w[1]);
        // And the ends of the path are still there, so the person can tell which file it is.
        assert!(w[1].contains(&format!("{}\u{2026}", &dir[..20])), "{}", w[1]);
        assert!(w[1].contains("x-sit1.yml"), "{}", w[1]);

        // A tenant file sent to another tenant's branch.
        let r = routed(&["sit1", "sit2"], &[tenant_file.as_str()], Some(&t("sit2")));
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].ends_with("its content goes to the file of tenant sit2."),
            "{}",
            w[0]
        );

        // A base file replacing a tenant's file.
        let r = routed(&["sit1"], &[base_file.as_str()], Some(&t("sit1")));
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[]));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("its content replaces") && w[0].ends_with("of tenant sit1."),
            "{}",
            w[0]
        );

        // A new file on a tenant branch.
        let r = routed(&["sit1"], &[tenant_file.as_str()], None);
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, false, true), &[]));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].ends_with("it forks from the base file and will not follow later base changes (C1)."),
            "{}",
            w[0]
        );
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, false, false), &[]));
        assert!(
            w[0].ends_with("its settings override base for this tenant."),
            "{}",
            w[0]
        );

        // A file left out because the repository already has it.
        let gone = RepoPath::parse(&format!("data/config/{dir}/same.yml")).unwrap();
        let w = texts(&warnings(&r[0].dest, &repo(), &rows(&r, true, true), &[gone]));
        assert!(
            w[0].ends_with("already holds this content: it is left out."),
            "{}",
            w[0]
        );
    }

    #[test]
    fn a_path_is_cut_in_the_middle_on_character_boundaries() {
        assert_eq!(abbreviate("a/b.yml", 16), "a/b.yml");
        let long = "\u{e9}".repeat(300);
        for max in [16, 17, 40, 41] {
            let got = abbreviate(&long, max);
            assert!(got.len() <= max, "{max}: {}", got.len());
            assert!(got.contains('\u{2026}'));
            assert!(got.starts_with('\u{e9}') && got.ends_with('\u{e9}'));
        }
    }
}
