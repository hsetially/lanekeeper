//! T6 (D41, D43, D77; S8, S10, S11, S13): pull requests for NFS-only changes, against a simulated GitHub on the loopback. The path is
//! reverse-mapped, the files are put into the form of the repository's own, the commit goes through the Git Data API with the
//! caller's own token, and the pull request names the swimlane, the hashes and the user.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use domain::{
    AuditId, PrRequest, PrTarget, RejectReason, Role, Secret, ShortText, SwimlaneId, TenantId, User, WriteCtx,
};
use hub_writes::github::{BranchName, GithubClient, GithubClientConfig, GithubError, RepoRef};
use hub_writes::prs::{PrDeps, PrError, PrService, PrSettings, Side, TargetError};
use hub_writes::vault::{PolicyRepos, TokenError, VaultDeps};
use ports::conformance::sample::nfs;
use ports::fakes::FakeKmsEnvelope;
use ports::{WriteService, content_hash};

use crate::support::fake_git::GitSim;
use crate::support::fake_github::{FakeGithub, LocalGithub, Reply, Seen};
use crate::support::log_capture;
use crate::support::pg::TestDb;
use crate::support::rig::{Rig, at, ctx, edit};

pub(crate) const BASE: &str = "acme/configuration-base-saas";
pub(crate) const TENANT: &str = "acme/csp-tenant-data";

/// A well-formed fine-grained token, distinct for each `n`. Not a real token.
pub(crate) fn tok(n: u32) -> String {
    format!("github_pat_11P{n:03}{}", "Qz9_".repeat(15))
}

/// The same bounds as in service, with waits short enough for a test.
pub(crate) fn fast() -> GithubClientConfig {
    GithubClientConfig {
        attempt_timeout: Duration::from_millis(150),
        total_timeout: Duration::from_secs(5),
        max_attempts: 3,
        backoff_base: Duration::from_millis(2),
        backoff_cap: Duration::from_millis(10),
        max_rate_wait: Duration::from_millis(100),
        ..GithubClientConfig::default()
    }
}

pub(crate) struct Prs {
    pub(crate) rig: Rig,
    pub(crate) sim: GitSim,
    pub(crate) server: LocalGithub,
    pub(crate) svc: PrService,
    pub(crate) editor: User,
    pub(crate) s: SwimlaneId,
    pub(crate) gh: FakeGithub,
    pub(crate) vault: hub_writes::vault::TokenVault,
    pub(crate) kms: Arc<FakeKmsEnvelope>,
    pub(crate) repos: PolicyRepos,
    pub(crate) deps: PrDeps,
}

pub(crate) async fn fixture() -> Option<Prs> {
    let rig = Rig::new(TestDb::ready().await?).await;
    let s = rig.swimlane("sit1", true).await;
    rig.tenant(&s, "sit1").await;
    let editor = rig.user(1, Role::Editor).await;

    let gh = FakeGithub::default();
    gh.token(&tok(1), "octocat", &[BASE, TENANT]);
    let repos = PolicyRepos {
        base: RepoRef::parse(BASE).unwrap(),
        tenant: RepoRef::parse(TENANT).unwrap(),
    };
    let kms = Arc::new(FakeKmsEnvelope::new());
    let vault = rig.hub.token_vault(VaultDeps {
        kms: kms.clone(),
        github: Arc::new(gh.clone()),
        repos: repos.clone(),
    });
    vault
        .put(&ctx(&editor, "k-token"), Secret::new(tok(1)))
        .await
        .unwrap();

    let sim = GitSim::new();
    sim.expect_token(&tok(1));
    let server = LocalGithub::start(sim.handler()).await;
    let client = GithubClient::for_tests(&server.base(), fast()).unwrap();
    let deps = PrDeps {
        vault: vault.clone(),
        git: Arc::new(client),
        settings: PrSettings {
            repos: repos.clone(),
            base_branch: BranchName::parse("master").unwrap(),
            max_total_bytes: PrSettings::DEFAULT_MAX_TOTAL_BYTES,
        },
    };
    let svc = rig.hub.pull_requests(deps.clone());
    Some(Prs {
        rig,
        sim,
        server,
        svc,
        editor,
        s,
        gh,
        vault,
        kms,
        repos,
        deps,
    })
}

impl Prs {
    pub(crate) fn ctx(&self, key: &str) -> WriteCtx {
        ctx(&self.editor, key)
    }

    pub(crate) fn request(&self, paths: &[&str], target: Option<PrTarget>) -> PrRequest {
        PrRequest {
            swimlane: self.s.clone(),
            paths: paths.iter().map(|p| nfs(p)).collect(),
            title: ShortText::parse("Sync the NFS changes").unwrap(),
            body: Some("Please review.".to_owned()),
            target,
        }
    }

    /// The requests that wrote something (everything but a read).
    fn writes(&self) -> Vec<Seen> {
        self.server
            .seen()
            .into_iter()
            .filter(|r| r.method != "GET")
            .collect()
    }

