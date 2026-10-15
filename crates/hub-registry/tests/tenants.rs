//! The tenants of a swimlane (T10, D84, Q34, A30): where the set comes from, what the checks do with the whole of it, and which member
//! stands for the swimlane where a view needs one. Over a real Postgres 16, the real pipeline, adoption service and read service.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{CompareRef, GridQuery, Page, Role, UserStatus};
use hub_registry::{Bulk, RegistryApi, Resolution};
use ports::conformance::sample::user;
use support::adoption::{AdoptionRig, audit_count, tenants_of};
use support::http::Http;
use support::reads::{Reads, lane, logical, page};
use support::world::World;

fn viewer() -> domain::User {
    user(3, Some(Role::Viewer), UserStatus::Active)
}

/// The open findings of `kind` of the swimlane, as (path, message), in path order.
async fn findings(w: &World, kind: &str) -> Vec<(Option<String>, String)> {
    sqlx::query_as(
        "SELECT path, message FROM findings WHERE swimlane_id = 'sit1' AND kind = $1 AND resolved_at IS NULL \
         ORDER BY path, message LIMIT 1000",
    )
    .bind(kind)
    .fetch_all(&w.rig.pool)
    .await
    .unwrap()
}

/// Git has the base repository and the branches sit1, sit2 and sit3 (and nothing else deployable).
fn git(w: &World) {
    w.base_commit(&[("config/app/core.yml", Some("a: 1\n"))]);
    for (branch, n) in [("sit1", 1), ("sit2", 2), ("sit3", 3)] {
        w.tenant_commit(
            branch,
            &[
                ("data/config/app/core.yml", Some(&format!("t: {n}\n"))),
                ("data/config/app/extra.yml", Some(&format!("e: {n}\n"))),
            ],
        );
    }
}

// ------------------------------------------------------------------------------------------------ where the set comes from

#[tokio::test]
async fn the_set_is_what_the_adoption_confirmed_and_the_cluster_and_nfs_are_only_evidence() {
    let Some(mut r) = AdoptionRig::new(&[]).await else {
        return;
    };
    git(&r.w);
    // NFS carries the files of sit1 and sit2 (suffixes seen), the cluster names sit2 by a Helm release and sit1 by the profile of a Deployment
    // (hints), and the swimlane has no tenant yet.
    r.w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/core-sit2.yml", "t: 2\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
    ])
    .await;
    r.w.rig
        .cluster(support::cluster::report(
            true,
            vec![
                support::cluster::dep("api")
                    .app("api")
                    .env("SPRING_PROFILES_ACTIVE", "sit1,default")
                    .pod("api-1", support::cluster::T0)
                    .build(),
            ],
            None,
        ))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO release_hints (swimlane_id, namespace, service, key, value) \
         VALUES ('sit1', 'csp', 'api', 'chart', 'csp-tenant-data-sit2-1.0.0')",
    )
    .execute(&r.w.rig.pool)
    .await
    .unwrap();
    assert!(tenants_of(&r.w.rig.pool, "sit1").await.is_empty());

    let view = r.start().await;
    let hinted: Vec<(&str, &str)> = view
        .hints
        .iter()
        .map(|h| (h.tenant.as_str(), h.key.as_str()))
        .collect();
    assert_eq!(
        hinted,
        [("sit1", "SPRING_PROFILES_ACTIVE"), ("sit2", "chart")],
        "{:?}",
        view.hints
    );
    let first_two: Vec<&str> = view.ranking.iter().take(2).map(|b| b.branch.as_str()).collect();
    assert_eq!(first_two.len(), 2);
    assert!(
        first_two.contains(&"sit1") && first_two.contains(&"sit2"),
        "{first_two:?}"
    );
    assert!(
        tenants_of(&r.w.rig.pool, "sit1").await.is_empty(),
        "hints and suffixes are evidence the admin is shown: the cluster mentioning a tenant does not add it"
    );

    // The admin's confirmation is what writes the set, with one audit event.
    r.choose(view.id, &["sit1", "sit2"]).await;
    r.resolve_all(view.id, Bulk::default(), Resolution::GitIsRight)
        .await;
    r.svc.scan_secrets(&r.admin, view.id).await.unwrap();
    let before = audit_count(&r.w.rig.pool, "settings_changed").await;
    let outcome = r.svc.confirm(&r.admin, view.id).await.unwrap();
    assert!(outcome.tenants_changed);
    assert_eq!(tenants_of(&r.w.rig.pool, "sit1").await, ["sit1", "sit2"]);
    assert_eq!(
        audit_count(&r.w.rig.pool, "settings_changed").await,
        before + 1,
        "one audit event for the change of the set (A12)"
    );
}

