//! What the fakes do with the queries of the `contract/07-mcp` change (G1 to G8), beyond what the conformance suites assert:
//! the registry fake applies a query to the answer it was scripted with, so a caller that forgets to pass its tenant, channel,
//! prefix or state shows in its own test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use domain::{
    AppName, ChannelName, CompareQuery, CompareRef, Comparison, ContentHash, DriftQuery, DriftState,
    EffectiveConfig, EffectiveQuery, EffectiveSetting, EffectiveSource, FileClass, FileEntry, FileKind,
    FileRole, LogicalFile, NfsPath, Page, Paged, Role, ServeRequest, ServedFile, SettingPath, ShortText,
    TenantId, TreeCompareEntry, TreeComparison, UserStatus,
};
use ports::conformance::sample::{active, nfs, swimlane, user};
use ports::fakes::FakeRegistry;
use ports::{ReadError, RegistryRead};

fn entry(path: &str, drift: DriftState, is_dir: bool) -> FileEntry {
    FileEntry {
        path: nfs(path),
        is_dir,
        kind: FileKind::Base,
        class: FileClass::Structured,
        role: FileRole::PropertySource,
        size: 3,
        hash: (!is_dir).then(|| ContentHash::from_bytes([1; 32])),
        drift,
        format: None,
        content_withheld: false,
        attribution: None,
        severity: None,
        pickup_state: None,
        logical_file: None,
        tenant: None,
    }
}

fn setting(path: &str) -> EffectiveSetting {
    EffectiveSetting {
        path: SettingPath::parse(path).unwrap(),
        value: "v".to_owned(),
        source: nfs("app/app.yml"),
        location: None,
    }
}

fn effective() -> EffectiveConfig {
    EffectiveConfig {
        swimlane: swimlane("sit1"),
        file: LogicalFile::parse("app/app.yml").unwrap(),
        tenant: TenantId::parse("sit0").unwrap(),
        channel: None,
        role: FileRole::PropertySource,
        hash: ContentHash::from_bytes([2; 32]),
        sources: Vec::new(),
        settings: ["a.b", "a.b.c", "a.b[0]", "a.bd", "a.c", "x"]
            .map(setting)
            .to_vec(),
        unresolved_placeholders: Vec::new(),
    }
}

fn seeded() -> FakeRegistry {
    let r = FakeRegistry::new();
    assert!(r.seed(
        &swimlane("sit1"),
        &[(nfs("app/app.yml"), Bytes::from_static(b"a: 1\n"))]
    ));
    r
}

#[tokio::test]
async fn effective_for_is_the_answer_for_the_tenant_channel_and_prefix_asked() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    let q = |prefix: Option<&str>| EffectiveQuery {
        file: LogicalFile::parse("app/other.yml").unwrap(),
        tenant: TenantId::parse("sit2").unwrap(),
        channel: Some(ChannelName::parse("web").unwrap()),
        setting_prefix: prefix.map(|p| SettingPath::parse(p).unwrap()),
    };
    assert_eq!(
        r.effective_for(&viewer, &s, &q(None)).await.unwrap_err(),
        ReadError::Unavailable,
        "nothing scripted"
    );
    r.set_effective(effective());

    let all = r.effective_for(&viewer, &s, &q(None)).await.unwrap();
    assert_eq!(all.settings.len(), 6, "no prefix, every setting");
    assert_eq!(
        all.tenant.as_str(),
        "sit2",
        "the tenant asked, not the scripted one"
    );
    assert_eq!(all.channel.as_ref().map(ChannelName::as_str), Some("web"));
    assert_eq!(all.file.as_str(), "app/other.yml");
    assert_eq!(all.swimlane, s);

    // At or below `a.b`: itself, `.c` and `[0]`; never `a.bd`, which only starts the same.
    let below = r.effective_for(&viewer, &s, &q(Some("a.b"))).await.unwrap();
    let paths: Vec<&str> = below.settings.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(paths, ["a.b", "a.b.c", "a.b[0]"]);
    let none = r.effective_for(&viewer, &s, &q(Some("nope"))).await.unwrap();
    assert!(none.settings.is_empty());

    assert_eq!(
        r.effective_for(&viewer, &swimlane("nowhere"), &q(None))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, None, UserStatus::Active),
    ] {
        assert_eq!(
            r.effective_for(&bad, &s, &q(None)).await.unwrap_err(),
            ReadError::Forbidden
        );
    }
}

