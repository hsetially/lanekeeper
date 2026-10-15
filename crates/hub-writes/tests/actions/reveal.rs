//! T10 (D79, S4, S10, S13, S14b): the reveal action and the refusal of writes to denied paths, through the real hub over a real
//! Postgres. A reveal shows one flagged value to an Editor or above, for a reason, with exactly one chained audit event that never
//! holds the value; a reveal that cannot be audited shows nothing; a user can reveal only so often; and no door of the write path
//! lets a change reach a denied file.
#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use domain::{
    DeleteRequest, PreviewChange, PreviewRequest, RejectReason, Role, SettingPath, ShortText, SwimlaneId,
    User, UserStatus, Via,
};
use hub_writes::{ProposalDesk as _, RevealDesk, RevealError, RevealRequest, WriteConfig};
use ports::conformance::sample::nfs;
use ports::{
    AuditAction, AuditActor, AuditVia, BlobStore as _, WriteError, WriteService, canonical_json, verify_chain,
};

use crate::support::rig::{Rig, at, ctx, edit, h};
use crate::support::withholding_registry::WithholdingRegistry;
use crate::{aws_key, rejected};

/// The file the reveal tests use. Line 3 holds the flagged value.
fn creds() -> String {
    format!(
        "app: demo\ncreds:\n  aws: {}\n  note: hello\nname: n\n",
        aws_key()
    )
}

fn ask(s: &SwimlaneId, path: &str, setting: &str, reason: Option<&str>) -> RevealRequest {
    RevealRequest {
        swimlane: s.clone(),
        path: nfs(path),
        setting: SettingPath::parse(setting).unwrap(),
        reason: reason.map(|r| ShortText::parse(r).unwrap()),
    }
}

struct Lane {
    rig: Rig,
    s: SwimlaneId,
    editor: User,
}

async fn lane() -> Option<Lane> {
    lane_with(WriteConfig::default()).await
}

async fn lane_with(config: WriteConfig) -> Option<Lane> {
    let rig = Rig::with_config(crate::support::pg::TestDb::ready().await?, config).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(
        &s,
        &[
            ("creds.yml", &creds()),
            ("plain.yml", "a: 1\nb: 2\n"),
            ("notes.txt", &format!("token: {}\n", aws_key())),
        ],
    );
    let editor = rig.user(1, Role::Editor).await;
    Some(Lane { rig, s, editor })
}

/// The canonical JSON of every event of the chain: what the audit log keeps.
async fn chain_text(rig: &Rig) -> String {
    let mut all = String::new();
    for entry in rig.chain().await {
        all.push_str(&String::from_utf8(canonical_json(&entry.event).unwrap()).unwrap());
        all.push('\n');
    }
    all
}

