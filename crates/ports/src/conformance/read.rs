use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AppName, BaselineState, BaselineStatus, CompareQuery, CompareRef, ContentSource, DriftQuery, DriftState,
    EffectiveQuery, FileEntry, FindingsQuery, GridQuery, JobRef, LogicalFile, NfsPath, Page, PathMappingRule,
    PickupState, ReverseMapping, Role, ServeRequest, ServiceRef, ServiceState, SettingPath, SettingsQuery,
    ShortText, SwimlaneId, SwimlaneSummary, SyncingInfo, TenantId, TenantSet, TextQuery, Timestamp, UserRef,
    UserStatus,
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

    /// Make the tenants deployed to `s` (a swimlane that was just seeded) exactly `tenants`, so that a file whose name ends in
    /// `-<tenant>` is that tenant's (`contract/08-web`, CG2). Returns `false` when the harness cannot name tenants, which is
    /// the default: the suite then holds the registry to the rules that hold for any tenant set, and not to the ones that
    /// name `sit1`.
    async fn set_tenants(&self, _s: &SwimlaneId, _tenants: &[TenantId]) -> bool {
        false
    }

    /// Make the services of `s` that `pending_restarts` lists exactly `services` (`contract/08-web`, `CG4a`). Returns `false` when
    /// the harness cannot, which is the default: the suite then checks the pickup counts against whatever the registry lists.
    async fn set_pending_restarts(&self, _s: &SwimlaneId, _services: Vec<ServiceState>) -> bool {
        false
    }

    /// Give `s` this tier and project name (`CG4a`). Returns `false` when the harness cannot, which is the default.
    async fn set_tier_and_project(
        &self,
        _s: &SwimlaneId,
        _tier: Option<ShortText>,
        _project: Option<ShortText>,
    ) -> bool {
        false
    }

    /// Give `s` this baseline (`CG4a`). Returns `false` when the harness cannot, which is the default.
    async fn set_baseline(&self, _s: &SwimlaneId, _baseline: BaselineStatus) -> bool {
        false
    }

    /// Open a sync window on `s`, or close the one that is open (`CG4a`). Returns `false` when the harness cannot, which is the
    /// default.
    async fn set_syncing(&self, _s: &SwimlaneId, _syncing: Option<SyncingInfo>) -> bool {
        false
    }
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
/// - `contract/08-web`, CG2: every file a listing returns names its `logical_file` (a base file its own path, a tenant file the
///   path without `-<tenant>`) and its `tenant` (none for a base file), a folder has neither, and, when the harness can name a
///   swimlane's tenants, `tx-core-sit1.yml` of tenant `sit1` is the logical file `tx-core.yml`, while `tx-core-sit10.yml` is a
///   base file. An implementation that forgets the two fields fails here.
/// - `contract/08-web`, `CG4a`: every summary of `swimlanes` states its baseline, and a baseline that is not confirmed names nobody
///   and no time, while a confirmed one says when; its `pickup_counts` list each pickup state that is not live at most once,
///   with a count above zero, and equal what `pending_restarts` lists for the swimlane. When the harness can script a swimlane,
///   the tier, project, baseline, sync window and pickup counts it was given come back as given.
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
    for summary in &list {
        let pending = r.pending_restarts(&viewer, &summary.id).await.unwrap();
        check_summary_additions(summary, &pending);
    }

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
    root.items.iter().for_each(check_logical_fields);
    let under_a = r.tree(&viewer, &s1, &nfs("a"), Page::default()).await.unwrap();
    under_a.items.iter().for_each(check_logical_fields);
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
        p.items.iter().for_each(check_logical_fields);
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
    logical_files(r).await;
    summary_additions(r).await;

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
        check_logical_fields(e);
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

/// What `logical_file` and `tenant` of one listed entry must be, whatever the tenants of its swimlane are (CG2): a folder has
/// neither; a file names its logical file; a base file (no tenant) is its own logical file; a file with a tenant has that
/// tenant as the suffix of its name, and its logical file is the name without it.
fn check_logical_fields(e: &FileEntry) {
    if e.is_dir {
        assert!(
            e.logical_file.is_none() && e.tenant.is_none(),
            "{}: a directory has no logical file and no tenant",
            e.path
        );
        return;
    }
    let Some(logical) = &e.logical_file else {
        panic!("{}: a file names its logical file", e.path);
    };
    let Some(tenant) = &e.tenant else {
        assert_eq!(
            logical.as_str(),
            e.path.as_str(),
            "{}: a base file is its own logical file",
            e.path
        );
        return;
    };
    let only = TenantSet::new([tenant.clone()]).unwrap();
    match PathMappingRule::default().reverse(&e.path, &only).unwrap() {
        ReverseMapping::Tenant {
            tenant: suffix,
            logical: without,
            ..
        } => {
            assert_eq!(
                &suffix, tenant,
                "{}: the tenant is the suffix of the name",
                e.path
            );
            assert_eq!(
                &without, logical,
                "{}: the logical file of a tenant file is its name without the suffix",
                e.path
            );
        }
        ReverseMapping::Base { .. } => panic!(
            "{}: names a tenant that its name does not end with ({tenant})",
            e.path
        ),
    }
}

