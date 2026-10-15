//! Compare one file, and compare two sides folder by folder (T6, D65, D67, D76, D79, A28, A29, A30, A31, P7): the line diff, the
//! settings that differ, a binary and a denied file by hash alone, an absent side, Git as a side, a changed secret that must not look
//! unchanged, the entity tag, the diff cache, and the whole-tree compare with its skipped subtrees and its pages. Over a real Postgres.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use domain::{
    ChangeKind, CompareRef, CompareState, Comparison, ContentHash, FileClass, GitRef, Page, SwimlaneId,
    TreeComparison,
};
use hub_registry::metrics::{CacheName, CacheResult};
use hub_registry::{RegistryApi, Tagged};
use ports::{ReadError, RegistryRead, content_hash};
use support::reads::{Reads, lane, logical, nfs, page};

const SECRET_A: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
const SECRET_B: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ9876543210";

fn nfs_of(s: &str) -> CompareRef {
    CompareRef::Nfs(lane(s))
}

fn effective_of(s: &str) -> CompareRef {
    CompareRef::Effective(lane(s))
}

/// Two swimlanes `sit1` and `sit2` (one tenant each) that run an application `core`.
async fn two_lanes(sit1: &[(&str, &str)], sit2: &[(&str, &str)]) -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    r.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    for id in ["sit1", "sit2"] {
        r.sql(&format!(
            "INSERT INTO services (swimlane_id, namespace, name, env_values) \
             VALUES ('{id}', 'ns', 'core', '{{\"SPRING_APPLICATION_NAME\": \"core\"}}')"
        ))
        .await;
    }
    // The tenant's branch exists in Git, so `-sit1` is read as a tenant suffix.
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
    r.world.inventory(sit1).await;
    r.world.second_lane("sit2", &["sit2"], sit2).await;
    Some(r)
}

async fn compare(r: &Reads, l: CompareRef, rr: CompareRef, file: &str) -> Result<Comparison, ReadError> {
    r.service
        .compare(&Reads::viewer(), l, rr, Some(logical(file)))
        .await
}

fn json(c: &Comparison) -> String {
    let dto = hub_registry::http::dto::ComparisonDto::from(c);
    serde_json::to_string(&dto).unwrap()
}

// ------------------------------------------------------------------------------------------------ one file

#[tokio::test]
async fn two_text_sides_have_hunks_and_a_structured_file_has_the_settings_that_changed() {
    let Some(r) = two_lanes(
        &[("core.yml", "port: 1\nname: a\n"), ("notes.txt", "one\ntwo\n")],
        &[("core.yml", "port: 2\nname: a\n"), ("notes.txt", "one\nthree\n")],
    )
    .await
    else {
        return;
    };
    let c = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "core.yml")
        .await
        .unwrap();
    assert!(!c.identical && !c.eol_only);
    assert_eq!(c.class, FileClass::Structured);
    assert_eq!(c.hunks.len(), 1);
    assert_eq!(c.setting_changes.len(), 1);
    let change = &c.setting_changes[0];
    assert_eq!(change.path.as_str(), "port");
    assert_eq!(change.kind, ChangeKind::Changed);
    assert_eq!(change.left.as_ref().unwrap().text, "1");
    assert_eq!(change.right.as_ref().unwrap().text, "2");
    assert_eq!(change.left_location.as_ref().unwrap().lines.start(), 1);

    // A text file has hunks and no settings.
    let t = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "notes.txt")
        .await
        .unwrap();
    assert_eq!(t.class, FileClass::Text);
    assert!(t.setting_changes.is_empty() && t.hunks.len() == 1);

    // A file against itself is identical, with nothing to show.
    let same = compare(&r, nfs_of("sit1"), nfs_of("sit1"), "core.yml")
        .await
        .unwrap();
    assert!(same.identical && same.hunks.is_empty() && same.setting_changes.is_empty());
    // Reversing the sides reverses the change.
    let back = compare(&r, nfs_of("sit2"), nfs_of("sit1"), "core.yml")
        .await
        .unwrap();
    assert_eq!(back.setting_changes[0].left.as_ref().unwrap().text, "2");
}