#[tokio::test]
async fn every_reveal_is_audited() {
    let Some(l) = lane().await else { return };
    let operator = l.rig.user(2, Role::Operator).await;
    let admin = l.rig.user(3, Role::Admin).await;
    let file_hash = h(&creds());

    // An Editor, with a reason.
    let revealed = l
        .rig
        .hub
        .reveal(
            &ctx(&l.editor, "r-1"),
            ask(
                &l.s,
                "creds.yml",
                "creds.aws",
                Some("rotating the staging key, ticket 4411"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(revealed.value.expose(), &aws_key());

    // Operators and admins can too; without a reason the event says the fixed one.
    for (who, key) in [(&operator, "r-2"), (&admin, "r-3")] {
        let again = l
            .rig
            .hub
            .reveal(&ctx(who, key), ask(&l.s, "creds.yml", "creds.aws", None))
            .await
            .unwrap();
        assert_eq!(again.value.expose(), &aws_key());
    }

    // Exactly one chained event for each, in order, with who, which file, which version, which setting and line, and why.
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 3, "one event for each reveal and none besides");
    verify_chain(&chain).unwrap();
    let expect = [
        (&l.editor, "req-r-1", "rotating the staging key, ticket 4411"),
        (&operator, "req-r-2", "masked_value_reveal"),
        (&admin, "req-r-3", "masked_value_reveal"),
    ];
    for (entry, (who, request, why)) in chain.iter().zip(expect) {
        let e = &entry.event;
        assert_eq!(e.action, AuditAction::FlaggedValueRevealed);
        assert_eq!(e.actor, AuditActor::User { user: who.id.clone() });
        assert_eq!(e.via, AuditVia::Ui);
        assert_eq!(e.swimlane.as_ref(), Some(&l.s));
        assert_eq!(e.path.as_ref().map(domain::NfsPath::as_str), Some("creds.yml"));
        assert_eq!(e.hash_before, Some(file_hash));
        assert_eq!(e.hash_after, None, "a reveal changes nothing");
        assert_eq!(
            e.diff.as_deref(),
            Some(format!("revealed setting creds.aws at line 3; reason: {why}").as_str())
        );
        assert_eq!(
            e.request_id.as_ref().map(domain::RequestId::as_str),
            Some(request)
        );
    }

    // The value is in none of them.
    assert!(!chain_text(&l.rig).await.contains(&aws_key()));

    // A reveal is not replayable: the same key again is another reveal and another event, and nothing is stored under the key
    // (the stored answer would be the secret).
    for _ in 0..2 {
        l.rig
            .hub
            .reveal(
                &ctx(&l.editor, "r-1"),
                ask(&l.s, "creds.yml", "creds.aws", Some("again")),
            )
            .await
            .unwrap();
    }
    assert_eq!(l.rig.chain().await.len(), 5);
    assert_eq!(l.rig.count("idempotency_keys").await, 0);
}

#[tokio::test]
async fn a_reveal_leaves_the_value_nowhere_in_the_database() {
    let Some(l) = lane().await else { return };
    l.rig
        .hub
        .reveal(
            &ctx(&l.editor, "r-1"),
            ask(&l.s, "creds.yml", "creds.aws", Some("looking")),
        )
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
    )
    .fetch_all(&l.rig.pool)
    .await
    .unwrap();
    assert!(tables.len() > 10, "{tables:?}");
    let mut searched = 0;
    for table in tables {
        let found: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM \"{table}\" t WHERE position($1 in t::text) > 0"
        ))
        .bind(aws_key())
        .fetch_one(&l.rig.pool)
        .await
        .unwrap();
        assert_eq!(found, 0, "the value is in a row of {table}");
        searched += 1;
    }
    assert!(searched > 10);
}

#[tokio::test]
async fn a_reveal_whose_audit_fails_returns_no_value() {
    let Some(l) = lane().await else { return };
    l.rig.audit.fail_next();
    let out = l
        .rig
        .hub
        .reveal(
            &ctx(&l.editor, "r-1"),
            ask(&l.s, "creds.yml", "creds.aws", Some("looking")),
        )
        .await;
    assert_eq!(out.unwrap_err(), RevealError::Unavailable);
    assert_eq!(l.rig.chain().await.len(), 0);
    // The next one works: nothing was left half done.
    l.rig
        .hub
        .reveal(&ctx(&l.editor, "r-2"), ask(&l.s, "creds.yml", "creds.aws", None))
        .await
        .unwrap();
    assert_eq!(l.rig.chain().await.len(), 1);
}

#[tokio::test]
async fn reveal_flags_only_scan_matches() {
    let Some(l) = lane().await else { return };
    let hub = &l.rig.hub;
    for (n, (path, setting)) in [
        // A setting the scan does not flag, in a file that has one it does.
        ("creds.yml", "creds.note"),
        ("creds.yml", "app"),
        // A group is not a value.
        ("creds.yml", "creds"),
        // A setting that is not there, a file that is not there, a file with no settings.
        ("creds.yml", "creds.nope"),
        ("missing.yml", "a"),
        ("notes.txt", "token"),
        ("plain.yml", "a"),
    ]
    .into_iter()
    .enumerate()
    {
        let out = hub
            .reveal(&ctx(&l.editor, &format!("r-{n}")), ask(&l.s, path, setting, None))
            .await;
        assert_eq!(out.unwrap_err(), RevealError::NotFound, "{path} {setting}");
    }
    assert_eq!(
        l.rig.chain().await.len(),
        0,
        "nothing was revealed, so nothing is audited"
    );
}