#[tokio::test]
async fn a_suffix_that_is_not_in_the_set_is_an_orphan_and_changes_nothing_else() {
    // C4 is the sign of a suffix the set does not hold (T4 raises the review): the set stays what it was.
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    git(&w);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/core-sit3.yml", "t: 3\n"),
    ])
    .await;
    w.whole().await;
    let orphans = findings(&w, "orphan_file").await;
    assert_eq!(orphans.len(), 1, "{orphans:?}");
    assert_eq!(orphans[0].0.as_deref(), Some("app/core-sit3.yml"));
    assert!(orphans[0].1.contains("sit3"), "{}", orphans[0].1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = 'sit1'")
            .fetch_all(&w.rig.pool)
            .await
            .unwrap(),
        ["sit1"]
    );
}

// ------------------------------------------------------------------------------------------------ C3 and C4 use the whole set

#[tokio::test]
async fn the_whole_set_feeds_c3_and_c4_and_an_admins_change_to_it_is_followed() {
    let Some(mut w) = World::new(&["sit1", "sit2"]).await else {
        return;
    };
    git(&w);
    // sit3 has a file of its own, so that its copy on NFS is a tracked one and is an orphan only by its suffix.
    w.tenant_commit("sit3", &[("data/config/app/other.yml", Some("o: 3\n"))]);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        // sit1 has copies of two files, sit2 of one: C3 for the one sit2 lacks.
        ("app/core-sit1.yml", "t: 1\n"),
        ("app/core-sit2.yml", "t: 2\n"),
        ("app/extra-sit1.yml", "e: 1\n"),
        // sit3's copy: not in the set.
        ("app/other-sit3.yml", "o: 3\n"),
    ])
    .await;
    w.whole().await;

    let uneven = findings(&w, "uneven_tenant_files").await;
    assert_eq!(
        uneven.iter().map(|(p, _)| p.as_deref()).collect::<Vec<_>>(),
        [Some("app/extra.yml")],
        "a logical file that some of the swimlane's tenants have a copy of and others do not: {uneven:?}"
    );
    assert!(
        uneven[0].1.contains("sit1") && uneven[0].1.contains("sit2"),
        "both tenants of the set are in it: {}",
        uneven[0].1
    );
    let orphans = findings(&w, "orphan_file").await;
    assert_eq!(
        orphans.iter().map(|(p, _)| p.as_deref()).collect::<Vec<_>>(),
        [Some("app/other-sit3.yml")],
        "only the suffix that is not in the set (sit2's copy is not an orphan): {orphans:?}"
    );

    // An admin's change goes through `swimlane_tenants` and nothing else: sit3 joins the set. The suffix is no orphan, and the logical files
    // that sit3 has no copy of are uneven now too.
    w.add_tenant("sit3").await;
    w.whole().await;
    assert!(
        findings(&w, "orphan_file").await.is_empty(),
        "sit3 is deployed now"
    );
    let uneven = findings(&w, "uneven_tenant_files").await;
    assert_eq!(
        uneven.iter().map(|(p, _)| p.as_deref()).collect::<Vec<_>>(),
        [Some("app/core.yml"), Some("app/extra.yml"), Some("app/other.yml")],
        "{uneven:?}"
    );

    // One tenant has no unevenness to speak of; a swimlane of one tenant orphans every other suffix.
    sqlx::query("DELETE FROM swimlane_tenants WHERE swimlane_id = 'sit1' AND tenant_id <> 'sit1'")
        .execute(&w.rig.pool)
        .await
        .unwrap();
    w.whole().await;
    assert!(findings(&w, "uneven_tenant_files").await.is_empty());
    let orphans = findings(&w, "orphan_file").await;
    assert_eq!(
        orphans.iter().map(|(p, _)| p.as_deref()).collect::<Vec<_>>(),
        [Some("app/core-sit2.yml"), Some("app/other-sit3.yml")]
    );
}