/// Every entry of `prefix`, one page of two at a time.
async fn tree_entries<R: RegistryRead + ?Sized>(r: &R, s: &SwimlaneId, prefix: &NfsPath) -> Vec<FileEntry> {
    let viewer = active(1, Role::Viewer);
    let mut all = Vec::new();
    let mut cursor = None;
    for _ in 0..1000 {
        let page = r
            .tree(&viewer, s, prefix, Page::new(2, cursor.clone()).unwrap())
            .await
            .unwrap();
        all.extend(page.items);
        match page.next {
            Some(c) => cursor = Some(c),
            None => return all,
        }
    }
    panic!("tree paging did not end after 1,000 pages");
}

/// `logical_file` and `tenant` (CG2) on a swimlane with tenant files. Whatever the tenants are, every entry of every page
/// follows [`check_logical_fields`]. When the harness can name the tenants (`sit1`), the answers are the ones the path mapping
/// gives: `-sit1` before the extension is the tenant's suffix, `-sit10` is not, and the name is never split at its first
/// hyphen.
async fn logical_files<R: RegistryScenario + ?Sized>(r: &R) {
    let viewer = active(1, Role::Viewer);
    let s = swimlane("sit2");
    r.seed(
        &s,
        &files(&[
            ("tx-core.yml", "a: 1\n"),
            ("tx-core-sit1.yml", "a: 2\n"),
            ("tx-core-sit10.yml", "a: 3\n"),
            ("svc/app.yml", "b: 1\n"),
            ("svc/app-sit1.yml", "b: 2\n"),
        ]),
    )
    .await;
    let sit1 = TenantId::parse("sit1").unwrap();
    let named = r.set_tenants(&s, std::slice::from_ref(&sit1)).await;

    let mut entries = tree_entries(r, &s, &NfsPath::root()).await;
    entries.extend(tree_entries(r, &s, &nfs("svc")).await);
    assert!(
        entries.iter().any(|e| !e.is_dir) && entries.iter().any(|e| e.is_dir),
        "the suite seeded files and a folder"
    );
    entries.iter().for_each(check_logical_fields);
    if !named {
        return;
    }

    let expected: [(&str, Option<&str>, Option<&str>); 6] = [
        ("tx-core.yml", Some("tx-core.yml"), None),
        ("tx-core-sit1.yml", Some("tx-core.yml"), Some("sit1")),
        ("tx-core-sit10.yml", Some("tx-core-sit10.yml"), None),
        ("svc", None, None),
        ("svc/app.yml", Some("svc/app.yml"), None),
        ("svc/app-sit1.yml", Some("svc/app.yml"), Some("sit1")),
    ];
    for (path, logical, tenant) in expected {
        let e = entries
            .iter()
            .find(|e| e.path.as_str() == path)
            .unwrap_or_else(|| panic!("{path} is not listed"));
        let got = (
            e.logical_file.as_ref().map(LogicalFile::as_str),
            e.tenant.as_ref().map(TenantId::as_str),
        );
        let why = match tenant {
            Some(t) => format!("belongs to the tenant {t} (the suffix of its name); its logical file is the name without it"),
            None => "has no tenant of this swimlane in its name, so it has none, and a file is its own logical file".to_owned(),
        };
        assert_eq!(got, (logical, tenant), "{path} {why}");
    }

    let listed = r
        .swimlanes(&viewer)
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == s)
        .expect("a seeded swimlane is listed");
    assert_eq!(listed.tenants, [sit1], "swimlanes lists the tenants it was given");
}

/// What is true of the additions of one summary (`CG4a`), whatever the registry holds: the baseline is stated, and exclusive;
/// the pickup counts are the non-live states of `pending`, each once, none of them zero.
fn check_summary_additions(s: &SwimlaneSummary, pending: &[ServiceState]) {
    let id = &s.id;
    let Some(baseline) = &s.baseline else {
        panic!("{id}: swimlanes states the baseline of every swimlane");
    };
    match baseline.state {
        BaselineState::Confirmed => assert!(
            baseline.at.is_some(),
            "{id}: a confirmed baseline says when it was confirmed"
        ),
        BaselineState::Unconfirmed | BaselineState::InProgress => assert!(
            baseline.by.is_none() && baseline.at.is_none(),
            "{id}: a baseline that is not confirmed names nobody and no time"
        ),
    }
    assert!(
        s.pickup_counts
            .iter()
            .all(|(state, _)| *state != PickupState::Live),
        "{id}: pickup_counts lists no live state"
    );
    let expected: Vec<(PickupState, u32)> = PickupState::ALL
        .into_iter()
        .filter(|p| *p != PickupState::Live)
        .filter_map(|p| {
            let n = pending.iter().filter(|x| x.pickup == p).count();
            (n > 0).then(|| (p, u32::try_from(n).unwrap()))
        })
        .collect();
    let mut got = s.pickup_counts.clone();
    got.sort_by_key(|(state, _)| *state);
    assert_eq!(
        got, expected,
        "{id}: pickup_counts equal the pending restarts per state, each non-live state once and none at zero"
    );
}

