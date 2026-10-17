//! The six methods that `contract/07-mcp` added to `RegistryRead` (`effective_for`, `compare_in`, `compare_tree_under`, `drift`,
//! `auto_notify`, `served`), answered by the real service through the port, over a real Postgres 16 (S4, S10, D79, D82, D83, D85, D86,
//! D104). MCP calls the port and never the REST handlers, so what the registry specifically does for each is pinned here: how a
//! tenant and a channel are resolved, how a prefix and a cursor page, which files are drift and in which order, what auto-notify says
//! of a swimlane nobody switched on, and what the served file is when the agent says yes, no, nothing or too much.
//!
//! The REST operations behind them keep answering as before (their own tests, `http_contract` and the walker, are not edited); here
//! each port answer is also compared with the answer of the `RegistryApi` method that the REST handler calls, so that the two cannot
//! drift apart.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use domain::{
    AgentReply, AppName, ChannelName, CompareQuery, CompareRef, CompareState, DriftQuery, DriftState,
    EffectiveQuery, FileClass, FileEntry, FileRole, HubCommand, NfsPath, Page, Role, ServeRequest,
    SettingPath, TenantId, TreeComparison, User, UserStatus,
};
use hub_registry::config::RegistryConfig;
use hub_registry::{RegistryApi, Tagged};
use ports::conformance::sample::user as sample_user;
use ports::{ReadError, RegistryRead};
use support::reads::{Reads, lane, logical, nfs, page};

fn viewer() -> User {
    Reads::viewer()
}

fn tenant(t: &str) -> TenantId {
    TenantId::parse(t).unwrap()
}

fn channel(c: &str) -> ChannelName {
    ChannelName::parse(c).unwrap()
}

fn setting(p: &str) -> SettingPath {
    SettingPath::parse(p).unwrap()
}

fn nfs_of(s: &str) -> CompareRef {
    CompareRef::Nfs(lane(s))
}

fn effective_of(s: &str) -> CompareRef {
    CompareRef::Effective(lane(s))
}

/// Users the port must refuse, whatever they ask for.
fn refused() -> [User; 3] {
    [
        sample_user(2, Some(Role::Admin), UserStatus::Pending),
        sample_user(3, Some(Role::Admin), UserStatus::Disabled),
        sample_user(4, None, UserStatus::Active),
    ]
}

/// An application `core`, so that `core.yml` and `core-<tenant>.yml` are property sources and `core/...` files are resources.
async fn with_app(r: &Reads, swimlane: &str) {
    r.sql(&format!(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         VALUES ('{swimlane}', 'ns', 'core', '{{\"SPRING_APPLICATION_NAME\": \"core\"}}')"
    ))
    .await;
}

// ------------------------------------------------------------------------------------------------ S4

#[tokio::test]
async fn the_six_refuse_a_user_who_may_not_read_before_they_look_at_anything_else() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world.inventory(&[("core.yml", "port: 1\n")]).await;
    // Arguments that would be `NotFound` or `BadRequest` for a user who may read: the refusal comes first.
    let nowhere = lane("nowhere");
    let q_effective = EffectiveQuery {
        file: logical("core.yml"),
        tenant: tenant("sit9"),
        channel: None,
        setting_prefix: None,
    };
    let q_compare = CompareQuery {
        left: nfs_of("nowhere"),
        right: nfs_of("sit1"),
        file: None,
        channel: None,
    };
    let q_drift = DriftQuery {
        state: None,
        prefix: None,
    };
    let request = ServeRequest {
        application: AppName::parse("core").unwrap(),
        tenant: tenant("sit9"),
        channel: None,
        file: nfs("core.yml"),
    };
    for user in refused() {
        let s = &r.service;
        assert_eq!(
            s.effective_for(&user, &nowhere, &q_effective).await.unwrap_err(),
            ReadError::Forbidden,
            "effective_for"
        );
        assert_eq!(
            s.compare_in(&user, &q_compare).await.unwrap_err(),
            ReadError::Forbidden,
            "compare_in"
        );
        assert_eq!(
            s.compare_tree_under(&user, nfs_of("nowhere"), nfs_of("sit1"), &nfs("a"), page(10))
                .await
                .unwrap_err(),
            ReadError::Forbidden,
            "compare_tree_under"
        );
        assert_eq!(
            s.drift(&user, &nowhere, &q_drift, page(10)).await.unwrap_err(),
            ReadError::Forbidden,
            "drift"
        );
        assert_eq!(
            s.auto_notify(&user, &nowhere).await.unwrap_err(),
            ReadError::Forbidden,
            "auto_notify"
        );
        assert_eq!(
            s.served(&user, &nowhere, &request).await.unwrap_err(),
            ReadError::Forbidden,
            "served"
        );
    }
    // And no agent was asked on behalf of a user who may not read.
    assert!(r.gateway.received(&lane("sit1")).is_empty());
}

#[tokio::test]
async fn every_role_that_may_read_may_ask_the_six() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world.inventory(&[("core.yml", "port: 1\n")]).await;
    for role in [Role::Viewer, Role::Editor, Role::Operator, Role::Admin] {
        let u = sample_user(9, Some(role), UserStatus::Active);
        assert!(
            !r.service.auto_notify(&u, &lane("sit1")).await.unwrap(),
            "{role:?}"
        );
        assert!(
            r.service
                .drift(
                    &u,
                    &lane("sit1"),
                    &DriftQuery {
                        state: None,
                        prefix: None
                    },
                    page(10)
                )
                .await
                .is_ok(),
            "{role:?}"
        );
    }
}

// ------------------------------------------------------------------------------------------------ auto_notify

#[tokio::test]
async fn auto_notify_is_the_swimlanes_setting_and_off_when_nobody_switched_it_on() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world.inventory(&[("core.yml", "port: 1\n")]).await;
    r.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    let v = viewer();
    assert!(
        !r.service.auto_notify(&v, &lane("sit1")).await.unwrap(),
        "unset is off (D85)"
    );
    r.sql("UPDATE swimlanes SET auto_notify = true WHERE id = 'sit1'")
        .await;
    assert!(r.service.auto_notify(&v, &lane("sit1")).await.unwrap());
    assert!(
        !r.service.auto_notify(&v, &lane("sit2")).await.unwrap(),
        "it is the swimlane's own setting, not a global one"
    );
    // The REST detail says the same.
    assert!(r.service.swimlane(&v, &lane("sit1")).await.unwrap().auto_notify);
    r.sql("UPDATE swimlanes SET auto_notify = false WHERE id = 'sit1'")
        .await;
    assert!(!r.service.auto_notify(&v, &lane("sit1")).await.unwrap());
    assert_eq!(
        r.service.auto_notify(&v, &lane("nowhere")).await.unwrap_err(),
        ReadError::NotFound
    );
}