#[tokio::test]
async fn a_reveal_of_a_denied_or_withheld_file_is_not_found() {
    let Some(l) = lane().await else { return };
    // `private-keys.yml` matches a default glob; `tx/odd.yml` matches nothing the hub knows, but the registry withholds it (the
    // agent's own globs matched it). Both hold a flagged value in a structured shape.
    l.rig.seed(
        &l.s,
        &[
            ("creds.yml", &creds()),
            ("tx/odd.yml", &creds()),
            ("tx/private-keys.yml", &creds()),
        ],
    );
    let withholding = l.rig.hub_with(
        WriteConfig::default(),
        Some(Arc::new(WithholdingRegistry::new(
            l.rig.registry.clone(),
            vec![nfs("tx/odd.yml")],
        ))),
    );
    let out = l
        .rig
        .hub
        .reveal(
            &ctx(&l.editor, "r-1"),
            ask(&l.s, "tx/private-keys.yml", "creds.aws", None),
        )
        .await;
    assert_eq!(out.unwrap_err(), RevealError::NotFound);
    let out = withholding
        .reveal(&ctx(&l.editor, "r-2"), ask(&l.s, "tx/odd.yml", "creds.aws", None))
        .await;
    assert_eq!(out.unwrap_err(), RevealError::NotFound);
    assert_eq!(l.rig.chain().await.len(), 0);
}

/// A request that says the file is new (`Expected::Absent`) is no way round the registry's withholding: the file the agent's
/// own globs matched is there, so an upload, an edit or a careful user's edit that calls it new is refused as denied, and
/// nothing is sent to the agent, stored as a proposal or audited.
#[tokio::test]
async fn a_new_file_request_over_a_withheld_file_is_refused() {
    let Some(l) = lane().await else { return };
    let careful = l.rig.approval_user(2, Role::Editor).await;
    l.rig.agent.put(&l.s, &nfs("tx/odd.yml"), b"x: 1\n");
    l.rig.catch_up_registry(&l.s);
    let withheld = l.rig.hub_with(
        WriteConfig::default(),
        Some(Arc::new(WithholdingRegistry::new(
            l.rig.registry.clone(),
            vec![nfs("tx/odd.yml")],
        ))),
    );
    let odd = "tx/odd.yml";
    // The preview was issued by a hub that did not know the file was withheld.
    let pv = l
        .rig
        .hub_with(WriteConfig::default(), None)
        .preview_effect(
            &ctx(&l.editor, "n-p"),
            preview_of(&l.s, odd, upload_change(domain::Expected::Absent)),
        )
        .await
        .unwrap();
    let denied = || -> Result<domain::WriteOutcome, WriteError> { Ok(rejected(RejectReason::Denied)) };
    assert_eq!(
        withheld
            .upload(
                &ctx(&l.editor, "n-u"),
                upload_of(&l.s, odd, domain::Expected::Absent, Some(&pv))
            )
            .await,
        denied(),
        "upload"
    );
    assert_eq!(
        withheld
            .edit(
                &ctx(&l.editor, "n-e"),
                edit(&l.s, odd, domain::Expected::Absent, "x: 2\n")
            )
            .await,
        denied(),
        "edit"
    );
    assert_eq!(
        withheld
            .edit(
                &ctx(&careful, "n-c"),
                edit(&l.s, odd, domain::Expected::Absent, "x: 2\n")
            )
            .await,
        denied(),
        "careful edit"
    );
    assert_eq!(l.rig.agent.write_commands(), 0, "nothing was sent to the agent");
    assert_eq!(l.rig.count("proposals").await, 0);
    assert_eq!(l.rig.chain().await.len(), 0);
}

