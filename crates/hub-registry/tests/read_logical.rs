//! `logical_file` and `tenant` on every read that names a file (`contract/08-web`, CG2, S17, D104): the tree, the drift lists, the settings
//! search, the text search and the findings, over a real Postgres, with files that got in through ingestion.
//!
//! The value is the registry's own classification of the file (`file_state.logical_file`, the engine's `classify_path`): a base file is its
//! own logical file and has no tenant, a tenant copy has the name without its `-<tenant>` suffix and names the tenant, and a folder has
//! neither. A denied file's are as visible as its path and say nothing of its content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use domain::{
    DriftQuery, FileEntry, FindingKind, FindingsQuery, LogicalFile, NfsPath, SettingsQuery, ShortText,
    TenantId, TextQuery,
};
use hub_registry::RegistryApi;
use ports::RegistryRead;
use support::reads::{Reads, lane, nfs, page};

/// The swimlane deploys `sit1` and `sit2`. `core-sit10.yml` is not a tenant copy (no tenant of that name), and `core-sit3.yml` is a suffix of
/// a tenant that the swimlane does not deploy: both are base files by their names.
const FILES: [(&str, &str); 7] = [
    ("app/core.yml", "port: 1\n"),
    ("app/core-sit1.yml", "port: 2\n"),
    ("app/core-sit2.yml", "port: 3\n"),
    ("app/core-sit10.yml", "port: 4\n"),
    ("app/core-sit3.yml", "port: 5\n"),
    ("top.properties", "k=v\n"),
    ("tx-infinity-core-sit1.yml", "name: x\n"),
];

async fn reads() -> Option<Reads> {
    let mut r = Reads::new(&["sit1", "sit2"]).await?;
    r.world.inventory(&FILES).await;
    r.index_all().await;
    Some(r)
}

/// `(logical file, tenant)` as strings.
type Names = (Option<String>, Option<String>);

fn names(e: &FileEntry) -> Names {
    (
        e.logical_file.as_ref().map(ToString::to_string),
        e.tenant.as_ref().map(ToString::to_string),
    )
}

fn some(logical: &str, tenant: Option<&str>) -> Names {
    (Some(logical.to_owned()), tenant.map(str::to_owned))
}

fn of<'a>(entries: &'a [FileEntry], path: &str) -> &'a FileEntry {
    entries
        .iter()
        .find(|e| e.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} is not listed"))
}

#[tokio::test]
async fn the_tree_names_the_logical_file_and_the_tenant_of_every_file_and_none_for_a_folder() {
    let Some(r) = reads().await else { return };
    let v = Reads::viewer();
    let app = r
        .service
        .tree(&v, &lane("sit1"), &nfs("app"), page(100))
        .await
        .unwrap();
    assert_eq!(names(of(&app.items, "app/core.yml")), some("app/core.yml", None));
    assert_eq!(
        names(of(&app.items, "app/core-sit1.yml")),
        some("app/core.yml", Some("sit1")),
        "a tenant copy is the file without its suffix, and the tenant's"
    );
    assert_eq!(
        names(of(&app.items, "app/core-sit2.yml")),
        some("app/core.yml", Some("sit2")),
        "every tenant of the swimlane, not only the first"
    );
    assert_eq!(
        names(of(&app.items, "app/core-sit10.yml")),
        some("app/core-sit10.yml", None),
        "`-sit10` is not `-sit1`: a name is never cut at a prefix of a tenant id"
    );
    assert_eq!(
        names(of(&app.items, "app/core-sit3.yml")),
        some("app/core-sit3.yml", None),
        "a suffix that is no tenant of the swimlane is part of the name"
    );
    let root = r
        .service
        .tree(&v, &lane("sit1"), &NfsPath::root(), page(100))
        .await
        .unwrap();
    let folder = of(&root.items, "app");
    assert!(folder.is_dir);
    assert_eq!(names(folder), (None, None), "a folder is no file");
    assert_eq!(
        names(of(&root.items, "tx-infinity-core-sit1.yml")),
        some("tx-infinity-core.yml", Some("sit1")),
        "a name with hyphens of its own is cut at the tenant's suffix only"
    );
    assert_eq!(
        names(of(&root.items, "top.properties")),
        some("top.properties", None)
    );
}