// ------------------------------------------------------------------------------------------------ the tenant that stands in (A30)

#[tokio::test]
async fn the_first_tenant_in_sorted_order_stands_in_for_the_swimlane_in_every_view_and_the_answer_names_it() {
    // The tenants are stored out of order, and every view that needs one tenant of the swimlane takes the same one.
    let Some(mut h) = Http::new(&["sit3", "sit1", "sit2"]).await else {
        return;
    };
    h.reads
        .world
        .inventory(&[
            ("app/core.yml", "port: 0\n"),
            ("app/core-sit1.yml", "port: 1\n"),
            ("app/core-sit2.yml", "port: 2\n"),
            ("app/core-sit3.yml", "port: 3\n"),
        ])
        .await;
    let api = &h.reads.service;
    let (v, s) = (viewer(), lane("sit1"));
    let file = logical("app/core.yml");

    let effective = api
        .effective_tagged(&v, &s, &file, None, None, None)
        .await
        .unwrap();
    let domain_tenant = match effective {
        hub_registry::Tagged::Fresh { value, .. } => value.tenant,
        hub_registry::Tagged::NotModified { .. } => panic!("no tag was sent"),
    };
    assert_eq!(domain_tenant.as_str(), "sit1");

    let grid = api
        .grid_named(
            &v,
            GridQuery {
                file: file.clone(),
                swimlanes: vec![s.clone()],
                channel: None,
                only_differing: false,
                page: Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(grid.tenants, [(s.clone(), domain_tenant.clone())]);

    let compared = api
        .compare_tagged(
            &v,
            CompareRef::Effective(s.clone()),
            CompareRef::Nfs(s.clone()),
            &file,
            None,
            None,
        )
        .await
        .unwrap();
    let named = match compared {
        hub_registry::Tagged::Fresh { value, .. } => value,
        hub_registry::Tagged::NotModified { .. } => panic!("no tag was sent"),
    };
    assert!(
        named.tenants.iter().all(|(_, t)| *t == domain_tenant),
        "{:?}",
        named.tenants
    );
    let tree = api
        .compare_tree_under(
            &v,
            CompareRef::Effective(s.clone()),
            CompareRef::Nfs(s.clone()),
            &domain::NfsPath::root(),
            page(100),
        )
        .await
        .unwrap();
    assert!(
        tree.tenants.iter().all(|(_, t)| *t == domain_tenant),
        "{:?}",
        tree.tenants
    );

    // On the wire: the header says the same as the answers above.
    let reply = h.view("/api/v1/grid?file=app%2Fcore.yml&swimlanes=sit1").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("x-lanekeeper-tenants"), Some("sit1=sit1"));
    let reply = h
        .view("/api/v1/compare?left=effective%3Asit1&right=nfs%3Asit1&file=app%2Fcore.yml")
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.header("x-lanekeeper-tenants"), Some("sit1=sit1"));
    // (The REST route for the effective config takes the tenant as a required parameter: the default is the service's, tested above.)
}

#[tokio::test]
async fn a_swimlane_with_no_tenant_has_none_standing_for_it() {
    let Some(mut r) = Reads::new(&[]).await else {
        return;
    };
    r.world.inventory(&[("app/core.yml", "port: 0\n")]).await;
    let got = r
        .service
        .effective_tagged(
            &viewer(),
            &lane("sit1"),
            &logical("app/core.yml"),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(
        got.unwrap_err(),
        ports::ReadError::NotFound,
        "no tenant, no effective config"
    );
}