#[tokio::test]
async fn reveal_needs_an_editor_an_active_user_and_the_web_ui() {
    let Some(l) = lane().await else { return };
    let hub = &l.rig.hub;
    let viewer = l.rig.user(2, Role::Viewer).await;
    let pending = l.rig.user_with_status(3, None, UserStatus::Pending).await;
    let disabled = l
        .rig
        .user_with_status(4, Some(Role::Admin), UserStatus::Disabled)
        .await;
    for (who, key) in [(&viewer, "v-1"), (&pending, "p-1"), (&disabled, "d-1")] {
        let out = hub
            .reveal(&ctx(who, key), ask(&l.s, "creds.yml", "creds.aws", None))
            .await;
        assert_eq!(out.unwrap_err(), RevealError::Forbidden, "{:?}", who.role);
    }

    // The role is the directory's, not the one the request carries.
    let mut claims_admin = viewer.clone();
    claims_admin.role = Some(Role::Admin);
    let out = hub
        .reveal(
            &ctx(&claims_admin, "v-2"),
            ask(&l.s, "creds.yml", "creds.aws", None),
        )
        .await;
    assert_eq!(out.unwrap_err(), RevealError::Forbidden);

    // No tool reveals a flagged value (S14b): a call that came in over MCP is refused whatever the role.
    let admin = l.rig.user(5, Role::Admin).await;
    let mut over_mcp = ctx(&admin, "m-1");
    over_mcp.via = Via::Mcp;
    let out = hub
        .reveal(&over_mcp, ask(&l.s, "creds.yml", "creds.aws", None))
        .await;
    assert_eq!(out.unwrap_err(), RevealError::Forbidden);

    assert_eq!(l.rig.chain().await.len(), 0);
}