    fn post_bodies(&self, tail: &str) -> Vec<serde_json::Value> {
        self.server
            .seen()
            .iter()
            .filter(|r| r.method == "POST" && r.path.ends_with(tail))
            .map(Seen::json)
            .collect()
    }
}

pub(crate) fn sit1() -> PrTarget {
    PrTarget::Tenant {
        branch: TenantId::parse("sit1").unwrap(),
    }
}

/// The lines of `new` that are not lines of `old` at the same place, for two texts with the same number of lines.
fn changed_lines<'a>(old: &'a [u8], new: &'a [u8]) -> Vec<(&'a [u8], &'a [u8])> {
    let (o, n): (Vec<&[u8]>, Vec<&[u8]>) = (
        old.split_inclusive(|b| *b == b'\n').collect(),
        new.split_inclusive(|b| *b == b'\n').collect(),
    );
    assert_eq!(o.len(), n.len(), "the line count changed");
    o.into_iter().zip(n).filter(|(a, b)| a != b).collect()
}

// ------------------------------------------------------------------------------------------------------ the three targets

#[tokio::test]
async fn tenant_target_with_existing_file_commits_only_changed_lines() {
    let Some(f) = fixture().await else { return };
    let old = b"a: 1\nb: 2\nc: 3\n";
    let head = f.sim.branch(
        TENANT,
        "sit1",
        &[
            ("data/config/tx-api/entry.yml", old),
            ("data/config/other.yml", b"z: 1\n"),
        ],
    );
    f.sim
        .branch(BASE, "master", &[("config/tx-api/entry.yml", b"a: 0\n")]);
    // NFS is CRLF, the repository LF: committing the NFS bytes as they are would rewrite every line.
    f.rig
        .seed(&f.s, &[("tx-api/entry-sit1.yml", "a: 1\r\nb: 20\r\nc: 3\r\n")]);
    let before = f.sim.object_count(TENANT);

    let made = f
        .svc
        .create(&f.ctx("pr-1"), &f.request(&["tx-api/entry-sit1.yml"], None))
        .await
        .unwrap();

    // Where it went: the branch of the tenant, from the reverse-mapped path, on a branch of our own.
    assert_eq!(made.plan.dest.side, Side::Tenant);
    assert_eq!(made.plan.dest.branch.as_str(), "sit1");
    assert_eq!(made.plan.files.len(), 1);
    assert_eq!(
        made.plan.files[0].repo_path.as_str(),
        "data/config/tx-api/entry.yml"
    );
    assert!(made.plan.files[0].exists);
    assert!(made.plan.warnings.is_empty(), "{:?}", made.plan.warnings);
    let branch = made.committed.branch.as_str();
    assert!(
        branch.starts_with("lanekeeper/sit1/") && branch.len() == "lanekeeper/sit1/".len() + 10,
        "{branch}"
    );

    // What it holds: the repository file's own line endings, and the one line that changed.
    let committed = f
        .sim
        .file(TENANT, branch, "data/config/tx-api/entry.yml")
        .unwrap();
    assert_eq!(committed, b"a: 1\nb: 20\nc: 3\n");
    assert_eq!(
        changed_lines(old, &committed),
        vec![(&b"b: 2\n"[..], &b"b: 20\n"[..])]
    );
    assert_eq!(
        f.sim.paths(TENANT, branch),
        f.sim.paths(TENANT, "sit1"),
        "no other file is in the commit"
    );
    assert_eq!(
        f.sim.file(TENANT, branch, "data/config/other.yml").unwrap(),
        b"z: 1\n"
    );
    let (parents, message) = f.sim.head_commit(TENANT, branch).unwrap();
    assert_eq!(parents, vec![head.clone()], "built on the head the plan read");
    assert!(message.starts_with("Sync the NFS changes") && message.contains("data/config/tx-api/entry.yml"));
    // The destination branch is untouched: a pull request is the way in.
    assert_eq!(f.sim.head(TENANT, "sit1"), Some(head));
    assert_eq!(
        f.sim.object_count(TENANT) - before,
        3,
        "one blob, one tree, one commit"
    );
    assert!(
        f.sim.branches(BASE) == vec!["master".to_owned()],
        "the base repository is not touched"
    );

    // The pull request.
    let pulls = f.sim.pulls(TENANT);
    assert_eq!(pulls.len(), 1);
    assert_eq!((pulls[0].head.as_str(), pulls[0].base.as_str()), (branch, "sit1"));
    assert_eq!(pulls[0].title, "Sync the NFS changes");
    assert_eq!(made.opened.number, 1);
    assert_eq!(made.opened.url, pulls[0].url);
    let body = &pulls[0].body;
    assert!(body.starts_with("Please review."), "{body}");
    assert!(body.contains("swimlane `sit1`"), "{body}");
    assert!(
        body.contains(&content_hash(b"a: 1\r\nb: 20\r\nc: 3\r\n").to_string()),
        "the NFS hash: {body}"
    );
    assert!(body.contains(&f.editor.id.to_string()), "the user: {body}");
    assert!(!body.contains("b: 20"), "never the content");
}