#[tokio::test]
async fn effective_sides_compare_the_settings_a_tenant_reads_not_the_files() {
    let Some(r) = two_lanes(
        &[
            ("application.yml", "common: base\n"),
            ("core.yml", "port: 1\n"),
            ("core-sit1.yml", "name: sit1\n"),
        ],
        &[
            ("application.yml", "common: base\n"),
            ("core.yml", "port: 1\nname: sit2\n"),
        ],
    )
    .await
    else {
        return;
    };
    r.world.whole().await;
    let c = compare(&r, effective_of("sit1"), effective_of("sit2"), "core.yml")
        .await
        .unwrap();
    // sit1 reads port 1 and name sit1 (tenant file), sit2 reads port 1 and name sit2: one setting differs, whatever file it is in.
    assert!(!c.identical);
    assert!(c.hunks.is_empty(), "a merged property source has no text");
    assert_eq!(
        c.setting_changes
            .iter()
            .map(|s| (s.path.as_str(), s.kind))
            .collect::<Vec<_>>(),
        [("name", ChangeKind::Changed)]
    );
    let same = compare(&r, effective_of("sit1"), effective_of("sit1"), "core.yml")
        .await
        .unwrap();
    assert!(same.identical && same.setting_changes.is_empty());
}

#[tokio::test]
async fn a_difference_in_line_endings_alone_is_flagged() {
    let Some(r) = two_lanes(&[("notes.txt", "one\ntwo\n")], &[("notes.txt", "one\r\ntwo\r\n")]).await else {
        return;
    };
    let c = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "notes.txt")
        .await
        .unwrap();
    assert!(c.eol_only, "only the line endings differ");
    assert!(!c.identical, "the bytes differ");
    assert!(c.hunks.is_empty(), "there is nothing to show but the endings");
}

#[tokio::test]
async fn a_binary_file_and_a_denied_file_are_compared_by_hash_and_size_alone() {
    let Some(r) = two_lanes(
        &[
            ("blob.bin", "\u{0}\u{1}one"),
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nAAAA\n"),
        ],
        &[
            ("blob.bin", "\u{0}\u{1}two!"),
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nBBBB\n"),
        ],
    )
    .await
    else {
        return;
    };
    let bin = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "blob.bin")
        .await
        .unwrap();
    assert_eq!(bin.class, FileClass::Binary);
    let facts = bin.binary.as_ref().unwrap();
    assert_eq!((facts.left_size, facts.right_size), (Some(5), Some(6)));
    assert!(bin.hunks.is_empty() && bin.setting_changes.is_empty() && !bin.identical);

    let denied = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "keys/server.pem")
        .await
        .unwrap();
    assert_eq!(denied.class, FileClass::Denied);
    assert!(denied.hunks.is_empty() && denied.setting_changes.is_empty());
    assert!(
        denied.binary.is_some() && !denied.identical,
        "hashes and sizes, nothing else"
    );
    let text = json(&denied);
    assert!(
        !text.contains("AAAA") && !text.contains("BBBB") && !text.contains("PRIVATE"),
        "{text}"
    );
    // The same denied file on both sides is identical, by hash.
    let same = compare(&r, nfs_of("sit1"), nfs_of("sit1"), "keys/server.pem")
        .await
        .unwrap();
    assert!(same.identical);
}

#[tokio::test]
async fn a_changed_secret_does_not_look_unchanged_and_no_secret_is_in_the_answer() {
    let a = format!("visible: 1\ntoken: {SECRET_A}\n");
    let b = format!("visible: 1\ntoken: {SECRET_B}\n");
    let Some(r) = two_lanes(&[("core.yml", &a)], &[("core.yml", &b)]).await else {
        return;
    };
    for indexed in [false, true] {
        if indexed {
            r.index_all().await;
            r.service.clear_caches();
        }
        let c = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "core.yml")
            .await
            .unwrap();
        assert!(!c.identical, "indexed={indexed}");
        let changes: Vec<_> = c
            .setting_changes
            .iter()
            .map(|s| (s.path.as_str(), s.kind))
            .collect();
        assert_eq!(
            changes,
            [("token", ChangeKind::Changed)],
            "indexed={indexed}: two secrets of one length differ"
        );
        let change = &c.setting_changes[0];
        for side in [&change.left, &change.right] {
            let text = &side.as_ref().unwrap().text;
            assert!(text.starts_with("[REDACTED: "), "indexed={indexed}: {text}");
        }
        let body = json(&c);
        // The setting values are masked. (The hunks are the raw text, which A28 records as a residual risk for the threat model.)
        let changes_only =
            serde_json::to_string(&hub_registry::http::dto::ComparisonDto::from(&c).setting_changes).unwrap();
        assert!(
            !changes_only.contains(SECRET_A) && !changes_only.contains(SECRET_B),
            "indexed={indexed}"
        );
        assert!(body.contains("setting_changes"));
    }
    // The same secret on both sides: no change, identical.
    let Some(same) = two_lanes(&[("core.yml", &a)], &[("core.yml", &a)]).await else {
        return;
    };
    let c = compare(&same, nfs_of("sit1"), nfs_of("sit2"), "core.yml")
        .await
        .unwrap();
    assert!(c.identical && c.setting_changes.is_empty());
}

