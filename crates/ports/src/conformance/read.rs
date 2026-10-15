use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AppName, CompareQuery, CompareRef, ContentSource, DriftQuery, DriftState, EffectiveQuery, FindingsQuery,
    GridQuery, LogicalFile, NfsPath, Page, Role, ServeRequest, SettingPath, SettingsQuery, ShortText,
    SwimlaneId, TenantId, TextQuery, UserStatus,
};

use super::sample::{active, files, nfs, swimlane, user};
use crate::{ReadError, RegistryRead, content_hash};

fn text_query(pattern: &str) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex: false,
        case_sensitive: true,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: TextQuery::DEFAULT_MAX_TOTAL,
        max_per_file: TextQuery::DEFAULT_MAX_PER_FILE,
    }
}

/// What the registry conformance suite needs from the test harness.
#[async_trait]
pub trait RegistryScenario: RegistryRead {
    /// Make `s` exist with exactly these files, each with its content as the current NFS version.
    async fn seed(&self, s: &SwimlaneId, files: &[(NfsPath, Bytes)]);
}

/// - Pending, disabled and role-less users are `Forbidden` on every method (S4).
/// - Unknown swimlanes are `NotFound`.
/// - `tree` lists the direct children of a prefix ordered by path, directories included, and paging by
///   cursor yields every entry exactly once and never more than `limit` per page.
/// - `content` returns the exact bytes and hash; `history` lists the current version first.
/// - `grid` refuses more than 64 swimlanes.
/// - The methods of `contract/07-mcp` (`effective_for`, `compare_in`, `compare_tree_under`, `drift`, `auto_notify`, `served`)
///   are `Forbidden` for the same users; the ones that name a swimlane answer `NotFound` for an unknown one; `auto_notify` is
///   off for a swimlane nobody switched on (D85); `drift` lists files only, none in sync unless that state was asked for, in
///   one state when one was asked for, below the prefix when there is one, ordered by path, and a page of it never exceeds its
///   limit and visits every file exactly once. An implementation that keeps the `Unavailable` default of these fails here.
pub async fn registry_read<R: RegistryScenario + ?Sized>(r: &R) {
    let viewer = active(1, Role::Viewer);
    let s1 = swimlane("sit1");
    let seeded = files(&[
        ("a/one.yml", "x: 1\r\n"),
        ("a/two.yml", "y: 2\n"),
        ("a/deep/five.yml", "z: 5\n"),
        ("b/three.yml", "w: 3\n"),
        ("four.properties", "k=v\n"),
    ]);
    r.seed(&s1, &seeded).await;

    // Access.
    let page = Page::default();
    let any_file = nfs("a/one.yml");
    let logical = LogicalFile::parse("a/one.yml").unwrap();
    let query = FindingsQuery {
        swimlane: None,
        kinds: Vec::new(),
        min_severity: None,
        page: Page::default(),
    };
    for bad in [
        user(2, Some(Role::Admin), UserStatus::Pending),
        user(3, Some(Role::Admin), UserStatus::Disabled),
        user(4, None, UserStatus::Active),
    ] {
        assert_eq!(r.swimlanes(&bad).await.unwrap_err(), ReadError::Forbidden);
        assert_eq!(
            r.tree(&bad, &s1, &NfsPath::root(), page.clone())
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.content(&bad, &s1, &any_file, ContentSource::Nfs)
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.history(&bad, &s1, &any_file, page.clone()).await.unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.findings(&bad, query.clone()).await.unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.effective(&bad, &s1, &logical).await.unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.pending_restarts(&bad, &s1).await.unwrap_err(),
            ReadError::Forbidden
        );
        let left: CompareRef = "nfs:sit1".parse().unwrap();
        let right: CompareRef = "baseline:sit1".parse().unwrap();
        assert_eq!(
            r.compare(&bad, left.clone(), right.clone(), Some(logical.clone()))
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.compare_tree(&bad, left, right, page.clone()).await.unwrap_err(),
            ReadError::Forbidden
        );
        let grid = GridQuery {
            file: logical.clone(),
            swimlanes: vec![s1.clone()],
            channel: None,
            only_differing: false,
            page: page.clone(),
        };
        assert_eq!(r.grid(&bad, grid).await.unwrap_err(), ReadError::Forbidden);
        let settings = SettingsQuery {
            path_contains: ShortText::parse("timeout").unwrap(),
            swimlanes: Vec::new(),
            file: None,
            page: page.clone(),
        };
        assert_eq!(
            r.search_settings(&bad, settings).await.unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.search_text(&bad, text_query("needle")).await.unwrap_err(),
            ReadError::Forbidden
        );
        // `contract/07-mcp`.
        assert_eq!(
            r.effective_for(&bad, &s1, &effective_query(&logical))
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        let (l, rt): (CompareRef, CompareRef) =
            ("nfs:sit1".parse().unwrap(), "baseline:sit1".parse().unwrap());
        let compare = CompareQuery {
            left: l.clone(),
            right: rt.clone(),
            file: Some(logical.clone()),
            channel: None,
        };
        assert_eq!(
            r.compare_in(&bad, &compare).await.unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.compare_tree_under(&bad, l, rt, &nfs("a"), page.clone())
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(
            r.drift(&bad, &s1, &drift_query(None, None), page.clone())
                .await
                .unwrap_err(),
            ReadError::Forbidden
        );
        assert_eq!(r.auto_notify(&bad, &s1).await.unwrap_err(), ReadError::Forbidden);
        assert_eq!(
            r.served(&bad, &s1, &serve_request(&any_file)).await.unwrap_err(),
            ReadError::Forbidden
        );
    }

    // Swimlanes.
    let list = r.swimlanes(&viewer).await.unwrap();
    assert!(list.iter().any(|s| s.id == s1));
    assert!(list.windows(2).all(|w| w[0].id < w[1].id), "ordered by id");

    // Unknown swimlane.
    let nowhere = swimlane("nowhere");
    assert_eq!(
        r.tree(&viewer, &nowhere, &NfsPath::root(), page.clone())
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.content(&viewer, &nowhere, &any_file, ContentSource::Nfs)
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.pending_restarts(&viewer, &nowhere).await.unwrap_err(),
        ReadError::NotFound
    );
    // `contract/07-mcp`: the same unknown swimlane, by every method that names one.
    let logical = LogicalFile::parse("a/one.yml").unwrap();
    assert_eq!(
        r.effective_for(&viewer, &nowhere, &effective_query(&logical))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.drift(&viewer, &nowhere, &drift_query(None, None), page.clone())
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.auto_notify(&viewer, &nowhere).await.unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.served(&viewer, &nowhere, &serve_request(&any_file))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );

    // Tree: direct children, ordered, directories included.
    let root = r
        .tree(&viewer, &s1, &NfsPath::root(), Page::new(500, None).unwrap())
        .await
        .unwrap();
    let names: Vec<_> = root
        .items
        .iter()
        .map(|e| (e.path.as_str().to_owned(), e.is_dir))
        .collect();
    assert_eq!(
        names,
        [
            ("a".to_owned(), true),
            ("b".to_owned(), true),
            ("four.properties".to_owned(), false)
        ]
    );
    assert!(root.next.is_none());
    let under_a = r.tree(&viewer, &s1, &nfs("a"), Page::default()).await.unwrap();
    let names: Vec<_> = under_a.items.iter().map(|e| e.path.as_str().to_owned()).collect();
    assert_eq!(names, ["a/deep", "a/one.yml", "a/two.yml"]);
    let file = under_a.items.iter().find(|e| e.path == any_file).unwrap();
    assert_eq!(file.hash, Some(content_hash(b"x: 1\r\n")));
    assert_eq!(file.size, b"x: 1\r\n".len() as u64);
    assert!(under_a.items.iter().find(|e| e.is_dir).unwrap().hash.is_none());

    // Paging: two at a time visits every child once.
    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..10 {
        let p = r
            .tree(
                &viewer,
                &s1,
                &NfsPath::root(),
                Page::new(2, cursor.clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(p.items.len() <= 2, "a page never exceeds its limit");
        seen.extend(p.items.iter().map(|e| e.path.as_str().to_owned()));
        match p.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        ["a", "b", "four.properties"],
        "every entry exactly once, in order"
    );

    // Content and history.
    let content = r
        .content(&viewer, &s1, &any_file, ContentSource::Nfs)
        .await
        .unwrap();
    assert_eq!(
        content.bytes.as_deref(),
        Some(&b"x: 1\r\n"[..]),
        "bytes are not normalised"
    );
    assert_eq!(content.hash, content_hash(b"x: 1\r\n"));
    assert_eq!(
        r.content(&viewer, &s1, &nfs("a/missing.yml"), ContentSource::Nfs)
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    let history = r.history(&viewer, &s1, &any_file, Page::default()).await.unwrap();
    assert_eq!(
        history.items.first().map(|v| v.hash),
        Some(content_hash(b"x: 1\r\n"))
    );

    drift_and_auto_notify(r, &s1).await;

    // Limits.
    let too_many = GridQuery {
        file: logical,
        swimlanes: (0..=GridQuery::MAX_SWIMLANES).map(|_| s1.clone()).collect(),
        channel: None,
        only_differing: true,
        page: Page::default(),
    };
    assert_eq!(
        r.grid(&viewer, too_many).await.unwrap_err(),
        ReadError::BadRequest
    );
    let long = "n".repeat(TextQuery::MAX_PATTERN_BYTES + 1);
    assert_eq!(
        r.search_text(&viewer, text_query(&long)).await.unwrap_err(),
        ReadError::BadRequest
    );
    assert!(r.search_text(&viewer, text_query("needle")).await.is_ok());
    assert!(r.pending_restarts(&viewer, &s1).await.unwrap().is_empty());
}

fn effective_query(file: &LogicalFile) -> EffectiveQuery {
    EffectiveQuery {
        file: file.clone(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        setting_prefix: Some(SettingPath::parse("x").unwrap()),
    }
}

fn drift_query(state: Option<DriftState>, prefix: Option<&str>) -> DriftQuery {
    DriftQuery {
        state,
        prefix: prefix.map(nfs),
    }
}

fn serve_request(file: &NfsPath) -> ServeRequest {
    ServeRequest {
        application: AppName::parse("tx-api").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: file.clone(),
    }
}

/// Every file `drift` lists under `query`, one page of `size` at a time, in the order the pages come. Each page is at most
/// `size` long, and the cursor of the last page is absent.
async fn drift_pages<R: RegistryRead + ?Sized>(
    r: &R,
    s: &SwimlaneId,
    query: &DriftQuery,
    size: u32,
) -> Vec<domain::FileEntry> {
    let viewer = active(1, Role::Viewer);
    let mut all = Vec::new();
    let mut cursor = None;
    for _ in 0..1000 {
        let page = r
            .drift(&viewer, s, query, Page::new(size, cursor.clone()).unwrap())
            .await
            .unwrap();
        assert!(
            page.items.len() <= size as usize,
            "a page never exceeds its limit"
        );
        all.extend(page.items);
        match page.next {
            Some(c) => cursor = Some(c),
            None => return all,
        }
    }
    panic!("drift paging did not end after 1,000 pages");
}

/// `drift` and `auto_notify`, which every implementation can answer for a swimlane that was just seeded. What the drift of the
/// seeded files is depends on the implementation (it has a baseline and a Git, or it has not); what is true of every answer is
/// checked, and a fake that was given no drift answers an empty list.
async fn drift_and_auto_notify<R: RegistryScenario + ?Sized>(r: &R, s1: &SwimlaneId) {
    let viewer = active(1, Role::Viewer);

    // Off until an admin switches it on (D85).
    assert!(
        !r.auto_notify(&viewer, s1).await.unwrap(),
        "auto-notify is off for a swimlane nobody switched on"
    );

    // Not in sync, files only, ordered by path, once each.
    let everything = drift_pages(r, s1, &drift_query(None, None), 500).await;
    let paths: Vec<&str> = everything.iter().map(|e| e.path.as_str()).collect();
    assert!(
        paths.windows(2).all(|w| w[0] < w[1]),
        "ordered by path, none twice: {paths:?}"
    );
    for e in &everything {
        assert!(!e.is_dir, "{}: drift lists files, not folders", e.path);
        assert_ne!(
            e.drift,
            DriftState::InSync,
            "{}: a file in sync is not drift",
            e.path
        );
    }
    // One page at a time visits the same files in the same order.
    let by_ones = drift_pages(r, s1, &drift_query(None, None), 1).await;
    assert_eq!(
        by_ones.iter().map(|e| &e.path).collect::<Vec<_>>(),
        everything.iter().map(|e| &e.path).collect::<Vec<_>>(),
        "every file exactly once, in order, whatever the page size"
    );

    // A state, a prefix: only those, and never more than the whole.
    for state in DriftState::ALL {
        let of_state = drift_pages(r, s1, &drift_query(Some(state), None), 500).await;
        assert!(
            of_state.iter().all(|e| e.drift == state && !e.is_dir),
            "{state:?}: only files in that state"
        );
        if state != DriftState::InSync {
            assert!(
                of_state
                    .iter()
                    .all(|e| everything.iter().any(|x| x.path == e.path)),
                "{state:?}: a file in a drift state is in the list of the files that are not in sync"
            );
        }
    }
    let below_a = drift_pages(r, s1, &drift_query(None, Some("a")), 500).await;
    assert!(
        below_a.iter().all(|e| e.path.as_str().starts_with("a/")),
        "only files below the prefix, at any depth: {below_a:?}"
    );
    let nothing_here = drift_pages(r, s1, &drift_query(None, Some("no/such/folder")), 500).await;
    assert!(
        nothing_here.is_empty(),
        "a folder with no files is an empty list, not an error"
    );
}
