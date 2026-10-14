//! The plain reads of the registry tables (T6, S4, S11, P12): the swimlane list and detail, the tree, the drift list, the history, the
//! findings, the services and the content of a file. Over a real Postgres 16, with files that got in through ingestion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{
    AgentStatus, ContentSource, DriftState, FindingKind, FindingsQuery, GitRef, Page, PickupState, RepoKind,
    Severity,
};
use hub_registry::{RegistryApi, Tagged};
use ports::{ReadError, RegistryRead, content_hash};
use support::reads::{Reads, lane, names, nfs, page};

const FILES: [(&str, &str); 6] = [
    ("a/one.yml", "x: 1\n"),
    ("a/two.yml", "y: 2\n"),
    ("a/deep/five.yml", "z: 5\n"),
    ("b/three.yml", "w: 3\n"),
    ("top.properties", "k=v\n"),
    ("weird_%/x.yml", "q: 1\n"),
];

async fn reads() -> Option<Reads> {
    let mut r = Reads::new(&["sit1", "sit2"]).await?;
    r.world.inventory(&FILES).await;
    Some(r)
}

// ------------------------------------------------------------------------------------------------ swimlanes

#[tokio::test]
async fn the_swimlane_list_has_the_tenants_the_counts_and_the_agent() {
    let Some(r) = reads().await else { return };
    r.world.whole().await;
    // A swimlane nobody has said hello for.
    r.world.rig.add_swimlane("sit9", &[], false).await;
    let v = Reads::viewer();

    let list = r.service.swimlanes(&v).await.unwrap();
    let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["sit1", "sit9"], "ordered by id");
    let sit1 = &list[0];
    assert_eq!(
        sit1.tenants.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["sit1", "sit2"],
        "a set of tenants, sorted"
    );
    assert_eq!(
        sit1.agent,
        AgentStatus::Disconnected,
        "said hello, no live connection"
    );
    assert_eq!(list[1].agent, AgentStatus::NeverSeen);
    assert!(
        list[1].tenants.is_empty(),
        "a swimlane that is not adopted has no tenants yet"
    );
    assert!(sit1.last_scan_at.is_some());
    assert!(list[1].last_scan_at.is_none());
    let total: u32 = sit1.drift_counts.iter().map(|(_, n)| n).sum();
    assert_eq!(total as usize, FILES.len(), "{:?}", sit1.drift_counts);
    assert!(
        sit1.drift_counts.windows(2).all(|w| w[0].0 < w[1].0),
        "the non-zero states in the order of the states"
    );

    // A connection row that was renewed makes the agent connected; one that went stale does not.
    r.sql(
        "INSERT INTO agent_connections (swimlane_id, replica_id, replica_address, epoch, last_seen_at) \
         VALUES ('sit1', 'r1', '10.0.0.1:1', 1, now())",
    )
    .await;
    assert_eq!(
        r.service.swimlanes(&v).await.unwrap()[0].agent,
        AgentStatus::Connected
    );
    r.sql("UPDATE agent_connections SET last_seen_at = now() - interval '5 minutes'")
        .await;
    assert_eq!(
        r.service.swimlanes(&v).await.unwrap()[0].agent,
        AgentStatus::Disconnected
    );

    // Keyset paging by id: one at a time, every swimlane once.
    let first = r.service.swimlanes_page(&v, page(1)).await.unwrap();
    assert_eq!(first.items.len(), 1);
    let second = r
        .service
        .swimlanes_page(&v, domain::Page::new(1, first.next.clone()).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items[0].id.as_str(), "sit9");
    assert!(second.next.is_none());
}