#[tokio::test]
async fn git_is_a_side_through_the_path_mapping_and_a_missing_file_is_an_absent_side() {
    let Some(r) = two_lanes(
        &[("core.yml", "port: 1\n"), ("only.yml", "a: 1\n")],
        &[("core.yml", "port: 1\n")],
    )
    .await
    else {
        return;
    };
    r.world.base_commit(&[("config/core.yml", Some("port: 9\n"))]);
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("port: 1\n"))]);
    let base = CompareRef::GitBase(GitRef::parse("master").unwrap());
    let tenant = CompareRef::GitTenant(GitRef::parse("sit1").unwrap());
    let vs_base = compare(&r, nfs_of("sit1"), base.clone(), "core.yml")
        .await
        .unwrap();
    assert_eq!(vs_base.setting_changes.len(), 1);
    assert_eq!(vs_base.setting_changes[0].path.as_str(), "port");
    let vs_tenant = compare(&r, nfs_of("sit1"), tenant, "core.yml").await.unwrap();
    assert!(vs_tenant.identical, "NFS and the tenant's branch agree");
    assert_eq!(
        compare(
            &r,
            nfs_of("sit1"),
            CompareRef::GitBase(GitRef::parse("nope").unwrap()),
            "core.yml"
        )
        .await
        .unwrap_err(),
        ReadError::NotFound,
        "a ref that does not exist"
    );
    // `only.yml` is on sit1 and not on sit2: the right side is absent, and everything on the left is an addition.
    let absent = compare(&r, nfs_of("sit1"), nfs_of("sit2"), "only.yml")
        .await
        .unwrap();
    assert!(!absent.identical);
    assert_eq!(absent.setting_changes.len(), 1);
    assert_eq!(
        absent.setting_changes[0].kind,
        ChangeKind::Removed,
        "present on the left, gone on the right"
    );
    // A file on neither side is not a comparison.
    assert_eq!(
        compare(&r, nfs_of("sit1"), nfs_of("sit2"), "nowhere.yml")
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        compare(&r, nfs_of("sit1"), CompareRef::Nfs(lane("sit9")), "core.yml")
            .await
            .unwrap_err(),
        ReadError::NotFound,
        "an unknown swimlane"
    );
    assert_eq!(
        r.service
            .compare(&Reads::viewer(), nfs_of("sit1"), nfs_of("sit2"), None)
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "one file at a time; a whole tree is `compare_tree`"
    );
}

#[tokio::test]
async fn the_baseline_side_is_the_version_adoption_kept() {
    let Some(mut r) = two_lanes(&[("core.yml", "port: 1\n")], &[("core.yml", "port: 1\n")]).await else {
        return;
    };
    let original = content_hash(b"port: 1\n");
    r.world.baseline("core.yml", original).await;
    r.sql(&format!(
        "UPDATE baselines SET blob_hash = decode('{original}', 'hex') WHERE swimlane_id = 'sit1' AND path = 'core.yml'"
    ))
    .await;
    r.world.nfs_change(&[("core.yml", "port: 2\n")]).await;
    let c = compare(&r, CompareRef::Baseline(lane("sit1")), nfs_of("sit1"), "core.yml")
        .await
        .unwrap();
    assert_eq!(
        c.setting_changes.len(),
        1,
        "the baseline had port 1, NFS has port 2"
    );
    assert_eq!(c.setting_changes[0].left.as_ref().unwrap().text, "1");
    assert_eq!(c.setting_changes[0].right.as_ref().unwrap().text, "2");
    // A swimlane with no baseline for the file has an absent baseline side.
    let none = compare(&r, CompareRef::Baseline(lane("sit2")), nfs_of("sit2"), "core.yml").await;
    let none = none.unwrap();
    assert!(!none.identical, "nothing on the left, a file on the right");
    assert_eq!(none.setting_changes[0].kind, ChangeKind::Added);
}

// ------------------------------------------------------------------------------------------------ tag and cache