#[tokio::test]
async fn tenant_target_with_new_file_carries_the_fork_warning() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/other.yml", b"z: 1\r\n")]);
    f.rig.seed(
        &f.s,
        &[
            ("tx-api/fresh-sit1.yml", "k: v\r\nm: n\r\n"),
            ("tx-api/based.yml", "p: q\n"),
        ],
    );

    // A tenant file, no target: it goes to its own tenant's branch, and it is new there.
    let made = f
        .svc
        .create(&f.ctx("pr-new"), &f.request(&["tx-api/fresh-sit1.yml"], None))
        .await
        .unwrap();
    assert_eq!(
        made.plan.files[0].repo_path.as_str(),
        "data/config/tx-api/fresh.yml"
    );
    assert!(!made.plan.files[0].exists);
    let warnings: Vec<&str> = made.plan.warnings.iter().map(ShortText::as_str).collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("data/config/tx-api/fresh.yml")
            && warnings[0].contains("new on branch sit1")
            && warnings[0].contains("forks from the base file"),
        "{warnings:?}"
    );
    // A new file keeps the form it has on NFS (CRLF here).
    let branch = made.committed.branch.as_str();
    assert_eq!(
        f.sim
            .file(TENANT, branch, "data/config/tx-api/fresh.yml")
            .unwrap(),
        b"k: v\r\nm: n\r\n"
    );
    assert!(f.sim.pulls(TENANT)[0].body.contains("forks from the base file"));

    // A base file with an explicit tenant target: the same warning, and the file becomes the tenant's.
    let made = f
        .svc
        .create(
            &f.ctx("pr-new-2"),
            &f.request(&["tx-api/based.yml"], Some(sit1())),
        )
        .await
        .unwrap();
    assert_eq!(
        made.plan.files[0].repo_path.as_str(),
        "data/config/tx-api/based.yml"
    );
    assert_eq!(made.plan.warnings.len(), 1);
    assert!(
        made.plan.warnings[0]
            .as_str()
            .contains("forks from the base file")
    );
}

#[tokio::test]
async fn base_target_notes_every_swimlane_and_warns_for_a_tenant_file() {
    let Some(f) = fixture().await else { return };
    f.sim.branch(
        BASE,
        "master",
        &[
            ("config/tx-api/entry.yml", b"a: 1\r\nb: 2\r\n"),
            ("config/tx-api/plain.yml", b"p: 1\n"),
        ],
    );
    f.sim.branch(TENANT, "sit1", &[("data/config/x.yml", b"x: 1\n")]);
    f.rig.seed(
        &f.s,
        &[
            ("tx-api/entry-sit1.yml", "a: 1\nb: 3\n"),
            ("tx-api/plain.yml", "p: 2\n"),
        ],
    );

    // A tenant file copied into base: the suffix is dropped, the repository file's CRLF is kept, and there are two warnings.
    let made = f
        .svc
        .create(
            &f.ctx("pr-base"),
            &f.request(&["tx-api/entry-sit1.yml"], Some(PrTarget::Base)),
        )
        .await
        .unwrap();
    assert_eq!(made.plan.dest.side, Side::Base);
    assert_eq!(made.plan.dest.branch.as_str(), "master");
    assert_eq!(made.plan.repo.to_string(), BASE);
    assert_eq!(made.plan.files[0].repo_path.as_str(), "config/tx-api/entry.yml");
    let w: Vec<&str> = made.plan.warnings.iter().map(ShortText::as_str).collect();
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(
        w[0].contains("every swimlane") && w[0].contains("master"),
        "{w:?}"
    );
    assert!(
        w[1].contains("copies one tenant's content into base") && w[1].contains("tx-api/entry-sit1.yml"),
        "{w:?}"
    );
    let branch = made.committed.branch.as_str();
    assert_eq!(
        f.sim.file(BASE, branch, "config/tx-api/entry.yml").unwrap(),
        b"a: 1\r\nb: 3\r\n"
    );
    let pulls = f.sim.pulls(BASE);
    assert_eq!(
        (pulls[0].head.as_str(), pulls[0].base.as_str()),
        (branch, "master")
    );
    assert!(pulls[0].body.contains("every swimlane"), "{}", pulls[0].body);
    assert!(
        f.sim.pulls(TENANT).is_empty(),
        "the tenant repository is not touched"
    );

    // A base file for the base: only the note about every swimlane.
    let made = f
        .svc
        .create(&f.ctx("pr-base-2"), &f.request(&["tx-api/plain.yml"], None))
        .await
        .unwrap();
    assert_eq!(made.plan.dest.side, Side::Base);
    assert_eq!(made.plan.warnings.len(), 1);
    assert!(made.plan.warnings[0].as_str().contains("every swimlane"));
}