fn pending_service(name: &str, pickup: PickupState) -> ServiceState {
    ServiceState {
        service: ServiceRef::new("csp", name).unwrap(),
        pickup,
        pending_since: None,
        live_by: None,
        notifications_enabled: true,
        cache_ttl_secs: 1200,
        pending_files: Vec::new(),
    }
}

/// The summary additions (`CG4a`) on a swimlane the harness scripts when it can. Every summary follows
/// [`check_summary_additions`] whatever the harness can do; the tier, project, baseline, sync window and pickup counts a
/// harness set come back as set, and the counts agree with `pending_restarts` while the services change.
async fn summary_additions<R: RegistryScenario + ?Sized>(r: &R) {
    let viewer = active(1, Role::Viewer);
    let s = swimlane("sit3");
    r.seed(&s, &files(&[("a.yml", "x: 1\n")])).await;
    let summary_of = || async {
        let list = r.swimlanes(&viewer).await.unwrap();
        let summary = list
            .into_iter()
            .find(|x| x.id == s)
            .expect("a seeded swimlane is listed");
        let pending = r.pending_restarts(&viewer, &s).await.unwrap();
        check_summary_additions(&summary, &pending);
        summary
    };

    // Services in four states: two need a notification or restart, one is live within its TTL, one is live.
    let pending = r
        .set_pending_restarts(
            &s,
            vec![
                pending_service("tx-api", PickupState::NeedsNotifyOrRestart),
                pending_service("tx-batch", PickupState::NeedsNotifyOrRestart),
                pending_service("tx-ui", PickupState::LiveWithinTtl),
                pending_service("tx-live", PickupState::Live),
            ],
        )
        .await;
    let got = summary_of().await;
    if pending {
        assert_eq!(
            got.pickup_counts,
            [
                (PickupState::LiveWithinTtl, 1),
                (PickupState::NeedsNotifyOrRestart, 2)
            ],
            "{s}: pickup_counts equal the pending restarts per state"
        );
        // One service picks the change up: the count follows, and a state that empties is no longer listed.
        r.set_pending_restarts(
            &s,
            vec![pending_service("tx-api", PickupState::NeedsNotifyOrRestart)],
        )
        .await;
        assert_eq!(
            summary_of().await.pickup_counts,
            [(PickupState::NeedsNotifyOrRestart, 1)],
            "{s}: pickup_counts equal the pending restarts per state"
        );
    }

    if r.set_tier_and_project(
        &s,
        Some(ShortText::parse("SIT").unwrap()),
        Some(ShortText::parse("Core Banking").unwrap()),
    )
    .await
    {
        let got = summary_of().await;
        assert_eq!(
            (
                got.tier.as_ref().map(ShortText::as_str),
                got.project.as_ref().map(ShortText::as_str)
            ),
            (Some("SIT"), Some("Core Banking")),
            "{s}: swimlanes lists the tier and project it was given"
        );
    }

    // An open adoption is not a confirmed baseline (D17): the state says so and names nobody.
    let adopting = BaselineStatus {
        state: BaselineState::InProgress,
        by: None,
        at: None,
    };
    if r.set_baseline(&s, adopting.clone()).await {
        assert_eq!(
            summary_of().await.baseline,
            Some(adopting),
            "{s}: swimlanes lists the baseline it was given"
        );
        let confirmed = BaselineStatus {
            state: BaselineState::Confirmed,
            by: Some(UserRef::from(&user(2, Some(Role::Admin), UserStatus::Active))),
            at: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
        };
        assert!(r.set_baseline(&s, confirmed.clone()).await);
        assert_eq!(
            summary_of().await.baseline,
            Some(confirmed),
            "{s}: swimlanes lists the baseline it was given"
        );
    }

    let window = SyncingInfo {
        job: JobRef::new("sync-job-1", "uid-1").unwrap(),
        since: Timestamp::from_unix_millis(1_700_000_001_000),
    };
    if r.set_syncing(&s, Some(window.clone())).await {
        assert_eq!(
            summary_of().await.syncing,
            Some(window),
            "{s}: swimlanes lists the sync window it was given"
        );
        assert!(r.set_syncing(&s, None).await);
        assert_eq!(
            summary_of().await.syncing,
            None,
            "{s}: a closed sync window is no longer listed"
        );
    }
}