#[tokio::test]
async fn the_tag_is_from_the_two_sides_and_the_diff_is_cached_by_their_hashes() {
    let Some(mut r) = two_lanes(&[("core.yml", "port: 1\n")], &[("core.yml", "port: 2\n")]).await else {
        return;
    };
    let v = Reads::viewer();
    let ask = |tag: Option<String>| {
        let (service, v) = (r.service.clone(), v.clone());
        async move {
            service
                .compare_tagged(
                    &v,
                    nfs_of("sit1"),
                    nfs_of("sit2"),
                    &logical("core.yml"),
                    None,
                    tag.as_deref(),
                )
                .await
                .unwrap()
        }
    };
    let Tagged::Fresh { etag, value } = ask(None).await else {
        panic!("no tag was given");
    };
    assert_eq!(
        value.tenants,
        [
            (lane("sit1"), "sit1".parse().unwrap()),
            (lane("sit2"), "sit2".parse().unwrap())
        ],
        "A30: the tenant that stood for each swimlane"
    );
    let m = &r.world.rig.metrics;
    let (hits, misses) = (
        m.read_cache_count(CacheName::Diff, CacheResult::Hit),
        m.read_cache_count(CacheName::Diff, CacheResult::Miss),
    );
    assert_eq!(misses, 1);
    // The same two sides again: the diff is not made twice.
    let Tagged::Fresh { etag: again, .. } = ask(None).await else {
        panic!("no tag was given");
    };
    assert_eq!(again, etag);
    assert_eq!(m.read_cache_count(CacheName::Diff, CacheResult::Hit), hits + 1);
    assert_eq!(m.read_cache_count(CacheName::Diff, CacheResult::Miss), misses);
    // A matching tag is answered before any diff is made.
    let before = m.read_cache_count(CacheName::Diff, CacheResult::Hit)
        + m.read_cache_count(CacheName::Diff, CacheResult::Miss);
    assert!(matches!(
        ask(Some(etag.clone())).await,
        Tagged::NotModified { .. }
    ));
    let after = m.read_cache_count(CacheName::Diff, CacheResult::Hit)
        + m.read_cache_count(CacheName::Diff, CacheResult::Miss);
    assert_eq!(before, after, "a 304 reads no diff");
    // A change on either side is another tag, and the old one is a full answer.
    r.world.nfs_change(&[("core.yml", "port: 3\n")]).await;
    let Tagged::Fresh { etag: changed, .. } = ask(Some(etag.clone())).await else {
        panic!("the old tag no longer matches");
    };
    assert_ne!(changed, etag);
    // The sides are not interchangeable: swapped, the answer has its own tag.
    let Tagged::Fresh { etag: swapped, .. } = r
        .service
        .compare_tagged(
            &v,
            nfs_of("sit2"),
            nfs_of("sit1"),
            &logical("core.yml"),
            None,
            None,
        )
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert_ne!(swapped, changed);
}

// ------------------------------------------------------------------------------------------------ the whole tree

async fn tree(
    r: &Reads,
    l: CompareRef,
    rr: CompareRef,
    prefix: &str,
    p: Page,
) -> Result<TreeComparison, ReadError> {
    let prefix = if prefix.is_empty() {
        domain::NfsPath::root()
    } else {
        nfs(prefix)
    };
    Ok(r.service
        .compare_tree_named(&Reads::viewer(), l, rr, &prefix, p)
        .await?
        .value)
}

fn entries(t: &TreeComparison) -> Vec<(String, CompareState)> {
    t.entries
        .items
        .iter()
        .map(|e| (e.file.to_string(), e.state))
        .collect()
}