#[tokio::test]
async fn crlf_survives_the_git_data_api_blob() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/x.yml", b"x: 1\r\n")]);
    // A mixed file with a byte order mark: base64 carries it as it is.
    let nfs_text = "\u{feff}a: 1\r\nb: 2\r\n";
    f.rig.seed(&f.s, &[("tx-api/crlf-sit1.yml", nfs_text)]);
    f.svc
        .create(&f.ctx("pr-crlf"), &f.request(&["tx-api/crlf-sit1.yml"], None))
        .await
        .unwrap();

    let blobs = f.post_bodies("/git/blobs");
    assert_eq!(blobs.len(), 1);
    assert_eq!(blobs[0]["encoding"], "base64");
    let sent = base64::engine::general_purpose::STANDARD
        .decode(blobs[0]["content"].as_str().unwrap())
        .unwrap();
    assert_eq!(sent, nfs_text.as_bytes());
    assert!(sent.windows(2).any(|w| w == b"\r\n") && sent.starts_with(b"\xEF\xBB\xBF"));

    // The tree puts a regular file at the repository path and nothing else.
    let trees = f.post_bodies("/git/trees");
    assert_eq!(trees.len(), 1);
    let entries = trees[0]["tree"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "data/config/tx-api/crlf.yml");
    assert_eq!(
        (entries[0]["mode"].as_str(), entries[0]["type"].as_str()),
        (Some("100644"), Some("blob"))
    );
}

/// The PR half of the verify of T2: a CRLF file edited from LF input keeps its line endings on NFS (T2), and the pull request
/// built from it differs from the repository file in the one line that was edited.
#[tokio::test]
async fn pr_diff_contains_only_changed_lines() {
    let Some(f) = fixture().await else { return };
    let repo_old = b"a: 1\r\nb: 2\r\nc: 3\r\n";
    f.sim
        .branch(TENANT, "sit1", &[("data/config/tx-api/entry.yml", repo_old)]);
    let old = "a: 1\r\nb: 2\r\nc: 3\r\n";
    f.rig.seed(&f.s, &[("tx-api/entry-sit1.yml", old)]);

    // The editor types LF into the browser.
    let applied = f
        .rig
        .hub
        .edit(
            &f.ctx("edit-1"),
            edit(&f.s, "tx-api/entry-sit1.yml", at(old), "a: 1\nb: 20\nc: 3\n"),
        )
        .await
        .unwrap();
    assert!(applied.is_success());
    let on_nfs = f.rig.nfs_of(&f.s, "tx-api/entry-sit1.yml").unwrap();
    assert_eq!(
        &on_nfs[..],
        b"a: 1\r\nb: 20\r\nc: 3\r\n",
        "T2: the style of the file is kept"
    );
    f.rig.catch_up_registry(&f.s);

    let made = f
        .svc
        .create(&f.ctx("pr-diff"), &f.request(&["tx-api/entry-sit1.yml"], None))
        .await
        .unwrap();
    let committed = f
        .sim
        .file(
            TENANT,
            made.committed.branch.as_str(),
            "data/config/tx-api/entry.yml",
        )
        .unwrap();
    assert_eq!(committed, &on_nfs[..]);
    let changed = changed_lines(repo_old, &committed);
    assert_eq!(
        changed,
        vec![(&b"b: 2\r\n"[..], &b"b: 20\r\n"[..])],
        "only the edited line differs"
    );
    assert!(
        committed
            .split_inclusive(|b| *b == b'\n')
            .all(|l| l.ends_with(b"\r\n")),
        "every line still ends in CRLF"
    );
    assert_eq!(made.plan.files[0].nfs_hash, content_hash(&on_nfs));
}

// ---------------------------------------------------------------------------------------------------------- what is refused

#[tokio::test]
async fn paths_that_cannot_be_one_pull_request_are_refused_before_github_is_asked() {
    let Some(f) = fixture().await else { return };
    f.rig.tenant(&f.s, "sit2").await;
    f.rig.seed(
        &f.s,
        &[
            ("a/x-sit1.yml", "k: 1\n"),
            ("a/x-sit2.yml", "k: 2\n"),
            ("a/y.yml", "k: 3\n"),
        ],
    );
    let plan = |paths: &[&str], target| {
        let (svc, ctx, req) = (f.svc.clone(), f.ctx("pr-bad"), f.request(paths, target));
        async move { svc.plan(&ctx, &req).await.unwrap_err() }
    };
    assert_eq!(plan(&[], None).await, PrError::Target(TargetError::NoPaths));
    assert_eq!(
        plan(&["a/x-sit1.yml", "a/x-sit2.yml"], None).await,
        PrError::Target(TargetError::MixedDestinations)
    );
    assert_eq!(
        plan(
            &["a/y.yml"],
            Some(PrTarget::Tenant {
                branch: TenantId::parse("sit9").unwrap()
            })
        )
        .await,
        PrError::Target(TargetError::UnknownTenant)
    );
    assert_eq!(
        plan(&["a/y.yml", "a/y.yml"], None).await,
        PrError::Target(TargetError::DuplicatePath)
    );
    assert_eq!(
        f.server.requests(),
        0,
        "no call to GitHub, and so no token opened"
    );
}