#[tokio::test]
async fn reveal_is_rate_limited() {
    let Some(l) = lane_with(WriteConfig {
        reveals_per_minute: 3,
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let hub = &l.rig.hub;
    let other = l.rig.user(2, Role::Editor).await;
    let go = |who: &User, key: &str| {
        let (c, r) = (ctx(who, key), ask(&l.s, "creds.yml", "creds.aws", None));
        async move { hub.reveal(&c, r).await }
    };

    for n in 0..3 {
        go(&l.editor, &format!("a-{n}")).await.unwrap();
    }
    let Err(RevealError::Throttled { retry_after_secs }) = go(&l.editor, "a-3").await else {
        panic!("the fourth reveal in a minute must be refused");
    };
    assert!((1..=60).contains(&retry_after_secs), "{retry_after_secs}");
    assert_eq!(
        l.rig.chain().await.len(),
        3,
        "a refused reveal is not audited and shows nothing"
    );

    // Another user has a limit of their own.
    go(&other, "b-0").await.unwrap();

    // The window slides with the clock: after a minute the first three are old.
    l.rig.advance_secs(30);
    assert!(matches!(
        go(&l.editor, "a-4").await,
        Err(RevealError::Throttled { .. })
    ));
    l.rig.advance_secs(31);
    go(&l.editor, "a-5").await.unwrap();
    assert_eq!(l.rig.chain().await.len(), 5);
}

#[tokio::test]
async fn attempts_that_find_nothing_count_toward_the_limit() {
    let Some(l) = lane_with(WriteConfig {
        reveals_per_minute: 2,
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let hub = &l.rig.hub;
    for n in 0..2 {
        let out = hub
            .reveal(
                &ctx(&l.editor, &format!("n-{n}")),
                ask(&l.s, "plain.yml", "a", None),
            )
            .await;
        assert_eq!(out.unwrap_err(), RevealError::NotFound);
    }
    // A probe that found nothing used the allowance, so the route cannot be used to hunt for secrets faster.
    let out = hub
        .reveal(&ctx(&l.editor, "n-2"), ask(&l.s, "creds.yml", "creds.aws", None))
        .await;
    assert!(matches!(out, Err(RevealError::Throttled { .. })), "{out:?}");

    // Those who may not reveal do not use anybody's allowance, their own included.
    let viewer = l.rig.user(2, Role::Viewer).await;
    for n in 0..5 {
        let out = hub
            .reveal(
                &ctx(&viewer, &format!("v-{n}")),
                ask(&l.s, "creds.yml", "creds.aws", None),
            )
            .await;
        assert_eq!(out.unwrap_err(), RevealError::Forbidden);
    }
}

#[tokio::test]
async fn concurrent_reveals_respect_the_limit() {
    let Some(l) = lane_with(WriteConfig {
        reveals_per_minute: 5,
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let hub = &l.rig.hub;
    let attempts = (0..20).map(|n| {
        let editor = l.editor.clone();
        let request = ask(&l.s, "creds.yml", "creds.aws", None);
        async move { hub.reveal(&ctx(&editor, &format!("c-{n}")), request).await }
    });
    let results = futures::future::join_all(attempts).await;
    let shown = results.iter().filter(|r| r.is_ok()).count();
    let throttled = results
        .iter()
        .filter(|r| matches!(r, Err(RevealError::Throttled { .. })))
        .count();
    assert_eq!((shown, throttled), (5, 15));
    assert_eq!(l.rig.chain().await.len(), 5);
}

#[tokio::test]
async fn the_limit_is_kept_by_each_replica() {
    let Some(l) = lane_with(WriteConfig {
        reveals_per_minute: 1,
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let second = l.rig.hub_with(
        WriteConfig {
            reveals_per_minute: 1,
            ..WriteConfig::default()
        },
        None,
    );
    let request = || ask(&l.s, "creds.yml", "creds.aws", None);
    l.rig.hub.reveal(&ctx(&l.editor, "a-1"), request()).await.unwrap();
    assert!(l.rig.hub.reveal(&ctx(&l.editor, "a-2"), request()).await.is_err());
    // Documented, not hidden: the table is in memory, so a second replica has its own (the threat model says so).
    second.reveal(&ctx(&l.editor, "a-3"), request()).await.unwrap();
}

#[tokio::test]
async fn a_reason_that_repeats_the_value_is_not_recorded() {
    let Some(l) = lane().await else { return };
    let hub = &l.rig.hub;
    let repeats = format!("the key is {}", aws_key());
    let secretish = format!("compare with {}", ["AKIA", "ABCDEFGHIJKLMNOP"].concat());
    for (n, reason) in [repeats.as_str(), secretish.as_str(), "   "]
        .into_iter()
        .enumerate()
    {
        hub.reveal(
            &ctx(&l.editor, &format!("r-{n}")),
            ask(&l.s, "creds.yml", "creds.aws", Some(reason)),
        )
        .await
        .unwrap();
    }
    let chain = l.rig.chain().await;
    assert_eq!(chain.len(), 3);
    for entry in &chain {
        let diff = entry.event.diff.as_deref().unwrap();
        assert!(diff.ends_with("reason: masked_value_reveal"), "{diff}");
    }
    let text = chain_text(&l.rig).await;
    assert!(
        !text.contains(&aws_key()) && !text.contains("ABCDEFGHIJKLMNOP"),
        "{text}"
    );
}

// ------------------------------------------------------------------------------------------------ denied paths (D79)

fn upload_change(expected: domain::Expected) -> PreviewChange {
    PreviewChange::Upload {
        expected,
        content: bytes::Bytes::from_static(b"x: 2\n"),
    }
}

fn preview_of(s: &SwimlaneId, path: &str, change: PreviewChange) -> PreviewRequest {
    PreviewRequest {
        swimlane: s.clone(),
        path: nfs(path),
        change,
    }
}

fn upload_of(
    s: &SwimlaneId,
    path: &str,
    expected: domain::Expected,
    preview: Option<&domain::ChangePreview>,
) -> domain::UploadRequest {
    domain::UploadRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        content: bytes::Bytes::from_static(b"x: 2\n"),
        preview: preview.map(|p| p.preview_id.clone()),
    }
}

fn delete_of(
    s: &SwimlaneId,
    path: &str,
    expected: domain::ContentHash,
    preview: Option<&domain::ChangePreview>,
) -> DeleteRequest {
    DeleteRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        preview: preview.map(|p| p.preview_id.clone()),
    }
}

fn revert_of(
    s: &SwimlaneId,
    path: &str,
    expected: domain::Expected,
    to: domain::ContentHash,
) -> domain::RevertRequest {
    domain::RevertRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        to,
        change_eol: None,
    }
}

/// Files that the hub's globs deny: the defaults at any depth and in any case, and the extra glob of the configuration.
const DENIED: [&str; 6] = [
    "certs/server.pem",
    "store.jks",
    "docs/private-notes.txt",
    "Certs/SERVER.PEM",
    "deep/er/keys/id.p12",
    "tx/legacy.vault",
];

/// No door of the write path puts a change into a denied file, or removes or restores one: not an edit, an upload, a delete, a
/// revert, a preview, a draft, a user who needs approval (who gets no proposal), the approval of a proposal made before the glob
/// existed, or a file that only the registry knows to withhold. Nothing is sent to the agent and nothing is audited.
#[tokio::test]
async fn writes_to_denied_paths_are_refused() {
    let Some(l) = lane_with(WriteConfig {
        extra_deny_globs: vec!["*.vault".to_owned()],
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    let careful = l.rig.approval_user(2, Role::Editor).await;
    let admin = l.rig.user(3, Role::Admin).await;
    for path in DENIED {
        l.rig.agent.put(&l.s, &nfs(path), b"x: 1\n");
    }
    l.rig.agent.put(&l.s, &nfs("tx/odd.yml"), b"x: 1\n");
    l.rig.catch_up_registry(&l.s);
    // The version a revert would restore is kept, so that a revert gets as far as the file it would change.
    l.rig
        .blobs
        .put(bytes::Bytes::from_static(b"x: 1\n"))
        .await
        .unwrap();
    let hub = &l.rig.hub;
    let current = at("x: 1\n");
    let hash = h("x: 1\n");
    let withheld = l.rig.hub_with(
        WriteConfig::default(),
        Some(Arc::new(WithholdingRegistry::new(
            l.rig.registry.clone(),
            vec![nfs("tx/odd.yml")],
        ))),
    );

    // The hub whose globs are the defaults plus `*.vault` knows `DENIED`.
    for (n, path) in DENIED.into_iter().enumerate() {
        let k = |what: &str| format!("{what}-{n}");
        let denied = || -> Result<domain::WriteOutcome, WriteError> { Ok(rejected(RejectReason::Denied)) };
        assert_eq!(
            hub.edit(&ctx(&l.editor, &k("e")), edit(&l.s, path, current, "x: 2\n"))
                .await,
            denied(),
            "edit {path}"
        );
        assert_eq!(
            hub.upload(&ctx(&l.editor, &k("u")), upload_of(&l.s, path, current, None))
                .await,
            denied(),
            "upload {path}"
        );
        assert_eq!(
            hub.delete(&ctx(&l.editor, &k("d")), delete_of(&l.s, path, hash, None))
                .await,
            denied(),
            "delete {path}"
        );
        assert_eq!(
            hub.revert(&ctx(&l.editor, &k("r")), revert_of(&l.s, path, current, hash))
                .await,
            denied(),
            "revert {path}"
        );
        // A user who needs approval is refused too, and no proposal is made for a change that could never be applied.
        assert_eq!(
            hub.edit(&ctx(&careful, &k("c")), edit(&l.s, path, current, "x: 2\n"))
                .await,
            denied(),
            "careful edit {path}"
        );
        // The previews and the drafts, which change nothing, still say no.
        for change in [upload_change(current), PreviewChange::Delete { expected: hash }] {
            let out = hub
                .preview_effect(&ctx(&l.editor, &k("p")), preview_of(&l.s, path, change))
                .await;
            assert_eq!(out.err(), Some(WriteError::BadRequest), "preview {path}");
        }
        let draft = hub
            .propose_draft(&ctx(&l.editor, &k("f")), edit(&l.s, path, current, "x: 2\n"))
            .await;
        assert_eq!(draft.err(), Some(WriteError::BadRequest), "draft {path}");
    }

    // A file that no glob of the hub matches but that the registry withholds (the agent's own globs matched it) is just as
    // closed. Its previews were issued by a hub that did not know, so the upload and the delete get as far as the file.
    let denied = || -> Result<domain::WriteOutcome, WriteError> { Ok(rejected(RejectReason::Denied)) };
    let odd = "tx/odd.yml";
    let pv_upload = l
        .rig
        .hub_with(WriteConfig::default(), None)
        .preview_effect(
            &ctx(&l.editor, "w-p1"),
            preview_of(&l.s, odd, upload_change(current)),
        )
        .await
        .unwrap();
    let pv_delete = l
        .rig
        .hub_with(WriteConfig::default(), None)
        .preview_effect(
            &ctx(&l.editor, "w-p2"),
            preview_of(&l.s, odd, PreviewChange::Delete { expected: hash }),
        )
        .await
        .unwrap();
    assert_eq!(
        withheld
            .edit(&ctx(&l.editor, "w-e"), edit(&l.s, odd, current, "x: 2\n"))
            .await,
        denied(),
        "edit {odd}"
    );
    assert_eq!(
        withheld
            .upload(
                &ctx(&l.editor, "w-u"),
                upload_of(&l.s, odd, current, Some(&pv_upload))
            )
            .await,
        denied(),
        "upload {odd}"
    );
    assert_eq!(
        withheld
            .delete(
                &ctx(&l.editor, "w-d"),
                delete_of(&l.s, odd, hash, Some(&pv_delete))
            )
            .await,
        denied(),
        "delete {odd}"
    );
    assert_eq!(
        withheld
            .revert(&ctx(&l.editor, "w-r"), revert_of(&l.s, odd, current, hash))
            .await,
        denied(),
        "revert {odd}"
    );
    assert_eq!(
        withheld
            .edit(&ctx(&careful, "w-c"), edit(&l.s, odd, current, "x: 2\n"))
            .await,
        denied(),
        "careful edit {odd}"
    );
    for change in [upload_change(current), PreviewChange::Delete { expected: hash }] {
        let out = withheld
            .preview_effect(&ctx(&l.editor, "w-p"), preview_of(&l.s, odd, change))
            .await;
        assert_eq!(out.err(), Some(WriteError::BadRequest), "preview {odd}");
    }
    let draft = withheld
        .propose_draft(&ctx(&l.editor, "w-f"), edit(&l.s, odd, current, "x: 2\n"))
        .await;
    assert_eq!(draft.err(), Some(WriteError::BadRequest), "draft {odd}");

    assert_eq!(l.rig.agent.writes_applied(), 0);
    assert_eq!(l.rig.agent.deletes_applied(), 0);
    assert_eq!(
        l.rig.agent.write_commands(),
        0,
        "nothing was even sent to the agent"
    );
    for path in DENIED.into_iter().chain(["tx/odd.yml"]) {
        assert_eq!(
            l.rig.nfs_of(&l.s, path).as_deref(),
            Some(&b"x: 1\n"[..]),
            "{path}"
        );
    }
    assert_eq!(l.rig.count("proposals").await, 0);
    assert_eq!(l.rig.count("drafts").await, 0);
    assert_eq!(l.rig.chain().await.len(), 0);

    // A proposal made before the glob existed is not applied by the approval that comes after it.
    l.rig.agent.put(&l.s, &nfs("later.vault"), b"x: 1\n");
    l.rig.catch_up_registry(&l.s);
    let lax = l.rig.hub_with(WriteConfig::default(), None);
    let made = lax
        .edit(
            &ctx(&careful, "late-1"),
            edit(&l.s, "later.vault", current, "x: 2\n"),
        )
        .await
        .unwrap();
    let domain::WriteOutcome::ProposalCreated { id } = made else {
        panic!("the lax hub proposes: {made:?}");
    };
    let out = hub.approve_proposal(&ctx(&admin, "late-2"), id, None).await;
    assert_eq!(out, Ok(rejected(RejectReason::Denied)));
    assert_eq!(l.rig.nfs_of(&l.s, "later.vault").as_deref(), Some(&b"x: 1\n"[..]));
    assert_eq!(l.rig.agent.writes_applied(), 0);

    // And so is a delete that was proposed before the glob existed.
    l.rig.agent.put(&l.s, &nfs("later2.vault"), b"x: 1\n");
    l.rig.catch_up_registry(&l.s);
    let pv = lax
        .preview_effect(
            &ctx(&careful, "late-3"),
            preview_of(&l.s, "later2.vault", PreviewChange::Delete { expected: hash }),
        )
        .await
        .unwrap();
    let made = lax
        .delete(
            &ctx(&careful, "late-4"),
            delete_of(&l.s, "later2.vault", hash, Some(&pv)),
        )
        .await
        .unwrap();
    let domain::WriteOutcome::ProposalCreated { id } = made else {
        panic!("the lax hub proposes the delete: {made:?}");
    };
    let out = hub.approve_proposal(&ctx(&admin, "late-5"), id, None).await;
    assert_eq!(out, Ok(rejected(RejectReason::Denied)));
    assert_eq!(
        l.rig.nfs_of(&l.s, "later2.vault").as_deref(),
        Some(&b"x: 1\n"[..])
    );
    assert_eq!(l.rig.agent.deletes_applied(), 0);

    let kinds: Vec<_> = l.rig.chain().await.iter().map(|e| e.event.action).collect();
    assert_eq!(
        kinds,
        [AuditAction::ProposalCreated, AuditAction::ProposalCreated],
        "only the two proposals were recorded"
    );
}