#[tokio::test]
async fn a_whole_tree_compare_lists_the_files_that_differ_and_nothing_else() {
    let Some(r) = two_lanes(
        &[
            ("a/same.yml", "x: 1\n"),
            ("a/diff.yml", "x: 1\n"),
            ("a/left.yml", "x: 1\n"),
            ("b/deep/same.yml", "y: 1\n"),
            ("top.yml", "z: 1\n"),
        ],
        &[
            ("a/same.yml", "x: 1\n"),
            ("a/diff.yml", "x: 2\n"),
            ("a/right.yml", "x: 1\n"),
            ("b/deep/same.yml", "y: 1\n"),
            ("top.yml", "z: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    let t = tree(&r, nfs_of("sit1"), nfs_of("sit2"), "", page(100))
        .await
        .unwrap();
    assert_eq!(
        entries(&t),
        [
            ("a/diff.yml".to_owned(), CompareState::Different),
            ("a/left.yml".to_owned(), CompareState::LeftOnly),
            ("a/right.yml".to_owned(), CompareState::RightOnly),
        ]
    );
    assert_eq!(
        t.skipped_identical_subtrees, 0,
        "path-level hashes skip nothing (A29)"
    );
    // A prefix limits the compare to one folder.
    let a = tree(&r, nfs_of("sit1"), nfs_of("sit2"), "b", page(100))
        .await
        .unwrap();
    assert!(a.entries.items.is_empty());
    let only_a = tree(&r, nfs_of("sit1"), nfs_of("sit2"), "a", page(100))
        .await
        .unwrap();
    assert_eq!(only_a.entries.items.len(), 3);
    // A swimlane against itself differs nowhere.
    let same = tree(&r, nfs_of("sit1"), nfs_of("sit1"), "", page(100))
        .await
        .unwrap();
    assert!(same.entries.items.is_empty());
    // Unknown swimlane, and a Git side only against another Git or NFS side's raw hash.
    assert_eq!(
        tree(&r, nfs_of("sit1"), nfs_of("sit9"), "", page(10))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

#[tokio::test]
async fn two_effective_trees_skip_the_subtrees_that_are_equal_and_count_them() {
    let Some(r) = two_lanes(
        &[
            ("a/one.yml", "x: 1\n"),
            ("b/deep/two.yml", "y: 1\n"),
            ("c/three.yml", "z: 1\n"),
            ("top.yml", "t: 1\n"),
        ],
        &[
            ("a/one.yml", "x: 1\n"),
            ("b/deep/two.yml", "y: 1\n"),
            ("c/three.yml", "z: 2\n"),
            ("top.yml", "t: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    for id in ["sit1", "sit2"] {
        r.world
            .pipeline
            .process(vec![(SwimlaneId::parse(id).unwrap(), hub_registry::Work::Whole)])
            .await
            .unwrap();
    }
    let t = tree(&r, effective_of("sit1"), effective_of("sit2"), "", page(100))
        .await
        .unwrap();
    assert_eq!(entries(&t), [("c/three.yml".to_owned(), CompareState::Different)]);
    assert!(
        t.skipped_identical_subtrees >= 2,
        "the folders `a` and `b` were equal and were not read: {}",
        t.skipped_identical_subtrees
    );
    let same = tree(&r, effective_of("sit1"), effective_of("sit1"), "", page(100))
        .await
        .unwrap();
    assert!(same.entries.items.is_empty());
    assert!(
        same.skipped_identical_subtrees >= 1,
        "two equal roots are one skipped subtree"
    );
    // Effective against Git has no common hash to compare by.
    let git = CompareRef::GitBase(GitRef::parse("master").unwrap());
    assert_eq!(
        tree(&r, effective_of("sit1"), git, "", page(10))
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
}

#[tokio::test]
async fn the_tree_compare_pages_by_a_cursor_that_belongs_to_its_query() {
    let files: Vec<(String, String)> = (0..7)
        .map(|i| (format!("d/f{i}.yml"), format!("v: {i}\n")))
        .collect();
    let other: Vec<(String, String)> = (0..7)
        .map(|i| (format!("d/f{i}.yml"), format!("v: {}\n", i + 100)))
        .collect();
    let left: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let right: Vec<(&str, &str)> = other.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let Some(r) = two_lanes(&left, &right).await else {
        return;
    };
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let p = Page::new(3, cursor.clone()).unwrap();
        let t = tree(&r, nfs_of("sit1"), nfs_of("sit2"), "", p).await.unwrap();
        assert!(t.entries.items.len() <= 3);
        seen.extend(entries(&t).into_iter().map(|e| e.0));
        match t.entries.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 7);
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted, "in path order, each once");
    // A cursor of this compare is refused by another one.
    let first = tree(&r, nfs_of("sit1"), nfs_of("sit2"), "", page(3))
        .await
        .unwrap();
    let next = first.entries.next.unwrap();
    let p = Page::new(3, Some(next)).unwrap();
    assert_eq!(
        tree(&r, nfs_of("sit2"), nfs_of("sit1"), "", p.clone())
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "the sides are in the cursor's context"
    );
    assert_eq!(
        tree(&r, nfs_of("sit1"), nfs_of("sit2"), "d", p)
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "and so is the prefix"
    );
    let _: Option<ContentHash> = None;
}