#[tokio::test]
async fn pr_paths_are_capped_at_100() {
    let Some(f) = fixture().await else { return };
    let many: Vec<String> = (0..=100).map(|i| format!("a/f{i}.yml")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        f.svc
            .plan(&f.ctx("pr-100"), &f.request(&many, None))
            .await
            .unwrap_err(),
        PrError::Target(TargetError::TooManyPaths)
    );
    assert_eq!(f.server.requests(), 0);
}

#[tokio::test]
async fn a_pull_request_takes_a_bounded_amount_of_content() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    // Two files of 600 bytes against a limit of 1,000.
    let big = format!("k: {}\n", "v".repeat(600));
    f.rig
        .seed(&f.s, &[("a/x-sit1.yml", &big), ("a/y-sit1.yml", &big)]);
    let svc = f.rig.hub.pull_requests(PrDeps {
        vault: f.vault.clone(),
        git: Arc::new(GithubClient::for_tests(&f.server.base(), fast()).unwrap()),
        settings: PrSettings {
            repos: PolicyRepos {
                base: RepoRef::parse(BASE).unwrap(),
                tenant: RepoRef::parse(TENANT).unwrap(),
            },
            base_branch: BranchName::parse("master").unwrap(),
            max_total_bytes: 1_000,
        },
    });
    let one = svc
        .plan(&f.ctx("pr-cap"), &f.request(&["a/x-sit1.yml"], None))
        .await;
    assert!(one.is_ok(), "{one:?}");
    let both = svc
        .plan(
            &f.ctx("pr-cap-2"),
            &f.request(&["a/x-sit1.yml", "a/y-sit1.yml"], None),
        )
        .await
        .unwrap_err();
    assert_eq!(
        both,
        PrError::Rejected {
            index: 1,
            reason: RejectReason::TooLarge,
            issues: Vec::new()
        }
    );
}

#[tokio::test]
async fn denied_files_and_secrets_never_reach_github() {
    let Some(f) = fixture().await else { return };
    f.sim.branch(TENANT, "sit1", &[("data/config/x.yml", b"x: 1\n")]);
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
    f.rig.seed(
        &f.s,
        &[
            ("keys/server.pem", "pem"),
            ("a/leaky-sit1.yml", &format!("a: 1\nkey: {key}\n")),
        ],
    );

    let e = f
        .svc
        .plan(
            &f.ctx("pr-deny"),
            &f.request(&["keys/server.pem"], Some(PrTarget::Base)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e,
        PrError::Rejected {
            index: 0,
            reason: RejectReason::Denied,
            issues: Vec::new()
        }
    );

    let e = f
        .svc
        .create(&f.ctx("pr-secret"), &f.request(&["a/leaky-sit1.yml"], None))
        .await
        .unwrap_err();
    let PrError::Rejected {
        index,
        reason,
        issues,
    } = &e
    else {
        panic!("{e:?}")
    };
    assert_eq!((*index, *reason), (0, RejectReason::SecretDetected));
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].line.map(std::num::NonZeroU32::get), Some(2));
    assert!(!format!("{e} {e:?}").contains(&key), "lines and rules only");
    assert!(f.writes().is_empty(), "nothing was written to GitHub");
    assert!(
        !f.server.seen().iter().any(|r| r.everything().contains(&key)),
        "the secret never left the hub"
    );
}

#[tokio::test]
async fn a_file_that_is_already_there_is_left_out_and_nothing_to_change_is_an_error() {
    let Some(f) = fixture().await else { return };
    f.sim.branch(
        TENANT,
        "sit1",
        &[
            ("data/config/a/same.yml", b"k: 1\r\n"),
            ("data/config/a/diff.yml", b"k: 1\r\n"),
        ],
    );
    // `same` differs from the repository only in line endings: it is the same file.
    f.rig.seed(
        &f.s,
        &[("a/same-sit1.yml", "k: 1\n"), ("a/diff-sit1.yml", "k: 2\n")],
    );

    let plan = f
        .svc
        .plan(
            &f.ctx("pr-same"),
            &f.request(&["a/same-sit1.yml", "a/diff-sit1.yml"], None),
        )
        .await
        .unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].repo_path.as_str(), "data/config/a/diff.yml");
    assert_eq!(plan.unchanged, vec![nfs("a/same-sit1.yml")]);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.as_str().contains("data/config/a/same.yml") && w.as_str().contains("left out")),
        "{:?}",
        plan.warnings
    );

    let e = f
        .svc
        .create(&f.ctx("pr-same-2"), &f.request(&["a/same-sit1.yml"], None))
        .await
        .unwrap_err();
    assert_eq!(e, PrError::NothingToChange);
    assert!(f.writes().is_empty());
}