fn comparison() -> Comparison {
    Comparison {
        left: "nfs:sit1".parse().unwrap(),
        right: "baseline:sit1".parse().unwrap(),
        file: None,
        class: FileClass::Structured,
        identical: true,
        eol_only: false,
        hunks: Vec::new(),
        setting_changes: Vec::new(),
        binary: None,
    }
}

#[tokio::test]
async fn compare_in_answers_for_the_references_file_and_channel_asked() {
    let (r, viewer) = (seeded(), active(1, Role::Viewer));
    let q = CompareQuery {
        left: "git:base@main".parse().unwrap(),
        right: "effective:sit1".parse().unwrap(),
        file: Some(LogicalFile::parse("app/app.yml").unwrap()),
        channel: Some(ChannelName::parse("web").unwrap()),
    };
    assert_eq!(
        r.compare_in(&viewer, &q).await.unwrap_err(),
        ReadError::Unavailable
    );
    r.set_comparison(comparison());
    let c = r.compare_in(&viewer, &q).await.unwrap();
    assert_eq!((c.left, c.right, c.file), (q.left, q.right, q.file));
}

fn tree() -> TreeComparison {
    use domain::CompareState::Different;
    let items = ["a/x.yml", "a/y.yml", "ab/z.yml", "b/w.yml"]
        .map(|f| TreeCompareEntry {
            file: LogicalFile::parse(f).unwrap(),
            state: Different,
        })
        .to_vec();
    TreeComparison {
        entries: Paged { items, next: None },
        skipped_identical_subtrees: 7,
    }
}

#[tokio::test]
async fn compare_tree_under_lists_the_files_below_the_prefix_a_page_at_a_time() {
    let (r, viewer) = (seeded(), active(1, Role::Viewer));
    let (left, right): (CompareRef, CompareRef) =
        ("nfs:sit1".parse().unwrap(), "baseline:sit1".parse().unwrap());
    let under = |prefix: &str, page: Page| {
        let (r, viewer, left, right, prefix) = (
            r.clone(),
            viewer.clone(),
            left.clone(),
            right.clone(),
            nfs(prefix),
        );
        async move { r.compare_tree_under(&viewer, left, right, &prefix, page).await }
    };
    assert_eq!(
        under("a", Page::default()).await.unwrap_err(),
        ReadError::Unavailable
    );
    r.set_tree_comparison(tree());

    let whole = under("a", Page::default()).await.unwrap();
    let files: Vec<&str> = whole.entries.items.iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, ["a/x.yml", "a/y.yml"], "`ab` only starts like `a`");
    assert_eq!(
        whole.skipped_identical_subtrees, 7,
        "the count of skipped subtrees is the comparison's"
    );
    assert!(whole.entries.next.is_none());

    // Two pages of one: every file once, in order, the cursor of the last absent.
    let first = under("a", Page::new(1, None).unwrap()).await.unwrap();
    assert_eq!(first.entries.items.len(), 1);
    let cursor = first.entries.next.expect("more below the prefix");
    let second = under("a", Page::new(1, Some(cursor)).unwrap()).await.unwrap();
    assert_eq!(second.entries.items[0].file.as_str(), "a/y.yml");
    assert!(second.entries.next.is_none());

    assert!(
        under("zzz", Page::default())
            .await
            .unwrap()
            .entries
            .items
            .is_empty()
    );
    assert_eq!(
        r.compare_tree_under(
            &user(2, None, UserStatus::Active),
            "nfs:sit1".parse().unwrap(),
            "nfs:sit1".parse().unwrap(),
            &nfs("a"),
            Page::default()
        )
        .await
        .unwrap_err(),
        ReadError::Forbidden
    );
}