#[tokio::test]
async fn the_swimlane_detail_adds_the_services_and_the_auto_notify_setting() {
    let Some(r) = reads().await else { return };
    r.sql(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) VALUES \
           ('sit1', 'ns', 'api', '{\"CONFIG_CLIENT_CACHE_TTL\": \"60\", \"CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED\": \"true\"}'), \
           ('sit1', 'ns', 'web', '{}'); \
         UPDATE swimlanes SET auto_notify = true WHERE id = 'sit1'",
    )
    .await;
    let v = Reads::viewer();
    let detail = r.service.swimlane(&v, &lane("sit1")).await.unwrap();
    assert!(detail.auto_notify);
    assert_eq!(detail.summary.id.as_str(), "sit1");
    let svc: Vec<(&str, PickupState, u32, bool)> = detail
        .services
        .iter()
        .map(|s| {
            (
                s.service.name(),
                s.pickup,
                s.cache_ttl_secs,
                s.notifications_enabled,
            )
        })
        .collect();
    assert_eq!(
        svc,
        [
            ("api", PickupState::Live, 60, true),
            ("web", PickupState::Live, 1200, false)
        ],
        "every service, with the client settings of its Deployment"
    );
    assert_eq!(
        r.service.swimlane(&v, &lane("nowhere")).await.unwrap_err(),
        ReadError::NotFound
    );
}

// ------------------------------------------------------------------------------------------------ the tree and the drift list

async fn arrange_states(r: &Reads) {
    r.sql(
        "UPDATE file_state SET state = 'conflict', severity = 'high' WHERE swimlane_id = 'sit1' AND path = 'a/deep/five.yml'; \
         UPDATE file_state SET state = 'git_ahead', severity = 'low' WHERE swimlane_id = 'sit1' AND path = 'a/two.yml'; \
         UPDATE file_state SET state = 'in_sync' WHERE swimlane_id = 'sit1' AND path IN ('a/one.yml', 'top.properties', 'weird_%/x.yml'); \
         UPDATE file_state SET state = 'in_sync' WHERE swimlane_id = 'sit1' AND path = 'b/three.yml'",
    )
    .await;
}

#[tokio::test]
async fn a_folder_is_as_bad_as_the_worst_file_below_it() {
    let Some(r) = reads().await else { return };
    arrange_states(&r).await;
    let v = Reads::viewer();
    let root = r
        .service
        .tree_filtered(&v, &lane("sit1"), &nfs_root(), None, page(100))
        .await
        .unwrap();
    assert_eq!(names(&root), ["a/", "b/", "top.properties", "weird_%/"]);
    let by_name = |n: &str| root.items.iter().find(|e| e.path.as_str() == n).unwrap();
    assert_eq!(
        by_name("a").drift,
        DriftState::Conflict,
        "the worst of conflict, git_ahead and in_sync"
    );
    assert_eq!(
        by_name("a").severity,
        Some(Severity::High),
        "the worst severity below it"
    );
    assert_eq!(by_name("b").drift, DriftState::InSync);
    assert_eq!(by_name("b").severity, None);
    assert!(by_name("a").hash.is_none() && by_name("a").is_dir);
    let file = by_name("top.properties");
    assert_eq!(file.hash, Some(content_hash(b"k=v\n")));
    assert_eq!(file.size, 4);
    assert_eq!(file.role, domain::FileRole::Resource);

    let under_a = r
        .service
        .tree_filtered(&v, &lane("sit1"), &nfs("a"), None, page(100))
        .await
        .unwrap();
    assert_eq!(names(&under_a), ["a/deep/", "a/one.yml", "a/two.yml"]);
    assert_eq!(under_a.items[0].drift, DriftState::Conflict);
    let with_attribution = &under_a.items[1];
    assert!(
        with_attribution.attribution.is_some(),
        "a file that an observation points at has an attribution"
    );
}

fn nfs_root() -> domain::NfsPath {
    domain::NfsPath::root()
}