// ------------------------------------------------------------------------------------------------ effective_for

const NESTED: &str = "db:\n  host: h\n  port: 1\ndbx:\n  y: 2\nlist:\n  - a\n  - b\nlisting: z\n";

fn query(file: &str, t: &str, channel: Option<&str>, prefix: Option<&str>) -> EffectiveQuery {
    EffectiveQuery {
        file: logical(file),
        tenant: tenant(t),
        channel: channel.map(self::channel),
        setting_prefix: prefix.map(setting),
    }
}

async fn tagged_value(r: &Reads, q: &EffectiveQuery) -> domain::EffectiveConfig {
    match r
        .service
        .effective_tagged(
            &viewer(),
            &lane("sit1"),
            &q.file,
            Some(&q.tenant),
            q.channel.as_ref(),
            None,
        )
        .await
        .unwrap()
    {
        Tagged::Fresh { value, .. } => value,
        Tagged::NotModified { .. } => panic!("no tag was given"),
    }
}

#[tokio::test]
async fn effective_for_answers_for_the_tenant_and_channel_asked_and_names_them() {
    let Some(mut r) = Reads::new(&["sit2", "sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world
        .inventory(&[
            ("core.yml", "name: base\nport: 2\n"),
            ("core-sit1.yml", "name: one\n"),
            ("core-sit2.yml", "name: two\n"),
            ("core/msg.txt", "say ${name}\n"),
            ("core/chan/msg.txt", "channel says ${name}\n"),
        ])
        .await;
    let v = viewer();
    let s = lane("sit1");

    // The tenant is the one asked for; the port's `effective` has no tenant and gets the standing one (A30).
    let two = r
        .service
        .effective_for(&v, &s, &query("core.yml", "sit2", None, None))
        .await
        .unwrap();
    assert_eq!((two.tenant.as_str(), two.file.as_str()), ("sit2", "core.yml"));
    assert_eq!(two.swimlane, s);
    assert_eq!(two.channel, None);
    assert_eq!(
        two.settings
            .iter()
            .find(|x| x.path.as_str() == "name")
            .unwrap()
            .value,
        "two"
    );
    let one = r
        .service
        .effective_for(&v, &s, &query("core.yml", "sit1", None, None))
        .await
        .unwrap();
    assert_eq!(
        one.settings
            .iter()
            .find(|x| x.path.as_str() == "name")
            .unwrap()
            .value,
        "one"
    );
    let standing = r.service.effective(&v, &s, &logical("core.yml")).await.unwrap();
    assert_eq!(
        standing, one,
        "no tenant is the first in sorted order, which is sit1"
    );
    assert_ne!(one.hash, two.hash);

    // The same answer as the REST handler's, for the same question.
    for q in [
        query("core.yml", "sit2", None, None),
        query("core/msg.txt", "sit1", None, None),
        query("core/msg.txt", "sit1", Some("chan"), None),
    ] {
        assert_eq!(
            r.service.effective_for(&v, &s, &q).await.unwrap(),
            tagged_value(&r, &q).await,
            "{q:?}"
        );
    }

    // A channel: the copy in the channel's folder wins, and the answer names the channel.
    let plain = r
        .service
        .effective_for(&v, &s, &query("core/msg.txt", "sit1", None, None))
        .await
        .unwrap();
    let chan = r
        .service
        .effective_for(&v, &s, &query("core/msg.txt", "sit1", Some("chan"), None))
        .await
        .unwrap();
    assert_eq!(plain.role, FileRole::Resource);
    assert_eq!(chan.channel, Some(channel("chan")));
    assert_eq!(plain.channel, None);
    assert_eq!(chan.sources[0].path.as_str(), "core/chan/msg.txt");
    assert_eq!(plain.sources[0].path.as_str(), "core/msg.txt");
    assert_ne!(plain.hash, chan.hash);
    // A resource has no settings, so a prefix selects nothing of it and the rest of the answer is the same.
    let cut = r
        .service
        .effective_for(&v, &s, &query("core/msg.txt", "sit1", Some("chan"), Some("name")))
        .await
        .unwrap();
    assert!(cut.settings.is_empty());
    assert_eq!(cut.hash, chan.hash);

    // Not found: a tenant the swimlane does not deploy, a file that is not there, a swimlane that is not there.
    assert_eq!(
        r.service
            .effective_for(&v, &s, &query("core.yml", "sit9", None, None))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.service
            .effective_for(&v, &s, &query("missing.yml", "sit1", None, None))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        r.service
            .effective_for(&v, &lane("nowhere"), &query("core.yml", "sit1", None, None))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

#[tokio::test]
async fn effective_for_cuts_the_settings_at_a_prefix_by_path_components_and_nothing_else() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world.inventory(&[("core.yml", NESTED)]).await;
    let (v, s) = (viewer(), lane("sit1"));
    let paths = |e: &domain::EffectiveConfig| -> Vec<String> {
        e.settings.iter().map(|x| x.path.to_string()).collect()
    };
    let all = r
        .service
        .effective_for(&v, &s, &query("core.yml", "sit1", None, None))
        .await
        .unwrap();
    assert_eq!(
        paths(&all),
        ["db.host", "db.port", "dbx.y", "list[0]", "list[1]", "listing"],
        "no prefix: every setting"
    );
    for (prefix, want) in [
        ("db", &["db.host", "db.port"][..]),
        ("db.host", &["db.host"][..]),
        ("dbx", &["dbx.y"][..]),
        ("list", &["list[0]", "list[1]"][..]),
        ("list[1]", &["list[1]"][..]),
        ("listing", &["listing"][..]),
        ("d", &[][..]),
        ("db.ho", &[][..]),
        ("nothing", &[][..]),
    ] {
        let cut = r
            .service
            .effective_for(&v, &s, &query("core.yml", "sit1", None, Some(prefix)))
            .await
            .unwrap();
        assert_eq!(paths(&cut), want, "prefix {prefix}");
        // Only `settings` is cut: what the file is, where it came from and its hash are the whole answer's.
        assert_eq!(
            (cut.hash, &cut.sources, &cut.role, &cut.tenant),
            (all.hash, &all.sources, &all.role, &all.tenant)
        );
        for kept in &cut.settings {
            assert!(
                all.settings.contains(kept),
                "a kept setting is the whole answer's, unchanged"
            );
        }
    }
}

#[tokio::test]
async fn a_flagged_value_stays_masked_when_a_prefix_selects_it() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    let secret = support::http::SECRET;
    r.world
        .inventory(&[("core.yml", &format!("visible: fine\ntoken: {secret}\n"))])
        .await;
    let (v, s) = (viewer(), lane("sit1"));
    for indexed in [false, true] {
        if indexed {
            r.index_all().await;
            r.service.clear_caches();
        }
        for prefix in [None, Some("token")] {
            let e = r
                .service
                .effective_for(&v, &s, &query("core.yml", "sit1", None, prefix))
                .await
                .unwrap();
            let token = e.settings.iter().find(|x| x.path.as_str() == "token").unwrap();
            assert!(
                token.value.starts_with("[REDACTED: ") && !token.value.contains(secret),
                "indexed={indexed} prefix={prefix:?}: {}",
                token.value
            );
            assert!(!serde_json::to_string(&e).unwrap().contains(secret));
        }
    }
}

// ------------------------------------------------------------------------------------------------ compare_in

async fn two_lanes(sit1: &[(&str, &str)], sit2: &[(&str, &str)]) -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    r.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    for id in ["sit1", "sit2"] {
        with_app(&r, id).await;
    }
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
    r.world.inventory(sit1).await;
    r.world.second_lane("sit2", &["sit2"], sit2).await;
    Some(r)
}

fn compare_query(l: CompareRef, rr: CompareRef, file: Option<&str>, channel: Option<&str>) -> CompareQuery {
    CompareQuery {
        left: l,
        right: rr,
        file: file.map(logical),
        channel: channel.map(self::channel),
    }
}

#[tokio::test]
async fn compare_in_is_the_compare_of_the_rest_handler_and_serves_a_channel_its_own_copy() {
    let Some(r) = two_lanes(
        &[
            ("core.yml", "port: 1\n"),
            ("core/msg.txt", "hello\n"),
            ("core/chan/msg.txt", "channel hello\n"),
        ],
        &[("core.yml", "port: 2\n"), ("core/msg.txt", "hello\n")],
    )
    .await
    else {
        return;
    };
    let v = viewer();

    // No channel: the same answer as `compare`, and as the handler's.
    let q = compare_query(nfs_of("sit1"), nfs_of("sit2"), Some("core.yml"), None);
    let in_port = r.service.compare_in(&v, &q).await.unwrap();
    assert_eq!(
        in_port,
        r.service
            .compare(&v, nfs_of("sit1"), nfs_of("sit2"), Some(logical("core.yml")))
            .await
            .unwrap()
    );
    assert!(!in_port.identical);
    assert_eq!(in_port.file, Some(logical("core.yml")));
    assert_eq!(
        (in_port.left.clone(), in_port.right.clone()),
        (nfs_of("sit1"), nfs_of("sit2"))
    );

    // A channel: the copy in the channel's folder is what the left side is. Without it the two sides are the same file.
    let plain = r
        .service
        .compare_in(
            &v,
            &compare_query(nfs_of("sit1"), nfs_of("sit2"), Some("core/msg.txt"), None),
        )
        .await
        .unwrap();
    assert!(plain.identical, "no channel: both sides have core/msg.txt");
    let q = compare_query(nfs_of("sit1"), nfs_of("sit2"), Some("core/msg.txt"), Some("chan"));
    let in_channel = r.service.compare_in(&v, &q).await.unwrap();
    assert!(!in_channel.identical, "the channel's copy differs");
    assert!(
        in_channel
            .hunks
            .iter()
            .any(|h| format!("{h:?}").contains("channel hello")),
        "the hunks are the channel copy's: {:?}",
        in_channel.hunks
    );
    let Tagged::Fresh { value, .. } = r
        .service
        .compare_tagged(
            &v,
            nfs_of("sit1"),
            nfs_of("sit2"),
            &logical("core/msg.txt"),
            Some(&channel("chan")),
            None,
        )
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert_eq!(
        in_channel, value.value,
        "the handler's answer, for the same question"
    );

    // The effective sides too (the port's `compare` has no channel at all).
    let q = compare_query(
        effective_of("sit1"),
        effective_of("sit2"),
        Some("core/msg.txt"),
        Some("chan"),
    );
    assert!(!r.service.compare_in(&v, &q).await.unwrap().identical);
}

#[tokio::test]
async fn compare_in_needs_a_file_and_says_not_found_for_what_is_not_there() {
    let Some(r) = two_lanes(&[("core.yml", "port: 1\n")], &[("core.yml", "port: 2\n")]).await else {
        return;
    };
    let v = viewer();
    // A comparison of one file needs the file (a whole tree is `compare_tree_under`), as `compare` says.
    assert_eq!(
        r.service
            .compare_in(&v, &compare_query(nfs_of("sit1"), nfs_of("sit2"), None, None))
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
    assert_eq!(
        r.service
            .compare(&v, nfs_of("sit1"), nfs_of("sit2"), None)
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "the same rule as the port's compare"
    );
    for q in [
        compare_query(nfs_of("sit1"), nfs_of("sit9"), Some("core.yml"), None),
        compare_query(nfs_of("sit1"), nfs_of("sit2"), Some("missing.yml"), None),
    ] {
        assert_eq!(
            r.service.compare_in(&v, &q).await.unwrap_err(),
            ReadError::NotFound,
            "{q:?}"
        );
    }
}

#[tokio::test]
async fn compare_in_reads_the_standing_tenants_copy_of_a_swimlane_that_deploys_several() {
    // `sit1` deploys `sit2` and `sit1`: the first in sorted order stands for it (A30, D106), whatever order the rows come in.
    let Some(mut r) = Reads::new(&["sit2", "sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world
        .inventory(&[
            ("core.yml", "name: base\n"),
            ("core-sit1.yml", "name: one\n"),
            ("core-sit2.yml", "name: two\n"),
        ])
        .await;
    // Another swimlane whose own file says what `sit1`'s standing tenant says.
    r.world
        .second_lane("sit9", &["sit9"], &[("core.yml", "name: one\n")])
        .await;
    let v = viewer();
    let q = compare_query(nfs_of("sit1"), nfs_of("sit9"), Some("core.yml"), None);
    assert!(
        r.service.compare_in(&v, &q).await.unwrap().identical,
        "sit1 is read as its tenant sit1 (`name: one`), not as sit2 (`name: two`)"
    );
    let other = compare_query(nfs_of("sit1"), nfs_of("sit1"), Some("core.yml"), None);
    assert!(r.service.compare_in(&v, &other).await.unwrap().identical);
    // The handler names the tenant that stood for each swimlane, for the header; the port has no place for it and drops it.
    let Tagged::Fresh {
        value: hub_registry::Named { tenants, .. },
        ..
    } = r
        .service
        .compare_tagged(
            &v,
            nfs_of("sit1"),
            nfs_of("sit9"),
            &logical("core.yml"),
            None,
            None,
        )
        .await
        .unwrap()
    else {
        panic!("no tag was given");
    };
    assert_eq!(
        tenants,
        [(lane("sit1"), tenant("sit1")), (lane("sit9"), tenant("sit9"))]
    );
}

// ------------------------------------------------------------------------------------------------ compare_tree_under

fn entries(t: &TreeComparison) -> Vec<(String, CompareState)> {
    t.entries
        .items
        .iter()
        .map(|e| (e.file.to_string(), e.state))
        .collect()
}

async fn tree_pages(
    r: &Reads,
    l: &CompareRef,
    rr: &CompareRef,
    prefix: &NfsPath,
    size: u32,
) -> Vec<(String, CompareState)> {
    let mut out = Vec::new();
    let mut cursor = None;
    for _ in 0..100 {
        let t = r
            .service
            .compare_tree_under(
                &viewer(),
                l.clone(),
                rr.clone(),
                prefix,
                Page::new(size, cursor.clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(
            t.entries.items.len() <= size as usize,
            "a page never exceeds its limit"
        );
        out.extend(entries(&t));
        match t.entries.next {
            Some(c) => cursor = Some(c),
            None => return out,
        }
    }
    panic!("the pages did not end");
}

#[tokio::test]
async fn compare_tree_under_lists_what_the_whole_comparison_lists_below_a_folder_and_pages_by_keyset() {
    let Some(r) = two_lanes(
        &[
            ("a/same.yml", "x: 1\n"),
            ("a/diff.yml", "x: 1\n"),
            ("a/left.yml", "x: 1\n"),
            ("a/deep/one.yml", "x: 1\n"),
            ("ab/left.yml", "x: 1\n"),
            ("b/diff.yml", "x: 1\n"),
            ("top.yml", "x: 1\n"),
        ],
        &[
            ("a/same.yml", "x: 1\n"),
            ("a/diff.yml", "x: 2\n"),
            ("a/right.yml", "x: 1\n"),
            ("a/deep/one.yml", "x: 2\n"),
            ("ab/left.yml", "x: 2\n"),
            ("b/diff.yml", "x: 2\n"),
            ("top.yml", "x: 2\n"),
        ],
    )
    .await
    else {
        return;
    };
    // The effective sides read what the pipeline computed.
    for id in ["sit1", "sit2"] {
        r.world
            .pipeline
            .process(vec![(lane(id), hub_registry::Work::Whole)])
            .await
            .unwrap();
    }
    let v = viewer();
    for (l, rr) in [
        (nfs_of("sit1"), nfs_of("sit2")),
        (effective_of("sit1"), effective_of("sit2")),
    ] {
        let whole = r
            .service
            .compare_tree(&v, l.clone(), rr.clone(), Page::new(200, None).unwrap())
            .await
            .unwrap();
        let whole = entries(&whole);
        assert!(whole.len() >= 7, "{l:?}: {whole:?}");
        assert!(
            whole.windows(2).all(|w| w[0].0 < w[1].0),
            "the whole comparison is ordered by path"
        );

        // Below `a`: the same entries, in the same order, and not those of `ab/` (a folder is whole path components).
        let below_a: Vec<_> = whole.iter().filter(|e| e.0.starts_with("a/")).cloned().collect();
        assert!(!below_a.is_empty());
        assert!(
            whole.iter().any(|e| e.0.starts_with("ab/")),
            "{l:?}: the control is in the whole comparison"
        );
        let got = tree_pages(&r, &l, &rr, &nfs("a"), 200).await;
        assert_eq!(got, below_a, "{l:?}");
        // One at a time visits each once, in order.
        assert_eq!(tree_pages(&r, &l, &rr, &nfs("a"), 1).await, below_a, "{l:?}");
        assert_eq!(
            tree_pages(&r, &l, &rr, &nfs("a/deep"), 1).await,
            [("a/deep/one.yml".to_owned(), CompareState::Different)]
        );
        // The root prefix is the whole comparison, whatever the port method it came through.
        assert_eq!(tree_pages(&r, &l, &rr, &NfsPath::root(), 3).await, whole, "{l:?}");
        // A folder with nothing below it differing is an empty list, not an error.
        assert!(
            tree_pages(&r, &l, &rr, &nfs("no/such/folder"), 10)
                .await
                .is_empty()
        );

        // The handler's answer, for the same question.
        let named = r
            .service
            .compare_tree_named(
                &v,
                l.clone(),
                rr.clone(),
                &nfs("a"),
                Page::new(200, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(entries(&named.value), below_a);
        let port = r
            .service
            .compare_tree_under(
                &v,
                l.clone(),
                rr.clone(),
                &nfs("a"),
                Page::new(200, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(port, named.value);
    }

    // A cursor is bound to its prefix: the page after `a`'s first is not a page of `b`, nor of the whole.
    let first = r
        .service
        .compare_tree_under(&v, nfs_of("sit1"), nfs_of("sit2"), &nfs("a"), page(1))
        .await
        .unwrap();
    let cursor = first.entries.next.clone().unwrap();
    for prefix in [nfs("b"), NfsPath::root()] {
        assert_eq!(
            r.service
                .compare_tree_under(
                    &v,
                    nfs_of("sit1"),
                    nfs_of("sit2"),
                    &prefix,
                    Page::new(1, Some(cursor.clone())).unwrap()
                )
                .await
                .unwrap_err(),
            ReadError::BadRequest,
            "{prefix}"
        );
    }
    assert_eq!(
        r.service
            .compare_tree_under(&v, nfs_of("sit1"), nfs_of("sit9"), &nfs("a"), page(1))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

// ------------------------------------------------------------------------------------------------ drift

const DRIFT_FILES: [(&str, &str); 8] = [
    ("a/one.yml", "x: 1\n"),
    ("a/two.yml", "y: 2\n"),
    ("a/deep/five.yml", "z: 5\n"),
    ("ab/four.yml", "v: 4\n"),
    ("b/three.yml", "w: 3\n"),
    ("top.properties", "k=v\n"),
    ("weird_%/x.yml", "q: 1\n"),
    ("zed.yml", "s: 1\n"),
];

/// Six files that are not in sync, in three states, whose severities put them in another order than their paths, and one of which was
/// created last and sorts first: the REST list is most serious first, then in creation order (D103), and the port's is by path.
async fn drift_world() -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    r.world.inventory(&DRIFT_FILES).await;
    // A file that arrives after the inventory has the last `seq` and the first path: creation order is not path order.
    r.world.nfs_change(&[("0-late.yml", "l: 1\n")]).await;
    r.sql(
        "UPDATE file_state SET state = 'conflict', severity = 'high' WHERE swimlane_id = 'sit1' AND path = 'a/deep/five.yml'; \
         UPDATE file_state SET state = 'git_ahead', severity = 'low' WHERE swimlane_id = 'sit1' AND path = 'a/two.yml'; \
         UPDATE file_state SET state = 'git_ahead', severity = 'medium' WHERE swimlane_id = 'sit1' AND path = 'ab/four.yml'; \
         UPDATE file_state SET state = 'nfs_ahead', severity = 'critical' WHERE swimlane_id = 'sit1' AND path = 'b/three.yml'; \
         UPDATE file_state SET state = 'conflict', severity = 'low' WHERE swimlane_id = 'sit1' AND path = 'weird_%/x.yml'; \
         UPDATE file_state SET state = 'git_ahead', severity = 'low' WHERE swimlane_id = 'sit1' AND path = '0-late.yml'; \
         UPDATE file_state SET state = 'in_sync' WHERE swimlane_id = 'sit1' AND path IN ('a/one.yml', 'top.properties', 'zed.yml')",
    )
    .await;
    Some(r)
}

fn drift_query(state: Option<DriftState>, prefix: Option<&str>) -> DriftQuery {
    DriftQuery {
        state,
        prefix: prefix.map(nfs),
    }
}

async fn drift_pages(r: &Reads, q: &DriftQuery, size: u32) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let mut cursor = None;
    for _ in 0..100 {
        let p = r
            .service
            .drift(
                &viewer(),
                &lane("sit1"),
                q,
                Page::new(size, cursor.clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(p.items.len() <= size as usize, "a page never exceeds its limit");
        out.extend(p.items);
        match p.next {
            Some(c) => cursor = Some(c),
            None => return out,
        }
    }
    panic!("the pages did not end");
}

fn paths(entries: &[FileEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.path.as_str()).collect()
}

#[tokio::test]
async fn drift_lists_the_files_that_are_not_in_sync_by_path_and_the_rest_list_keeps_its_severity_order() {
    let Some(r) = drift_world().await else { return };
    let v = viewer();
    let all = drift_pages(&r, &drift_query(None, None), 100).await;
    assert_eq!(
        paths(&all),
        [
            "0-late.yml",
            "a/deep/five.yml",
            "a/two.yml",
            "ab/four.yml",
            "b/three.yml",
            "weird_%/x.yml"
        ],
        "every file that is not in sync, files only, by path (the port's documentation), not by when its row was made"
    );
    assert!(all.iter().all(|e| !e.is_dir && e.drift != DriftState::InSync));
    // The REST list is the same files, most serious first (D103): the order is the one thing that differs.
    let ranked = r
        .service
        .drift_ranked(&v, &lane("sit1"), None, &NfsPath::root(), page(100))
        .await
        .unwrap();
    assert_eq!(
        paths(&ranked.items),
        [
            "b/three.yml",
            "a/deep/five.yml",
            "ab/four.yml",
            "a/two.yml",
            "weird_%/x.yml",
            "0-late.yml"
        ]
    );
    let mut by_path = ranked.items.clone();
    by_path.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        by_path, all,
        "the same entries, whole: state, attribution and severity included"
    );
    let severity = |p: &str| all.iter().find(|e| e.path.as_str() == p).unwrap().severity;
    assert_eq!(severity("b/three.yml"), Some(domain::Severity::Critical));
    assert!(
        all.iter().all(|e| e.severity.is_some()),
        "every entry carries its severity"
    );
    assert!(all.iter().all(|e| e.attribution.is_some()), "and its attribution");

    // A page size changes nothing but the number of pages.
    for size in [1, 2, 3] {
        assert_eq!(
            drift_pages(&r, &drift_query(None, None), size).await,
            all,
            "page size {size}"
        );
    }
}

#[tokio::test]
async fn drift_filters_by_state_and_by_folder_at_any_depth() {
    let Some(r) = drift_world().await else { return };
    let state = |s| drift_query(Some(s), None);
    assert_eq!(
        paths(&drift_pages(&r, &state(DriftState::GitAhead), 100).await),
        ["0-late.yml", "a/two.yml", "ab/four.yml"]
    );
    assert_eq!(
        paths(&drift_pages(&r, &state(DriftState::Conflict), 100).await),
        ["a/deep/five.yml", "weird_%/x.yml"]
    );
    assert_eq!(
        paths(&drift_pages(&r, &state(DriftState::NfsAhead), 100).await),
        ["b/three.yml"]
    );
    assert!(
        drift_pages(&r, &state(DriftState::Untracked), 100)
            .await
            .is_empty()
    );
    // A state that is "in sync" is asked for by name, and then only those are listed.
    assert_eq!(
        paths(&drift_pages(&r, &state(DriftState::InSync), 100).await),
        ["a/one.yml", "top.properties", "zed.yml"]
    );
    // A folder: components, not a string prefix (`ab/` is not below `a`), at any depth, and `_` and `%` are not wildcards.
    assert_eq!(
        paths(&drift_pages(&r, &drift_query(None, Some("a")), 100).await),
        ["a/deep/five.yml", "a/two.yml"]
    );
    assert_eq!(
        paths(&drift_pages(&r, &drift_query(None, Some("a/deep")), 100).await),
        ["a/deep/five.yml"]
    );
    assert_eq!(
        paths(&drift_pages(&r, &drift_query(None, Some("weird_%")), 100).await),
        ["weird_%/x.yml"]
    );
    assert!(
        drift_pages(&r, &drift_query(None, Some("weirdXY")), 100)
            .await
            .is_empty()
    );
    assert!(
        drift_pages(&r, &drift_query(None, Some("no/such/folder")), 100)
            .await
            .is_empty()
    );
    // Both at once; and the root as a prefix is no prefix.
    assert_eq!(
        paths(&drift_pages(&r, &drift_query(Some(DriftState::GitAhead), Some("a")), 100).await),
        ["a/two.yml"]
    );
    assert_eq!(
        r.service
            .drift(
                &viewer(),
                &lane("sit1"),
                &DriftQuery {
                    state: None,
                    prefix: Some(NfsPath::root())
                },
                page(100)
            )
            .await
            .unwrap()
            .items
            .len(),
        6
    );
}

#[tokio::test]
async fn a_drift_cursor_belongs_to_its_query_and_a_cursor_whose_row_is_gone_is_refused() {
    let Some(r) = drift_world().await else { return };
    let (v, s) = (viewer(), lane("sit1"));
    let first = r
        .service
        .drift(&v, &s, &drift_query(None, None), page(2))
        .await
        .unwrap();
    let cursor = first.next.clone().unwrap();
    let with = |c: &domain::Cursor| Page::new(2, Some(c.clone())).unwrap();
    assert_eq!(paths(&first.items), ["0-late.yml", "a/deep/five.yml"]);
    // The next page of the same query resumes after the second file.
    let second = r
        .service
        .drift(&v, &s, &drift_query(None, None), with(&cursor))
        .await
        .unwrap();
    assert_eq!(paths(&second.items), ["a/two.yml", "ab/four.yml"]);
    // Another state, another prefix, another swimlane, the REST list's cursor and a garbage one are all refused.
    for (name, q) in [
        ("another state", drift_query(Some(DriftState::Conflict), None)),
        ("another prefix", drift_query(None, Some("a"))),
    ] {
        assert_eq!(
            r.service.drift(&v, &s, &q, with(&cursor)).await.unwrap_err(),
            ReadError::BadRequest,
            "{name}"
        );
    }
    r.world.rig.add_swimlane("sit2", &["sit1"], true).await;
    assert_eq!(
        r.service
            .drift(&v, &lane("sit2"), &drift_query(None, None), with(&cursor))
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "another swimlane"
    );
    let ranked_first = r
        .service
        .drift_ranked(&v, &s, None, &NfsPath::root(), page(2))
        .await
        .unwrap();
    assert_eq!(
        r.service
            .drift(
                &v,
                &s,
                &drift_query(None, None),
                with(&ranked_first.next.unwrap())
            )
            .await
            .unwrap_err(),
        ReadError::BadRequest,
        "the other order's cursor"
    );
    assert_eq!(
        r.service
            .drift(
                &v,
                &s,
                &drift_query(None, None),
                with(&domain::Cursor::parse("not-a-cursor").unwrap())
            )
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
    // The row the cursor points after has gone: the position is lost, and that is said instead of an empty page.
    r.sql("DELETE FROM file_state WHERE swimlane_id = 'sit1' AND path = 'a/deep/five.yml'")
        .await;
    assert_eq!(
        r.service
            .drift(&v, &s, &drift_query(None, None), with(&cursor))
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
    assert_eq!(
        r.service
            .drift(&v, &lane("nowhere"), &drift_query(None, None), page(1))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
}

#[tokio::test]
async fn a_drift_entry_names_a_denied_file_as_denied_whether_its_row_or_the_hubs_globs_say_so() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world
        .inventory(&[
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nAAAA\n"),
            ("core.yml", "port: 1\n"),
            ("other.yml", "a: 1\n"),
        ])
        .await;
    r.sql("UPDATE file_state SET state = 'nfs_ahead', severity = 'critical' WHERE swimlane_id = 'sit1'")
        .await;
    let q = drift_query(None, None);
    let entry =
        |entries: &[FileEntry], p: &str| entries.iter().find(|e| e.path.as_str() == p).unwrap().clone();

    // The agent said the key is denied (and the hub's own globs say so): its name, size and hash are listed and its content is withheld.
    let plain = drift_pages(&r, &q, 100).await;
    let pem = entry(&plain, "keys/server.pem");
    assert_eq!((pem.class, pem.content_withheld), (FileClass::Denied, true));
    assert!(pem.hash.is_some() && pem.size > 0, "name, size and hash stay");
    for p in ["core.yml", "other.yml"] {
        let e = entry(&plain, p);
        assert!(
            !e.content_withheld && e.class != FileClass::Denied,
            "{p}: an ordinary file"
        );
    }

    // The hub restarts with one more glob and no agent has scanned since: the row says "ordinary", the hub's glob decides.
    let globbed = r.service_with(RegistryConfig {
        extra_deny_globs: vec!["core.yml".to_owned()],
        ..RegistryConfig::default()
    });
    let page = globbed
        .drift(&viewer(), &lane("sit1"), &q, page(100))
        .await
        .unwrap();
    let core = entry(&page.items, "core.yml");
    assert_eq!(
        (core.class, core.content_withheld),
        (FileClass::Denied, true),
        "S17, D104"
    );
    assert!(!entry(&page.items, "other.yml").content_withheld);
}

// ------------------------------------------------------------------------------------------------ served

fn request(file: &str) -> ServeRequest {
    ServeRequest {
        application: AppName::parse("core").unwrap(),
        tenant: tenant("sit1"),
        channel: None,
        file: nfs(file),
    }
}

type Script = Arc<dyn Fn(&HubCommand) -> Option<AgentReply> + Send + Sync>;

fn answer(status: u16, body: &'static [u8]) -> Script {
    Arc::new(move |cmd: &HubCommand| match cmd {
        HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
            request_id: request_id.clone(),
            status,
            bytes: Bytes::from_static(body),
        }),
        _ => None,
    })
}

async fn served_world(config: RegistryConfig) -> Option<Reads> {
    let mut r = Reads::with(&["sit1"], config).await?;
    with_app(&r, "sit1").await;
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
    r.world
        .inventory(&[
            ("application.yml", "common: base\n"),
            ("core.yml", "port: 2\n"),
            ("core-sit1.yml", "name: sit1\n"),
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\nAAAA\n"),
        ])
        .await;
    r.world.whole().await;
    Some(r)
}

#[tokio::test]
async fn served_is_what_the_config_server_answered_for_the_request_asked_with_the_engines_role() {
    let Some(r) = served_world(RegistryConfig::default()).await else {
        return;
    };
    let v = viewer();
    let s = lane("sit1");
    r.gateway
        .attach(&s, answer(200, b"port: 2\nname: ${NOT_THERE}\n"));
    // A request with a channel and an application: the answer is for that question.
    let asked = ServeRequest {
        application: AppName::parse("core").unwrap(),
        tenant: tenant("sit1"),
        channel: Some(channel("web")),
        file: nfs("core.yml"),
    };
    let got = r.service.served(&v, &s, &asked).await.unwrap();
    assert_eq!(got.swimlane, s);
    assert_eq!(got.application.as_str(), "core");
    assert_eq!(got.tenant.as_str(), "sit1");
    assert_eq!(got.channel, Some(channel("web")));
    assert_eq!(got.file, nfs("core.yml"));
    assert_eq!(got.text.as_deref(), Some("port: 2\nname: ${NOT_THERE}\n"));
    assert_eq!(got.role, FileRole::PropertySource);
    assert!(!got.content_withheld);
    assert_eq!(got.hash, ports::content_hash(b"port: 2\nname: ${NOT_THERE}\n"));
    assert_eq!(
        got.unresolved_placeholders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["NOT_THERE"]
    );
    assert!(!got.sources.is_empty());
    // One round trip, with exactly this request.
    let sent = r.gateway.received(&s);
    assert_eq!(sent.len(), 1, "one agent round trip");
    assert!(matches!(&sent[0], HubCommand::FetchServed { request, .. } if *request == asked));
    // The REST handler's answer is the same value, tag aside.
    let Tagged::Fresh { value, .. } = r.service.served_tagged(&v, &s, &asked, None).await.unwrap() else {
        panic!("no tag was given");
    };
    assert_eq!(value, got);
    // And on the wire it is the contract's `ServedFile`: its eleven fields, `text` always there.
    let wire = serde_json::to_value(&got).unwrap();
    let mut keys: Vec<&str> = wire.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "application",
            "channel",
            "content_withheld",
            "file",
            "hash",
            "role",
            "sources",
            "swimlane",
            "tenant",
            "text",
            "unresolved_placeholders"
        ]
    );
}

#[tokio::test]
async fn served_says_not_found_for_what_cannot_be_served_and_unavailable_when_nobody_can_say() {
    let Some(r) = served_world(RegistryConfig::default()).await else {
        return;
    };
    let (v, s) = (viewer(), lane("sit1"));
    let ask = |req: ServeRequest| {
        let (service, v, s) = (r.service.clone(), v.clone(), s.clone());
        async move { service.served(&v, &s, &req).await }
    };
    // No agent is connected.
    assert_eq!(
        ask(request("core.yml")).await.unwrap_err(),
        ReadError::Unavailable
    );
    // The config-server has no such file.
    r.gateway
        .attach(&s, answer(404, b"no such file at /srv/config/secret-path"));
    assert_eq!(ask(request("core.yml")).await.unwrap_err(), ReadError::NotFound);
    // The config-server is unwell: nothing of what it said is in the error.
    r.gateway.attach(&s, answer(500, b"internal detail"));
    assert_eq!(
        ask(request("core.yml")).await.unwrap_err(),
        ReadError::Unavailable
    );
    // A tenant the swimlane does not deploy, and a swimlane that is not there: the agent is not asked.
    r.gateway.attach(&s, answer(200, b"port: 2\n"));
    let before = r.gateway.received(&s).len();
    let other = ServeRequest {
        tenant: tenant("sit9"),
        ..request("core.yml")
    };
    assert_eq!(ask(other).await.unwrap_err(), ReadError::NotFound);
    assert_eq!(
        r.service
            .served(&v, &lane("nowhere"), &request("core.yml"))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(r.gateway.received(&s).len(), before, "the agent was not asked");
    // Bytes that are not text are not made into text, and are not "withheld" either: only a denied file is.
    r.gateway.attach(&s, answer(200, &[0xFF, 0xFE, 0x41]));
    let binary = ask(request("core.yml")).await.unwrap();
    assert!(binary.text.is_none() && !binary.content_withheld);
}

#[tokio::test]
async fn served_withholds_a_denied_file_and_never_asks_the_agent_for_it() {
    let Some(r) = served_world(RegistryConfig::default()).await else {
        return;
    };
    let (v, s) = (viewer(), lane("sit1"));
    r.gateway
        .attach(&s, answer(200, b"-----BEGIN PRIVATE KEY-----\nAAAA\n"));
    let got = r
        .service
        .served(&v, &s, &request("keys/server.pem"))
        .await
        .unwrap();
    assert!(got.content_withheld, "D79");
    assert!(got.text.is_none() && got.sources.is_empty() && got.unresolved_placeholders.is_empty());
    // It reveals at most the file's name, its size and its hash: the hash is that of the stored file.
    assert_eq!(got.file, nfs("keys/server.pem"));
    assert_eq!(
        got.hash,
        ports::content_hash(b"-----BEGIN PRIVATE KEY-----\nAAAA\n")
    );
    assert!(
        r.gateway.received(&s).is_empty(),
        "nothing of a denied file is fetched"
    );
    let shown = format!("{got:?}");
    assert!(!shown.contains("PRIVATE"), "{shown}");
    // The hub's own globs deny it too, before any agent has said so: another service over the same rows, with one more glob.
    let globbed = r.service_with(RegistryConfig {
        extra_deny_globs: vec!["core.yml".to_owned()],
        ..RegistryConfig::default()
    });
    let got = globbed.served(&v, &s, &request("core.yml")).await.unwrap();
    assert!(got.content_withheld && got.text.is_none());
    assert!(r.gateway.received(&s).is_empty());
    // A control: without the glob it is an ordinary file (the agent is asked, once).
    r.gateway.attach(&s, answer(200, b"port: 2\n"));
    let plain = r.service.served(&v, &s, &request("core.yml")).await.unwrap();
    assert!(!plain.content_withheld && plain.text.is_some());
    assert_eq!(r.gateway.received(&s).len(), 1);
}

#[tokio::test]
async fn served_is_limited_to_the_concurrency_and_the_timeout_of_the_rest_route() {
    let config = RegistryConfig {
        served_timeout: Duration::from_millis(300),
        served_concurrency: 2,
        ..RegistryConfig::default()
    };
    let Some(r) = served_world(config).await else {
        return;
    };
    let (v, s) = (viewer(), lane("sit1"));
    // The agent never answers: each call is given the timeout and then is unavailable.
    r.gateway.attach(&s, Arc::new(|_: &HubCommand| None));
    let started = std::time::Instant::now();
    assert_eq!(
        r.service.served(&v, &s, &request("core.yml")).await.unwrap_err(),
        ReadError::Unavailable
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(3),
        "{took:?}"
    );
    // Four at once against a limit of two: two reach the agent, and two are refused before it is asked.
    let before = r.gateway.received(&s).len();
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let (service, v, s) = (r.service.clone(), v.clone(), s.clone());
        tasks.push(tokio::spawn(async move {
            service.served(&v, &s, &request("core.yml")).await
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().unwrap_err(), ReadError::Unavailable);
    }
    assert_eq!(
        r.gateway.received(&s).len() - before,
        2,
        "no more round trips than the limit"
    );
    // The slots are given back: an agent that answers is asked again.
    r.gateway.attach(&s, answer(200, b"port: 2\n"));
    assert!(r.service.served(&v, &s, &request("core.yml")).await.is_ok());
}

// ------------------------------------------------------------------------------------------------ the REST routes beside the port

#[tokio::test]
async fn the_rest_drift_route_keeps_its_severity_order_and_its_own_cursors_beside_the_port() {
    let Some(r) = drift_world().await else { return };
    let h = support::http::Http::over(r);
    let listed = |reply: &support::http::Reply| -> Vec<String> {
        reply.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect()
    };
    // Most serious first, then in the order the rows were made (D103): not the port's order.
    let all = h.view("/api/v1/swimlanes/sit1/drift?limit=100").await;
    assert_eq!(all.status, 200, "{}", all.text());
    let ranked = [
        "b/three.yml",
        "a/deep/five.yml",
        "ab/four.yml",
        "a/two.yml",
        "weird_%/x.yml",
        "0-late.yml",
    ];
    assert_eq!(listed(&all), ranked);
    let port: Vec<String> = h
        .reads
        .service
        .drift(&viewer(), &lane("sit1"), &drift_query(None, None), page(100))
        .await
        .unwrap()
        .items
        .iter()
        .map(|e| e.path.to_string())
        .collect();
    assert_ne!(
        port, ranked,
        "the two lists have two orders, which is the point of keeping both"
    );
    // The REST cursor pages the REST order.
    let first = h.view("/api/v1/swimlanes/sit1/drift?limit=4").await;
    let cursor = first.json()["next"].as_str().unwrap().to_owned();
    let second = h
        .view(&format!("/api/v1/swimlanes/sit1/drift?limit=4&cursor={cursor}"))
        .await;
    let mut both = listed(&first);
    both.extend(listed(&second));
    assert_eq!(both, ranked);
    // The port's cursor is not the REST list's: it is refused as a 400 problem, not followed.
    let by_path = h
        .reads
        .service
        .drift(&viewer(), &lane("sit1"), &drift_query(None, None), page(2))
        .await
        .unwrap();
    let foreign = by_path.next.unwrap();
    let refused = h
        .view(&format!(
            "/api/v1/swimlanes/sit1/drift?limit=2&cursor={}",
            foreign.as_str()
        ))
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text());
}

#[tokio::test]
async fn the_rest_served_route_answers_with_what_the_port_answers_and_keeps_its_tag() {
    let Some(r) = served_world(RegistryConfig::default()).await else {
        return;
    };
    r.gateway
        .attach(&lane("sit1"), answer(200, b"port: 2\nname: ${NOT_THERE}\n"));
    let h = support::http::Http::over(r);
    let uri = "/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file=core.yml";
    let reply = h.view(uri).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    // The body is the port's `ServedFile`, field for field, `text` included.
    let through_the_port = h
        .reads
        .service
        .served(&viewer(), &lane("sit1"), &request("core.yml"))
        .await
        .unwrap();
    assert_eq!(reply.json(), serde_json::to_value(&through_the_port).unwrap());
    assert_eq!(reply.json()["content_withheld"], false);
    assert_eq!(reply.json()["unresolved_placeholders"][0], "NOT_THERE");
    // The tag and the 304 are the route's own and have not changed.
    let tag = reply.header("etag").unwrap().to_owned();
    assert_eq!(reply.header("cache-control"), Some("private, no-cache"));
    let again = h.get_with(Some("viewer"), uri, &[("if-none-match", &tag)]).await;
    assert_eq!(again.status, 304);
    assert!(again.body.is_empty());
    assert_eq!(again.header("etag"), Some(tag.as_str()));
    // A refusal is the problem body of the route, never a cause.
    h.reads
        .gateway
        .attach(&lane("sit1"), answer(500, b"internal detail"));
    let broken = h.view(uri).await;
    assert_eq!(broken.status, 503);
    assert!(!broken.text().contains("internal detail"));
}