#[tokio::test]
async fn the_drift_lists_name_them_too_in_either_order() {
    let Some(r) = reads().await else { return };
    // Every file is new to Git, so every file is drift of some kind.
    let v = Reads::viewer();
    let q = DriftQuery {
        state: None,
        prefix: None,
    };
    let by_path = r.service.drift(&v, &lane("sit1"), &q, page(100)).await.unwrap();
    assert!(!by_path.items.is_empty(), "the files are not in sync with Git");
    for e in &by_path.items {
        assert!(!e.is_dir);
        let (logical, tenant) = names(e);
        let logical = logical.unwrap_or_else(|| panic!("{}: a drift entry names its logical file", e.path));
        match tenant {
            Some(t) => assert_eq!(
                logical,
                e.path.as_str().replace(&format!("-{t}"), ""),
                "{}: the logical file is the name without the suffix",
                e.path
            ),
            None => assert_eq!(
                logical,
                e.path.as_str(),
                "{}: a base file is its own logical file",
                e.path
            ),
        }
    }
    let tenant_copy = of(&by_path.items, "app/core-sit1.yml");
    assert_eq!(names(tenant_copy), some("app/core.yml", Some("sit1")));
    // The REST list (most serious first) is the same entries.
    let rest = r
        .service
        .drift_ranked(&v, &lane("sit1"), None, &NfsPath::root(), page(100))
        .await
        .unwrap();
    assert_eq!(rest.items.len(), by_path.items.len());
    for e in &rest.items {
        let same = of(&by_path.items, e.path.as_str());
        assert_eq!(names(e), names(same), "{}: both orders say the same", e.path);
    }
}

#[tokio::test]
async fn a_denied_file_names_its_logical_file_and_tenant_from_its_name_and_nothing_of_its_content() {
    let Some(mut r) = reads().await else { return };
    r.world
        .nfs_raw(|at| {
            vec![
                support::rig::entry("keys/server-sit1.pem", b"-----BEGIN PRIVATE KEY-----", at),
                support::rig::entry("keys/server.pem", b"-----BEGIN PRIVATE KEY-----", at),
            ]
        })
        .await;
    let v = Reads::viewer();
    let keys = r
        .service
        .tree(&v, &lane("sit1"), &nfs("keys"), page(10))
        .await
        .unwrap();
    let tenant_copy = of(&keys.items, "keys/server-sit1.pem");
    assert!(tenant_copy.content_withheld, "it is denied");
    assert_eq!(tenant_copy.class, domain::FileClass::Denied);
    assert_eq!(names(tenant_copy), some("keys/server.pem", Some("sit1")));
    let base = of(&keys.items, "keys/server.pem");
    assert!(base.content_withheld);
    assert_eq!(names(base), some("keys/server.pem", None));
    // A search never lists a denied file, so no hit names it.
    r.index_all().await;
    let hits = r
        .service
        .search_settings(
            &v,
            SettingsQuery {
                path_contains: ShortText::parse("port").unwrap(),
                swimlanes: vec![],
                file: None,
                page: page(100),
            },
        )
        .await
        .unwrap();
    assert!(hits.items.iter().all(|h| !h.file.as_str().starts_with("keys/")));
    let found = r.service.search_text(&v, text("BEGIN PRIVATE")).await.unwrap();
    assert!(
        found.hits.is_empty(),
        "a denied file is not in the corpus: {:?}",
        found.hits
    );
}

#[tokio::test]
async fn setting_hits_name_the_file_they_are_in() {
    let Some(r) = reads().await else { return };
    let v = Reads::viewer();
    let hits = r
        .service
        .search_settings(
            &v,
            SettingsQuery {
                path_contains: ShortText::parse("port").unwrap(),
                swimlanes: vec![],
                file: None,
                page: page(100),
            },
        )
        .await
        .unwrap();
    let mut seen: Vec<(String, Names)> = hits
        .items
        .iter()
        .map(|h| {
            (
                h.file.to_string(),
                (
                    h.logical_file.as_ref().map(ToString::to_string),
                    h.tenant.as_ref().map(ToString::to_string),
                ),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("app/core-sit1.yml".to_owned(), some("app/core.yml", Some("sit1"))),
            ("app/core-sit10.yml".to_owned(), some("app/core-sit10.yml", None)),
            ("app/core-sit2.yml".to_owned(), some("app/core.yml", Some("sit2"))),
            ("app/core-sit3.yml".to_owned(), some("app/core-sit3.yml", None)),
            ("app/core.yml".to_owned(), some("app/core.yml", None)),
        ]
    );
    // The same setting in one logical file's group: the filter is by logical file, and the hits of the group name it.
    let group = r
        .service
        .search_settings(
            &v,
            SettingsQuery {
                path_contains: ShortText::parse("port").unwrap(),
                swimlanes: vec![],
                file: Some(LogicalFile::parse("app/core.yml").unwrap()),
                page: page(100),
            },
        )
        .await
        .unwrap();
    assert_eq!(group.items.len(), 3, "the base file and the two tenant copies");
    assert!(
        group
            .items
            .iter()
            .all(|h| h.logical_file.as_ref().map(LogicalFile::as_str) == Some("app/core.yml"))
    );
}

fn text(pattern: &str) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex: false,
        case_sensitive: false,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: 500,
        max_per_file: 20,
    }
}