#[tokio::test]
async fn a_drift_filter_keeps_the_files_in_that_state_and_the_folders_that_hold_them() {
    let Some(r) = reads().await else { return };
    arrange_states(&r).await;
    let v = Reads::viewer();
    let conflict = r
        .service
        .tree_filtered(
            &v,
            &lane("sit1"),
            &nfs_root(),
            Some(DriftState::Conflict),
            page(100),
        )
        .await
        .unwrap();
    assert_eq!(names(&conflict), ["a/"]);
    assert_eq!(conflict.items[0].drift, DriftState::Conflict);
    let in_sync = r
        .service
        .tree_filtered(&v, &lane("sit1"), &nfs("a"), Some(DriftState::InSync), page(100))
        .await
        .unwrap();
    assert_eq!(names(&in_sync), ["a/one.yml"]);
}

#[tokio::test]
async fn a_prefix_is_a_folder_and_the_characters_of_like_mean_nothing() {
    let Some(mut r) = reads().await else { return };
    r.world
        .nfs_change(&[
            ("weirdX/y.yml", "q: 2\n"),
            ("a_b/z.yml", "q: 3\n"),
            ("aXb/z.yml", "q: 4\n"),
        ])
        .await;
    let v = Reads::viewer();
    let weird = r
        .service
        .tree(&v, &lane("sit1"), &nfs("weird_%"), page(100))
        .await
        .unwrap();
    assert_eq!(names(&weird), ["weird_%/x.yml"], "`_` and `%` are not wildcards");
    let under = r
        .service
        .tree(&v, &lane("sit1"), &nfs("a_b"), page(100))
        .await
        .unwrap();
    assert_eq!(names(&under), ["a_b/z.yml"], "`aXb` is not below `a_b`");
    let a = r
        .service
        .tree(&v, &lane("sit1"), &nfs("a"), page(100))
        .await
        .unwrap();
    assert!(
        names(&a).iter().all(|n| n.starts_with("a/")),
        "`a_b` is not below `a`: {:?}",
        names(&a)
    );
    let file_as_prefix = r
        .service
        .tree(&v, &lane("sit1"), &nfs("top.properties"), page(100))
        .await
        .unwrap();
    assert!(file_as_prefix.items.is_empty(), "a file has no children");
}