#[tokio::test]
async fn drift_applies_the_state_the_prefix_the_order_and_the_page() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    let q = |state, prefix: Option<&str>| DriftQuery {
        state,
        prefix: prefix.map(nfs),
    };
    assert!(
        r.drift(&viewer, &s, &q(None, None), Page::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    r.set_drift(
        &s,
        vec![
            entry("b/two.yml", DriftState::Conflict, false),
            entry("a/one.yml", DriftState::NfsAhead, false),
            entry("a", DriftState::NfsAhead, true),
            entry("ab/x.yml", DriftState::NfsAhead, false),
            entry("a/deep/three.yml", DriftState::GitAhead, false),
            entry("a/same.yml", DriftState::InSync, false),
            entry("a/kept.yml", DriftState::IntentionalDivergence, false),
        ],
    );
    let paths = |p: &Paged<FileEntry>| {
        p.items
            .iter()
            .map(|e| e.path.as_str().to_owned())
            .collect::<Vec<_>>()
    };

    // Not in sync (intentional divergence is a divergence), files only, by path.
    let all = r
        .drift(&viewer, &s, &q(None, None), Page::default())
        .await
        .unwrap();
    assert_eq!(
        paths(&all),
        [
            "a/deep/three.yml",
            "a/kept.yml",
            "a/one.yml",
            "ab/x.yml",
            "b/two.yml"
        ]
    );
    // One state; and `in_sync` is a state that can be asked for.
    let ahead = r
        .drift(&viewer, &s, &q(Some(DriftState::NfsAhead), None), Page::default())
        .await
        .unwrap();
    assert_eq!(paths(&ahead), ["a/one.yml", "ab/x.yml"]);
    let same = r
        .drift(&viewer, &s, &q(Some(DriftState::InSync), None), Page::default())
        .await
        .unwrap();
    assert_eq!(paths(&same), ["a/same.yml"]);
    // A folder at any depth, whole components only.
    let below = r
        .drift(&viewer, &s, &q(None, Some("a")), Page::default())
        .await
        .unwrap();
    assert_eq!(paths(&below), ["a/deep/three.yml", "a/kept.yml", "a/one.yml"]);
    let both = r
        .drift(
            &viewer,
            &s,
            &q(Some(DriftState::GitAhead), Some("a")),
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!(paths(&both), ["a/deep/three.yml"]);

    // Pages of two: every file once, in order.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = r
            .drift(&viewer, &s, &q(None, None), Page::new(2, cursor.clone()).unwrap())
            .await
            .unwrap();
        assert!(page.items.len() <= 2);
        seen.extend(paths(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, paths(&all));

    // A new seed forgets the scripted drift, like it forgets everything else about the swimlane.
    assert!(r.seed(&s, &[(nfs("a.yml"), Bytes::from_static(b"k: v\n"))]));
    assert!(
        r.drift(&viewer, &s, &q(None, None), Page::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn auto_notify_is_off_until_it_is_switched_on_for_that_swimlane() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    assert!(r.seed(&swimlane("sit2"), &[]));
    assert!(!r.auto_notify(&viewer, &s).await.unwrap());
    r.set_auto_notify(&s, true);
    assert!(r.auto_notify(&viewer, &s).await.unwrap());
    assert!(
        !r.auto_notify(&viewer, &swimlane("sit2")).await.unwrap(),
        "per swimlane"
    );
    r.set_auto_notify(&s, false);
    assert!(!r.auto_notify(&viewer, &s).await.unwrap());
}

fn served() -> ServedFile {
    ServedFile {
        swimlane: swimlane("sit0"),
        application: AppName::parse("scripted").unwrap(),
        tenant: TenantId::parse("sit0").unwrap(),
        channel: None,
        file: nfs("scripted.yml"),
        role: FileRole::PropertySource,
        hash: ContentHash::from_bytes([3; 32]),
        content_withheld: false,
        text: Some("a: b\n".to_owned()),
        sources: vec![EffectiveSource {
            path: nfs("app/app.yml"),
            rank: 1,
        }],
        unresolved_placeholders: vec![ShortText::parse("${X}").unwrap()],
    }
}

#[tokio::test]
async fn served_answers_the_request_asked_and_is_unavailable_without_an_agent() {
    let (r, s, viewer) = (seeded(), swimlane("sit1"), active(1, Role::Viewer));
    let req = ServeRequest {
        application: AppName::parse("tx-api").unwrap(),
        tenant: TenantId::parse("sit2").unwrap(),
        channel: Some(ChannelName::parse("web").unwrap()),
        file: nfs("tx-api/core.yml"),
    };
    assert_eq!(
        r.served(&viewer, &s, &req).await.unwrap_err(),
        ReadError::Unavailable,
        "no file scripted: the agent could not fetch one"
    );
    r.set_served(served());
    let f = r.served(&viewer, &s, &req).await.unwrap();
    assert_eq!(
        (&f.swimlane, &f.application, &f.tenant, &f.channel, &f.file),
        (&s, &req.application, &req.tenant, &req.channel, &req.file)
    );
    assert_eq!(
        f.text.as_deref(),
        Some("a: b\n"),
        "the scripted content is what is served"
    );
    let _: NfsPath = f.file;
}