#[tokio::test]
async fn text_hits_name_the_file_they_are_in() {
    let Some(r) = reads().await else { return };
    let v = Reads::viewer();
    let found = r.service.search_text(&v, text("port")).await.unwrap();
    let mut seen: Vec<(String, Names)> = found
        .hits
        .iter()
        .map(|h| {
            (
                h.path.to_string(),
                (
                    h.logical_file.as_ref().map(ToString::to_string),
                    h.tenant.as_ref().map(ToString::to_string),
                ),
            )
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("app/core-sit1.yml".to_owned(), some("app/core.yml", Some("sit1"))),
            ("app/core-sit10.yml".to_owned(), some("app/core-sit10.yml", None)),
            ("app/core-sit2.yml".to_owned(), some("app/core.yml", Some("sit2"))),
            ("app/core-sit3.yml".to_owned(), some("app/core-sit3.yml", None)),
            ("app/core.yml".to_owned(), some("app/core.yml", None)),
        ]
    );
    // A tenant file and its hit say the same as its tree entry.
    let app = r
        .service
        .tree(&v, &lane("sit1"), &nfs("app"), page(100))
        .await
        .unwrap();
    let entry = of(&app.items, "app/core-sit1.yml");
    let hit = found
        .hits
        .iter()
        .find(|h| h.path.as_str() == "app/core-sit1.yml")
        .unwrap();
    assert_eq!(
        (&hit.logical_file, &hit.tenant),
        (&entry.logical_file, &entry.tenant)
    );
}

#[tokio::test]
async fn a_finding_names_the_file_it_is_about_and_none_when_that_is_no_tracked_file() {
    let Some(r) = reads().await else { return };
    r.sql(
        "INSERT INTO findings (swimlane_id, kind, path, severity, rule_ids, message, locations, fingerprint) VALUES \
          ('sit1', 'drift', 'app/core-sit1.yml', 'low', '{}', 'a tenant copy', '[]', 'f1'), \
          ('sit1', 'drift', 'app/core.yml', 'low', '{}', 'a base file', '[]', 'f2'), \
          ('sit1', 'config_server_restart_required', 'app', 'low', '{}', 'a folder', '[]', 'f3'), \
          ('sit1', 'config_server_restart_required', NULL, 'low', '{}', 'no path', '[]', 'f4'), \
          ('sit1', 'uneven_tenant_files', 'lonely/only.yml', 'low', '{}', 'the logical file of copies, with no base', '[]', 'f5'), \
          ('sit1', 'drift', 'gone/away.yml', 'low', '{}', 'a file that is gone', '[]', 'f6')",
    )
    .await;
    let v = Reads::viewer();
    let all = r
        .service
        .findings(
            &v,
            FindingsQuery {
                swimlane: None,
                kinds: vec![],
                min_severity: None,
                page: page(100),
            },
        )
        .await
        .unwrap();
    let by_message = |m: &str| {
        let f = all.items.iter().find(|f| f.message.as_str() == m).unwrap();
        (
            f.logical_file.as_ref().map(ToString::to_string),
            f.tenant.as_ref().map(ToString::to_string),
        )
    };
    assert_eq!(by_message("a tenant copy"), some("app/core.yml", Some("sit1")));
    assert_eq!(by_message("a base file"), some("app/core.yml", None));
    assert_eq!(by_message("a folder"), (None, None), "a folder is no file");
    assert_eq!(by_message("no path"), (None, None));
    assert_eq!(
        by_message("the logical file of copies, with no base"),
        some("lonely/only.yml", None),
        "C3 names the logical file itself as its path, whether or not a base file exists"
    );
    assert_eq!(
        by_message("a file that is gone"),
        (None, None),
        "the registry holds no classification of a file it does not track, and invents none"
    );
    let kinds: Vec<FindingKind> = all.items.iter().map(|f| f.kind).collect();
    assert!(kinds.contains(&FindingKind::UnevenTenantFiles));
    let sit1 = TenantId::parse("sit1").unwrap();
    assert!(all.items.iter().any(|f| f.tenant.as_ref() == Some(&sit1)));
}