#[tokio::test]
async fn files_and_branches_that_are_not_there_are_refused() {
    let Some(f) = fixture().await else { return };
    f.sim.branch(BASE, "master", &[("config/x.yml", b"x: 1\n")]);
    // No branch `sit1` in the tenant repository.
    f.rig.seed(&f.s, &[("a/y-sit1.yml", "k: 1\n")]);
    assert_eq!(
        f.svc
            .plan(&f.ctx("pr-nb"), &f.request(&["a/y-sit1.yml"], None))
            .await
            .unwrap_err(),
        PrError::TargetBranchMissing
    );
    // A path that is not on NFS.
    assert_eq!(
        f.svc
            .plan(&f.ctx("pr-nf"), &f.request(&["a/gone.yml"], Some(PrTarget::Base)))
            .await
            .unwrap_err(),
        PrError::NotOnNfs { index: 0 }
    );
    // A swimlane that does not exist.
    let mut r = f.request(&["a/y-sit1.yml"], None);
    r.swimlane = SwimlaneId::parse("nowhere").unwrap();
    assert_eq!(
        f.svc.plan(&f.ctx("pr-ns"), &r).await.unwrap_err(),
        PrError::UnknownSwimlane
    );
}

#[tokio::test]
async fn something_that_is_not_a_regular_file_in_the_repository_is_refused() {
    let Some(f) = fixture().await else { return };
    f.sim.branch(BASE, "master", &[("config/a/x.yml", b"x: 1\n")]);
    f.rig.seed(&f.s, &[("a/x.yml", "x: 2\n")]);
    // GitHub answers a directory with a list.
    f.sim.inject(|seen| {
        (seen.method == "GET" && seen.path.contains("/contents/"))
            .then(|| Reply::json(200, r#"[{"type":"file","name":"x.yml"}]"#))
    });
    assert_eq!(
        f.svc
            .plan(&f.ctx("pr-dir"), &f.request(&["a/x.yml"], None))
            .await
            .unwrap_err(),
        PrError::RepoFileUnreadable { index: 0 }
    );
}

// ------------------------------------------------------------------------------------------------------------------ S4, S8

#[tokio::test]
async fn pr_uses_the_callers_own_token() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    f.svc
        .create(&f.ctx("pr-own"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    let seen = f.server.seen();
    assert!(seen.len() >= 8, "{} calls", seen.len());
    for r in &seen {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {}", tok(1)).as_str()),
            "{} {}",
            r.method,
            r.path
        );
    }

    // A second editor has a token of their own, and it is theirs that is used for their pull request.
    let other = f.rig.user(2, Role::Editor).await;
    f.gh.token(&tok(2), "hubot", &[BASE, TENANT]);
    f.sim.expect_token(&tok(2));
    f.vault
        .put(&ctx(&other, "k-token-2"), Secret::new(tok(2)))
        .await
        .unwrap();
    let before = f.server.requests();
    f.svc
        .create(&ctx(&other, "pr-own-2"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    let theirs = &f.server.seen()[before..];
    assert!(!theirs.is_empty());
    assert!(
        theirs
            .iter()
            .all(|r| r.header("authorization") == Some(format!("Bearer {}", tok(2)).as_str()))
    );
}

#[tokio::test]
async fn a_user_without_a_usable_token_cannot_raise_a_pull_request() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let tokenless = f.rig.user(3, Role::Editor).await;
    let e = f
        .svc
        .plan(&ctx(&tokenless, "pr-nt"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap_err();
    assert_eq!(e, PrError::Token(TokenError::NoToken));
    assert_eq!(f.server.requests(), 0);

    // A token past its expiry is not opened.
    sqlx::query("UPDATE github_credentials SET expires_at = to_timestamp(0)")
        .execute(&f.rig.pool)
        .await
        .unwrap();
    let e = f
        .svc
        .plan(&f.ctx("pr-exp"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap_err();
    assert_eq!(e, PrError::Token(TokenError::Expired));
    assert_eq!(f.server.requests(), 0);
}

#[tokio::test]
async fn viewers_pending_and_disabled_users_cannot_raise_a_pull_request() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let viewer = f.rig.user(4, Role::Viewer).await;
    let pending = f.rig.user_with_status(5, None, domain::UserStatus::Pending).await;
    let disabled = f
        .rig
        .user_with_status(6, Some(Role::Editor), domain::UserStatus::Disabled)
        .await;
    for (who, key) in [(&viewer, "pr-v"), (&pending, "pr-p"), (&disabled, "pr-d")] {
        let r = f.request(&["a/x-sit1.yml"], None);
        assert_eq!(
            f.svc.plan(&ctx(who, key), &r).await.unwrap_err(),
            PrError::Forbidden
        );
        assert_eq!(
            f.svc.create(&ctx(who, key), &r).await.unwrap_err(),
            PrError::Forbidden
        );
    }
    assert_eq!(f.server.requests(), 0);
}

// -------------------------------------------------------------------------------------------------------- resuming a step

#[tokio::test]
async fn a_branch_that_holds_something_else_is_taken_and_one_that_holds_the_same_tree_is_ours() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let plan = f
        .svc
        .plan(&f.ctx("pr-r"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();

    // The first try made the branch, and its answer was lost: the next try finds its own tree there.
    let first = f.svc.commit(&f.ctx("pr-r"), &plan).await.unwrap();
    let again = f.svc.commit(&f.ctx("pr-r"), &plan).await.unwrap();
    assert_eq!(again, first, "the branch from the first try is the answer");
    assert_eq!(f.sim.branches(TENANT).len(), 2);

    // A branch of that name that holds another tree is not ours.
    let mut other = plan.clone();
    other.branch = BranchName::parse("lanekeeper/sit1/ffffffffff").unwrap();
    f.sim.branch(
        TENANT,
        other.branch.as_str(),
        &[("data/config/a/x.yml", b"k: 99\n")],
    );
    assert_eq!(
        f.svc.commit(&f.ctx("pr-r"), &other).await.unwrap_err(),
        PrError::BranchTaken
    );
}

#[tokio::test]
async fn opening_twice_finds_the_pull_request_the_first_try_made() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let plan = f
        .svc
        .plan(&f.ctx("pr-o"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    let committed = f.svc.commit(&f.ctx("pr-o"), &plan).await.unwrap();
    let first = f.svc.open(&f.ctx("pr-o"), &plan, &committed).await.unwrap();
    let again = f.svc.open(&f.ctx("pr-o"), &plan, &committed).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(f.sim.pulls(TENANT).len(), 1, "one pull request, not two");
}

#[tokio::test]
async fn the_audit_id_is_put_in_the_body_once_the_event_exists() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let made = f
        .svc
        .create(&f.ctx("pr-a"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    assert!(
        f.sim.pulls(TENANT)[0]
            .body
            .contains("added when the pull request is recorded")
    );
    f.svc
        .note_audit(&f.ctx("pr-a"), &made.plan, &made.opened, AuditId::from(42))
        .await
        .unwrap();
    let body = &f.sim.pulls(TENANT)[0].body;
    assert!(
        body.contains("Audit event: 42") && !body.contains("added when"),
        "{body}"
    );
    assert!(body.starts_with("Please review."), "{body}");
}

// ------------------------------------------------------------------------------------------------- the GitHub leg (rule 5)

#[tokio::test]
async fn contents_without_bytes_are_fetched_as_a_blob() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\r\nm: 2\r\n")]);
    f.sim.contents_without_bytes_over(4);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 1\nm: 3\n")]);
    let made = f
        .svc
        .create(&f.ctx("pr-big"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    // The blob of the repository file was read to learn its form: the commit is CRLF.
    assert_eq!(
        f.sim
            .file(TENANT, made.committed.branch.as_str(), "data/config/a/x.yml")
            .unwrap(),
        b"k: 1\r\nm: 3\r\n"
    );
    assert!(f.server.seen().iter().any(|r| r.path.contains("/git/blobs/")));
}

#[tokio::test]
async fn a_file_name_cannot_change_the_request() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/we ird#name?.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/we ird#name?-sit1.yml", "k: 2\n")]);
    let made = f
        .svc
        .create(&f.ctx("pr-enc"), &f.request(&["a/we ird#name?-sit1.yml"], None))
        .await
        .unwrap();
    assert_eq!(
        f.sim
            .file(
                TENANT,
                made.committed.branch.as_str(),
                "data/config/a/we ird#name?.yml"
            )
            .unwrap(),
        b"k: 2\n"
    );
    let reads: Vec<String> = f
        .server
        .seen()
        .iter()
        .filter(|r| r.path.contains("/contents/"))
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(reads.len(), 1);
    assert!(
        reads[0].contains("/contents/data/config/a/we%20ird%23name%3F.yml?ref="),
        "{reads:?}"
    );
}

#[tokio::test]
async fn github_calls_time_out_and_back_off_on_rate_limit() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);

    // A rate limit on a read is waited out, once, and the read goes through.
    let limited = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    f.sim.inject({
        let limited = Arc::clone(&limited);
        move |seen| {
            (seen.method == "GET"
                && seen.path.contains("/git/ref/heads/")
                && limited.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0)
                .then(|| {
                    Reply::json(403, r#"{"message":"rate limit"}"#)
                        .with_header("x-ratelimit-remaining", "0")
                        .with_header("retry-after", "0")
                })
        }
    });
    let plan = f
        .svc
        .plan(&f.ctx("pr-rl"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    assert_eq!(
        limited.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one refusal, one retry"
    );

    // A rate limit that asks for longer than the call waits is an error, not a sleep.
    f.sim.inject(|seen| {
        (seen.method == "POST" && seen.path.ends_with("/git/trees"))
            .then(|| Reply::json(429, r#"{"message":"slow down"}"#).with_header("retry-after", "3600"))
    });
    assert_eq!(
        f.svc.commit(&f.ctx("pr-rl"), &plan).await.unwrap_err(),
        PrError::Github(GithubError::RateLimited)
    );

    // A blob that never arrives: each attempt times out, three attempts, then `Unavailable`. Making a blob twice is harmless.
    let g = fixture().await.unwrap();
    g.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    g.rig.seed(&g.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let plan = g
        .svc
        .plan(&g.ctx("pr-to"), &g.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    g.sim.inject(|seen| {
        (seen.method == "POST" && seen.path.ends_with("/git/blobs"))
            .then(|| Reply::json(201, "{}").after(Duration::from_millis(600)))
    });
    assert_eq!(
        g.svc.commit(&g.ctx("pr-to"), &plan).await.unwrap_err(),
        PrError::Github(GithubError::Unavailable)
    );
    assert_eq!(g.post_bodies("/git/blobs").len(), 3, "bounded: three attempts");
}

#[tokio::test]
async fn a_branch_or_a_pull_request_is_never_made_twice_by_a_retry() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: 1\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: 2\n")]);
    let plan = f
        .svc
        .plan(&f.ctx("pr-w"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();

    // The answer to the branch is a 502: the call is made once, because the branch may exist now.
    f.sim.inject(|seen| {
        (seen.method == "POST" && seen.path.ends_with("/git/refs"))
            .then(|| Reply::json(502, r#"{"message":"bad gateway"}"#))
    });
    assert_eq!(
        f.svc.commit(&f.ctx("pr-w"), &plan).await.unwrap_err(),
        PrError::Github(GithubError::Unavailable)
    );
    assert_eq!(
        f.post_bodies("/git/refs").len(),
        1,
        "no retry of a call that may have worked"
    );
    // Reads are retried.
    f.sim.inject(|seen| {
        (seen.method == "GET" && seen.path.contains("/git/ref/heads/"))
            .then(|| Reply::json(502, r#"{"message":"bad gateway"}"#))
    });
    let before = f.server.requests();
    assert!(
        f.svc
            .plan(&f.ctx("pr-w2"), &f.request(&["a/x-sit1.yml"], None))
            .await
            .is_err()
    );
    let reads = f.server.seen()[before..]
        .iter()
        .filter(|r| r.path.contains("/git/ref/heads/"))
        .count();
    assert_eq!(reads, 3, "a read is tried three times");
}

// ----------------------------------------------------------------------------------------------------------- S10, S21

#[tokio::test]
async fn the_token_and_the_content_are_in_no_log_and_no_error() {
    let Some(f) = fixture().await else { return };
    f.sim
        .branch(TENANT, "sit1", &[("data/config/a/x.yml", b"k: old-value\n")]);
    f.rig.seed(&f.s, &[("a/x-sit1.yml", "k: brand-new-value\n")]);
    let (capture, _guard) = log_capture::install();
    let made = f
        .svc
        .create(&f.ctx("pr-log"), &f.request(&["a/x-sit1.yml"], None))
        .await
        .unwrap();
    // The failures too: a refused token, a rate limit, a taken branch.
    f.sim
        .inject(|_| Some(Reply::json(401, r#"{"message":"Bad credentials"}"#)));
    let errors = [
        f.svc.commit(&f.ctx("pr-log"), &made.plan).await.unwrap_err(),
        f.svc
            .open(&f.ctx("pr-log"), &made.plan, &made.committed)
            .await
            .unwrap_err(),
    ];
    let text = capture.text();
    let everything = format!(
        "{text}\n{made:?}\n{errors:?}\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    for secret in [tok(1).as_str(), "brand-new-value", "old-value"] {
        assert!(!everything.contains(secret), "{secret} leaked");
    }
    assert_eq!(errors[0], PrError::Github(GithubError::Unauthorized));
}

#[tokio::test]
async fn pr_errors_say_nothing_of_the_input() {
    let samples = [
        PrError::Forbidden,
        PrError::UnknownSwimlane,
        PrError::Target(TargetError::MixedDestinations),
        PrError::Rejected {
            index: 3,
            reason: RejectReason::SecretDetected,
            issues: Vec::new(),
        },
        PrError::NotOnNfs { index: 1 },
        PrError::RepoFileUnreadable { index: 2 },
        PrError::NothingToChange,
        PrError::TargetBranchMissing,
        PrError::BranchTaken,
        PrError::Token(TokenError::Expired),
        PrError::Github(GithubError::Unprocessable),
        PrError::Unavailable,
    ];
    for e in samples {
        let text = format!("{e} {e:?}");
        assert!(text.len() < 200, "{text}");
        assert!(!text.contains('/') && !text.contains("github_pat"), "{text}");
    }
}