#[tokio::test]
async fn the_tree_pages_by_name_and_a_cursor_belongs_to_its_own_list() {
    let Some(r) = reads().await else { return };
    let v = Reads::viewer();
    let mut seen = Vec::new();
    let mut next = None;
    for _ in 0..10 {
        let p = r
            .service
            .tree(
                &v,
                &lane("sit1"),
                &nfs_root(),
                Page::new(2, next.clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(p.items.len() <= 2);
        seen.extend(names(&p));
        match p.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        ["a/", "b/", "top.properties", "weird_%/"],
        "every child once, in byte order"
    );

    // A cursor of this list does not page another prefix, another swimlane, another filter or another kind of list.
    let first = r
        .service
        .tree(&v, &lane("sit1"), &nfs_root(), page(1))
        .await
        .unwrap();
    let cursor = first.next.unwrap();
    let with = |c: &domain::Cursor| Page::new(10, Some(c.clone())).unwrap();
    assert_eq!(
        r.service
            .tree(&v, &lane("sit1"), &nfs("a"), with(&cursor))
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "another prefix"
    );
    r.world.rig.add_swimlane("sit2", &["sit1"], true).await;
    assert_eq!(
        r.service
            .tree(&v, &lane("sit2"), &nfs_root(), with(&cursor))
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "another swimlane"
    );
    assert_eq!(
        r.service
            .tree_filtered(
                &v,
                &lane("sit1"),
                &nfs_root(),
                Some(DriftState::Conflict),
                with(&cursor)
            )
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "another filter"
    );
    assert_eq!(
        r.service
            .drift(&v, &lane("sit1"), None, &nfs_root(), with(&cursor))
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "another kind of list"
    );
    let garbage = domain::Cursor::parse("not-a-cursor").unwrap();
    assert_eq!(
        r.service
            .tree(&v, &lane("sit1"), &nfs_root(), with(&garbage))
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
}

#[tokio::test]
async fn the_drift_list_is_flat_paged_by_sequence_and_filtered() {
    let Some(r) = reads().await else { return };
    arrange_states(&r).await;
    let v = Reads::viewer();
    let all = r
        .service
        .drift(&v, &lane("sit1"), None, &nfs_root(), page(100))
        .await
        .unwrap();
    let mut got: Vec<String> = all.items.iter().map(|e| e.path.to_string()).collect();
    got.sort();
    assert_eq!(
        got,
        ["a/deep/five.yml", "a/two.yml"],
        "everything that is not in sync, files only"
    );
    assert!(all.items.iter().all(|e| !e.is_dir));

    let mut seen = Vec::new();
    let mut next = None;
    loop {
        let p = r
            .service
            .drift(
                &v,
                &lane("sit1"),
                None,
                &nfs_root(),
                Page::new(1, next.clone()).unwrap(),
            )
            .await
            .unwrap();
        seen.extend(p.items.iter().map(|e| e.path.to_string()));
        match p.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    seen.sort();
    assert_eq!(seen, got, "one at a time visits each once");

    let only = r
        .service
        .drift(
            &v,
            &lane("sit1"),
            Some(DriftState::GitAhead),
            &nfs_root(),
            page(100),
        )
        .await
        .unwrap();
    assert_eq!(
        only.items.iter().map(|e| e.path.to_string()).collect::<Vec<_>>(),
        ["a/two.yml"]
    );
    let below = r
        .service
        .drift(&v, &lane("sit1"), None, &nfs("a/deep"), page(100))
        .await
        .unwrap();
    assert_eq!(below.items.len(), 1);
    assert_eq!(
        r.service
            .drift(&v, &lane("nowhere"), None, &nfs_root(), page(1))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

// ------------------------------------------------------------------------------------------------ history

#[tokio::test]
async fn the_history_is_newest_first_and_pages_by_time_and_id() {
    let Some(mut r) = reads().await else { return };
    r.world.nfs_change(&[("a/one.yml", "x: 2\n")]).await;
    r.world.nfs_change(&[("a/one.yml", "x: 3\n")]).await;
    let v = Reads::viewer();
    let all = r
        .service
        .history(&v, &lane("sit1"), &nfs("a/one.yml"), page(100))
        .await
        .unwrap();
    let hashes: Vec<_> = all.items.iter().map(|x| x.hash).collect();
    assert_eq!(
        hashes,
        [
            content_hash(b"x: 3\n"),
            content_hash(b"x: 2\n"),
            content_hash(b"x: 1\n")
        ],
        "newest first, the current version at the top"
    );
    assert!(all.items.windows(2).all(|w| w[0].observed_at >= w[1].observed_at));
    assert!(
        all.items.iter().all(|x| x.commit.is_none()),
        "a version read from NFS has no commit"
    );

    let mut seen = Vec::new();
    let mut next = None;
    loop {
        let p = r
            .service
            .history(
                &v,
                &lane("sit1"),
                &nfs("a/one.yml"),
                Page::new(1, next.clone()).unwrap(),
            )
            .await
            .unwrap();
        seen.extend(p.items.iter().map(|x| x.hash));
        match p.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, hashes, "one at a time, in the same order");
    let other = r
        .service
        .history(&v, &lane("sit1"), &nfs("a/two.yml"), page(1))
        .await
        .unwrap();
    assert_eq!(other.items.len(), 1);
    let cursor = r
        .service
        .history(&v, &lane("sit1"), &nfs("a/one.yml"), page(1))
        .await
        .unwrap()
        .next
        .unwrap();
    assert_eq!(
        r.service
            .history(
                &v,
                &lane("sit1"),
                &nfs("a/two.yml"),
                Page::new(1, Some(cursor)).unwrap()
            )
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "the cursor of one file's history does not page another's"
    );
    let _ = &mut r;
}

// ------------------------------------------------------------------------------------------------ findings

#[tokio::test]
async fn findings_are_the_open_ones_filtered_and_paged_newest_first() {
    let Some(r) = reads().await else { return };
    r.world.rig.add_swimlane("sit2", &["sit2"], true).await;
    r.sql(
        "INSERT INTO findings (swimlane_id, kind, path, severity, rule_ids, message, locations, fingerprint) VALUES \
          ('sit1', 'drift', 'a/one.yml', 'low', '{r1}', 'a finding', '[{\"path\": \"x\", \"lines\": {\"start\": 1, \"end\": 2}}]', 'f1'), \
          ('sit1', 'orphan_file', 'top.properties', 'medium', '{}', 'an orphan', '[]', 'f2'), \
          ('sit2', 'drift', 'b/three.yml', 'critical', '{}', 'another', '[]', 'f3'), \
          ('sit1', 'duplicate_keys', 'a/two.yml', 'high', '{}', 'resolved long ago', '[]', 'f4'); \
         UPDATE findings SET resolved_at = now() WHERE fingerprint = 'f4'",
    )
    .await;
    let v = Reads::viewer();
    let q =
        |swimlane: Option<&str>, kinds: Vec<FindingKind>, min: Option<Severity>, limit: u32| FindingsQuery {
            swimlane: swimlane.map(lane),
            kinds,
            min_severity: min,
            page: page(limit),
        };
    let all = r.service.findings(&v, q(None, vec![], None, 100)).await.unwrap();
    let msgs: Vec<&str> = all.items.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        msgs,
        ["another", "an orphan", "a finding"],
        "open ones, newest first; the resolved one is gone"
    );
    assert_eq!(all.items[2].locations.len(), 1);
    assert_eq!(all.items[2].rule_ids[0].as_str(), "r1");
    assert_eq!(all.items[0].swimlane.as_str(), "sit2");

    let only = r
        .service
        .findings(&v, q(Some("sit1"), vec![FindingKind::Drift], None, 100))
        .await
        .unwrap();
    assert_eq!(only.items.len(), 1);
    let high = r
        .service
        .findings(&v, q(None, vec![], Some(Severity::Medium), 100))
        .await
        .unwrap();
    assert_eq!(high.items.len(), 2, "medium and critical");
    let paged = r.service.findings(&v, q(None, vec![], None, 2)).await.unwrap();
    assert_eq!(paged.items.len(), 2);
    let rest = r
        .service
        .findings(
            &v,
            FindingsQuery {
                page: Page::new(2, paged.next).unwrap(),
                ..q(None, vec![], None, 2)
            },
        )
        .await
        .unwrap();
    assert_eq!(rest.items.len(), 1);
    assert!(rest.next.is_none());
    assert_eq!(
        r.service
            .findings(&v, q(Some("nowhere"), vec![], None, 1))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

// ------------------------------------------------------------------------------------------------ pending restarts

#[tokio::test]
async fn pending_restarts_are_the_services_that_have_not_picked_up_everything() {
    let Some(r) = reads().await else { return };
    r.sql(
        "INSERT INTO services (swimlane_id, namespace, name) VALUES ('sit1', 'ns', 'api'), ('sit1', 'ns', 'web'), ('sit1', 'ns', 'gone'); \
         UPDATE services SET removed_at = now() WHERE name = 'gone'; \
         INSERT INTO service_state (service_id, pickup_state, pending_since, live_by, pending_files) \
           SELECT id, 'live_within_ttl', now() - interval '1 minute', now() + interval '10 minutes', '[\"a/one.yml\"]' \
           FROM services WHERE name = 'api'; \
         INSERT INTO service_state (service_id, pickup_state) SELECT id, 'live' FROM services WHERE name = 'web'",
    )
    .await;
    let v = Reads::viewer();
    let pending = r.service.pending_restarts(&v, &lane("sit1")).await.unwrap();
    assert_eq!(pending.len(), 1, "web is live and gone is removed");
    let api = &pending[0];
    assert_eq!(api.service.name(), "api");
    assert_eq!(api.pickup, PickupState::LiveWithinTtl);
    assert!(api.live_by.is_some() && api.pending_since.is_some());
    assert_eq!(
        api.pending_files
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["a/one.yml"]
    );
    let paged = r
        .service
        .pending_restarts_page(&v, &lane("sit1"), page(1))
        .await
        .unwrap();
    assert_eq!(paged.items.len(), 1);
    assert!(paged.next.is_none());
}

// ------------------------------------------------------------------------------------------------ content

#[tokio::test]
async fn content_is_exact_and_a_blob_is_only_served_under_a_path_that_had_it() {
    let Some(mut r) = reads().await else { return };
    r.world.nfs_change(&[("a/one.yml", "x: 2\r\n")]).await;
    let v = Reads::viewer();
    let current = r
        .service
        .content(&v, &lane("sit1"), &nfs("a/one.yml"), ContentSource::Nfs)
        .await
        .unwrap();
    assert_eq!(
        current.bytes.as_deref(),
        Some(&b"x: 2\r\n"[..]),
        "bytes are not normalised"
    );
    assert_eq!(current.format.eol, domain::EolStyle::Crlf);
    assert!(!current.withheld);

    // The previous version, by hash, under the same path.
    let old = r
        .service
        .content(
            &v,
            &lane("sit1"),
            &nfs("a/one.yml"),
            ContentSource::Blob {
                hash: content_hash(b"x: 1\n"),
            },
        )
        .await
        .unwrap();
    assert_eq!(old.bytes.as_deref(), Some(&b"x: 1\n"[..]));
    // The same hash is not served under a path that never had it, and a hash nobody has is not found.
    for (path, hash) in [
        ("a/two.yml", content_hash(b"x: 1\n")),
        ("a/one.yml", content_hash(b"never stored")),
    ] {
        assert_eq!(
            r.service
                .content(&v, &lane("sit1"), &nfs(path), ContentSource::Blob { hash })
                .await
                .unwrap_err(),
            ReadError::NotFound,
            "{path}"
        );
    }
    assert_eq!(
        r.service
            .content(&v, &lane("sit1"), &nfs("a/one.yml"), ContentSource::Baseline)
            .await
            .unwrap_err(),
        ReadError::NotFound,
        "no baseline yet"
    );
    // A baseline names the raw version behind it.
    r.sql(&format!(
        "INSERT INTO baselines (swimlane_id, path, hash, repo, commit_id, blob_hash) VALUES \
         ('sit1', 'a/one.yml', '\\x{h}', 'base', '{c}', '\\x{h}')",
        h = content_hash(b"x: 1\n"),
        c = "a".repeat(40)
    ))
    .await;
    let base = r
        .service
        .content(&v, &lane("sit1"), &nfs("a/one.yml"), ContentSource::Baseline)
        .await
        .unwrap();
    assert_eq!(base.bytes.as_deref(), Some(&b"x: 1\n"[..]));
}

#[tokio::test]
async fn content_from_git_is_read_at_a_ref_through_the_path_mapping() {
    let Some(r) = reads().await else { return };
    let commit = r
        .world
        .base_commit(&[("config/a/one.yml", Some("x: from git\n"))]);
    r.world
        .tenant_commit("sit1", &[("data/config/a/one.yml", Some("x: tenant\n"))]);
    let v = Reads::viewer();
    let read = |repo, git_ref: &str| {
        let service = r.service.clone();
        let (v, git_ref) = (v.clone(), GitRef::parse(git_ref).unwrap());
        async move {
            service
                .content(
                    &v,
                    &lane("sit1"),
                    &nfs("a/one.yml"),
                    ContentSource::Git { repo, git_ref },
                )
                .await
        }
    };
    assert_eq!(
        read(RepoKind::Base, "master").await.unwrap().bytes.as_deref(),
        Some(&b"x: from git\n"[..]),
        "a branch"
    );
    assert_eq!(
        read(RepoKind::Base, commit.as_str())
            .await
            .unwrap()
            .bytes
            .as_deref(),
        Some(&b"x: from git\n"[..]),
        "a commit id"
    );
    assert_eq!(
        read(RepoKind::Tenant, "sit1").await.unwrap().bytes.as_deref(),
        Some(&b"x: tenant\n"[..]),
        "the tenant repository, on the tenant's branch"
    );
    assert_eq!(
        read(RepoKind::Base, "no-such-branch").await.unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.service
            .content(
                &v,
                &lane("sit1"),
                &nfs("a/two.yml"),
                ContentSource::Git {
                    repo: RepoKind::Base,
                    git_ref: GitRef::parse("master").unwrap()
                }
            )
            .await
            .unwrap_err(),
        ReadError::NotFound,
        "Git has no such file"
    );
}

#[tokio::test]
async fn a_denied_file_is_withheld_whatever_the_source() {
    let Some(mut r) = reads().await else { return };
    // The agent says nothing about it; the name is one the hub's own deny globs know.
    r.world
        .nfs_raw(|at| {
            vec![support::rig::entry(
                "keys/server.pem",
                b"-----BEGIN PRIVATE KEY-----",
                at,
            )]
        })
        .await;
    let v = Reads::viewer();
    let withheld = r
        .service
        .content(&v, &lane("sit1"), &nfs("keys/server.pem"), ContentSource::Nfs)
        .await
        .unwrap();
    assert!(withheld.withheld && withheld.bytes.is_none());
    assert_eq!(withheld.class, domain::FileClass::Denied);
    let tree = r
        .service
        .tree(&v, &lane("sit1"), &nfs("keys"), page(10))
        .await
        .unwrap();
    assert_eq!(tree.items[0].class, domain::FileClass::Denied);
    assert!(tree.items[0].content_withheld);
    let by_hash = r
        .service
        .content(
            &v,
            &lane("sit1"),
            &nfs("keys/server.pem"),
            ContentSource::Blob { hash: withheld.hash },
        )
        .await
        .unwrap();
    assert!(by_hash.withheld && by_hash.bytes.is_none());
}

#[tokio::test]
async fn a_request_that_has_the_tag_gets_not_modified_before_the_blob_is_read() {
    let Some(r) = reads().await else { return };
    let v = Reads::viewer();
    let Tagged::Fresh { etag, .. } = r
        .service
        .content_tagged(&v, &lane("sit1"), &nfs("a/one.yml"), ContentSource::Nfs, None)
        .await
        .unwrap()
    else {
        panic!("no tag was given")
    };
    assert_eq!(
        etag,
        format!("\"{}\"", content_hash(b"x: 1\n")),
        "the tag is the content hash"
    );
    r.world.rig.blobs.forget_requests();
    let again = r
        .service
        .content_tagged(
            &v,
            &lane("sit1"),
            &nfs("a/one.yml"),
            ContentSource::Nfs,
            Some(&etag),
        )
        .await
        .unwrap();
    assert_eq!(again, Tagged::NotModified { etag });
    assert_eq!(
        r.world.rig.blobs.gets(),
        0,
        "a request that has the tag reads no blob"
    );
    // A different tag is a full answer.
    assert!(matches!(
        r.service
            .content_tagged(
                &v,
                &lane("sit1"),
                &nfs("a/one.yml"),
                ContentSource::Nfs,
                Some("\"other\"")
            )
            .await
            .unwrap(),
        Tagged::Fresh { .. }
    ));
    assert_eq!(r.world.rig.blobs.gets(), 1, "a full answer reads the blob once");
}
