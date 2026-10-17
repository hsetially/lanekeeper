//! Effective config and the grid (T6, D82, A28, A30, A31, P7): what the engine says a service reads, from setting rows that are read
//! once per blob, for one swimlane and for forty. Over a real Postgres 16.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic,
    clippy::many_single_char_names
)]

mod support;

use domain::{ChannelName, FileClass, FileRole, GridQuery, Page, TenantId};
use engine::normalised_hash;
use hub_registry::{RegistryApi, Tagged};
use ports::{ReadError, RegistryRead, content_hash};
use support::reads::{Reads, lane, logical, page};

/// An application `core` whose Deployments say so, so that `core.yml` and `core-<tenant>.yml` are property sources.
async fn with_app(r: &Reads, swimlane: &str) {
    r.sql(&format!(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         VALUES ('{swimlane}', 'ns', 'core', '{{\"SPRING_APPLICATION_NAME\": \"core\"}}')"
    ))
    .await;
}

fn tenant(t: &str) -> TenantId {
    TenantId::parse(t).unwrap()
}

async fn fresh(
    r: &Reads,
    s: &str,
    f: &str,
    t: Option<&str>,
    channel: Option<&str>,
) -> Tagged<domain::EffectiveConfig> {
    let t = t.map(tenant);
    let channel = channel.map(|c| ChannelName::parse(c).unwrap());
    r.service
        .effective_tagged(
            &Reads::viewer(),
            &lane(s),
            &logical(f),
            t.as_ref(),
            channel.as_ref(),
            None,
        )
        .await
        .unwrap()
}

fn value(t: Tagged<domain::EffectiveConfig>) -> domain::EffectiveConfig {
    match t {
        Tagged::Fresh { value, .. } => value,
        Tagged::NotModified { .. } => panic!("no tag was given"),
    }
}

// ------------------------------------------------------------------------------------------------ effective: a property source

#[tokio::test]
async fn a_property_source_is_the_layers_merged_with_the_tenant_on_top() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    // The tenant's branch exists in Git, so the pipeline reads `-sit1` as a tenant suffix (it classifies against the deployable branches).
    r.world
        .tenant_commit("sit1", &[("data/config/core.yml", Some("name: from git\n"))]);
    r.world
        .inventory(&[
            ("application.yml", "common: base\nport: 1\n"),
            ("core.yml", "port: 2\nname: core\n"),
            ("core-sit1.yml", "name: sit1-core\n"),
        ])
        .await;
    let e = value(fresh(&r, "sit1", "core.yml", Some("sit1"), None).await);
    assert_eq!(e.role, FileRole::PropertySource);
    assert_eq!(e.tenant.as_str(), "sit1");
    let settings: Vec<(String, String, String)> = e
        .settings
        .iter()
        .map(|s| (s.path.to_string(), s.value.clone(), s.source.to_string()))
        .collect();
    assert_eq!(
        settings,
        [
            (
                "common".to_owned(),
                "base".to_owned(),
                "application.yml".to_owned()
            ),
            (
                "name".to_owned(),
                "sit1-core".to_owned(),
                "core-sit1.yml".to_owned()
            ),
            ("port".to_owned(), "2".to_owned(), "core.yml".to_owned()),
        ],
        "the tenant's name, the base file's port, the root file's common"
    );
    let ranks: Vec<(String, u32)> = e.sources.iter().map(|s| (s.path.to_string(), s.rank)).collect();
    assert_eq!(
        ranks,
        [
            ("core-sit1.yml".to_owned(), 1),
            ("core.yml".to_owned(), 2),
            ("application.yml".to_owned(), 3)
        ],
        "highest precedence first"
    );
    assert!(
        e.settings.iter().all(|s| s.location.is_some()),
        "every setting has its lines"
    );
    assert!(e.unresolved_placeholders.is_empty());

    // The pipeline computes the same effective hash from the real bytes: the read agrees with the engine's lane computation.
    r.world.whole().await;
    let stored: Vec<u8> = sqlx::query_scalar(
        "SELECT hash::bytea FROM effective_files WHERE swimlane_id = 'sit1' AND logical_file = 'core.yml'",
    )
    .fetch_one(&r.world.rig.pool)
    .await
    .unwrap();
    assert_eq!(
        e.hash.as_bytes().as_slice(),
        stored.as_slice(),
        "the read side and the pipeline agree"
    );
}

#[tokio::test]
async fn the_standing_tenant_is_the_first_in_sorted_order_when_none_is_asked_for() {
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
    let v = Reads::viewer();
    let via_port = r
        .service
        .effective(&v, &lane("sit1"), &logical("core.yml"))
        .await
        .unwrap();
    assert_eq!(via_port.tenant.as_str(), "sit1", "A30");
    assert_eq!(via_port.settings[0].value, "one");
    let two = value(fresh(&r, "sit1", "core.yml", Some("sit2"), None).await);
    assert_eq!(two.settings[0].value, "two", "REST names the tenant");
}

#[tokio::test]
async fn a_tag_makes_the_second_request_not_modified_and_a_change_makes_a_new_tag() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world.inventory(&[("core.yml", "a: 1\n")]).await;
    let v = Reads::viewer();
    let (f, t) = (logical("core.yml"), tenant("sit1"));
    let Tagged::Fresh { etag, .. } = r
        .service
        .effective_tagged(&v, &lane("sit1"), &f, Some(&t), None, None)
        .await
        .unwrap()
    else {
        panic!("fresh")
    };
    assert_eq!(
        r.service
            .effective_tagged(&v, &lane("sit1"), &f, Some(&t), None, Some(&etag))
            .await
            .unwrap(),
        Tagged::NotModified { etag: etag.clone() }
    );
    r.world.nfs_change(&[("core.yml", "a: 2\n")]).await;
    let Tagged::Fresh { etag: changed, .. } = r
        .service
        .effective_tagged(&v, &lane("sit1"), &f, Some(&t), None, Some(&etag))
        .await
        .unwrap()
    else {
        panic!("a changed file is a new tag")
    };
    assert_ne!(changed, etag);
}

#[tokio::test]
async fn an_unknown_tenant_file_or_swimlane_is_not_found_and_a_denied_file_has_no_effective_config() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world
        .inventory(&[
            ("core.yml", "a: 1\n"),
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----"),
        ])
        .await;
    let v = Reads::viewer();
    let ask = |s: &str, f: &str, t: Option<&str>| {
        let (service, v, s, f, t) = (r.service.clone(), v.clone(), lane(s), logical(f), t.map(tenant));
        async move { service.effective_tagged(&v, &s, &f, t.as_ref(), None, None).await }
    };
    assert_eq!(
        ask("sit1", "core.yml", Some("sit9")).await.unwrap_err(),
        ReadError::NotFound,
        "a tenant it does not deploy"
    );
    assert_eq!(
        ask("sit1", "missing.yml", Some("sit1")).await.unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        ask("nowhere", "core.yml", Some("sit1")).await.unwrap_err(),
        ReadError::NotFound
    );
    assert_eq!(
        ask("sit1", "keys/server.pem", Some("sit1")).await.unwrap_err(),
        ReadError::NotFound,
        "D79"
    );
}

#[tokio::test]
async fn a_flagged_value_is_masked_in_the_effective_settings() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    r.world
        .inventory(&[("core.yml", &format!("visible: fine\ntoken: {secret}\n"))])
        .await;
    for indexed in [false, true] {
        if indexed {
            r.index_all().await;
            r.service.clear_caches();
        }
        let e = value(fresh(&r, "sit1", "core.yml", Some("sit1"), None).await);
        let token = e.settings.iter().find(|s| s.path.as_str() == "token").unwrap();
        assert!(
            token.value.starts_with("[REDACTED: ") && !token.value.contains(secret),
            "indexed={indexed}: {}",
            token.value
        );
        assert_eq!(
            e.settings
                .iter()
                .find(|s| s.path.as_str() == "visible")
                .unwrap()
                .value,
            "fine"
        );
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            !json.contains(secret),
            "indexed={indexed}: the response never holds the value"
        );
    }
}

// ------------------------------------------------------------------------------------------------ effective: a resource

#[tokio::test]
async fn a_resource_is_the_chosen_file_rendered_where_the_property_view_has_the_key() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world
        .inventory(&[
            ("core/core.yml", "greeting: hello\n"),
            ("core/msg.txt", "say ${greeting} and ${other}\r\n"),
            ("core/msg-sit1.txt", "tenant says ${greeting}\r\n"),
            ("core/chan/msg.txt", "channel says ${greeting}\n"),
        ])
        .await;
    let rendered = |text: &str| normalised_hash(&bytes::Bytes::from(text.to_owned()), FileClass::Text);

    let base_tenant = value(fresh(&r, "sit1", "core/msg.txt", Some("sit1"), None).await);
    assert_eq!(base_tenant.role, FileRole::Resource);
    assert!(base_tenant.settings.is_empty(), "no settings for a resource");
    assert_eq!(
        base_tenant.sources[0].path.as_str(),
        "core/msg-sit1.txt",
        "the tenant's copy, whole"
    );
    assert_eq!(base_tenant.hash, rendered("tenant says hello\r\n"));
    assert!(base_tenant.unresolved_placeholders.is_empty());

    let channel = value(fresh(&r, "sit1", "core/msg.txt", Some("sit1"), Some("chan")).await);
    assert_eq!(
        channel.sources[0].path.as_str(),
        "core/chan/msg.txt",
        "the channel folder's copy first"
    );
    assert_eq!(channel.hash, rendered("channel says hello\n"));
    assert_eq!(content_hash(b"x"), content_hash(b"x"));
}

#[tokio::test]
async fn a_placeholder_the_view_does_not_have_stays_as_written_and_is_listed() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    r.world
        .inventory(&[
            ("core/core.yml", "greeting: hello\n"),
            ("core/msg.txt", "say ${greeting} and ${other}\n"),
        ])
        .await;
    let e = value(fresh(&r, "sit1", "core/msg.txt", Some("sit1"), None).await);
    assert_eq!(
        e.unresolved_placeholders
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["other"],
        "the service fills it from its own environment; the hub never does"
    );
    assert_eq!(
        e.hash,
        normalised_hash(&bytes::Bytes::from("say hello and ${other}\n"), FileClass::Text)
    );
}

// ------------------------------------------------------------------------------------------------ the grid

/// Three swimlanes with the same file and different values, and one that lacks it.
async fn grid_world() -> Option<Reads> {
    let mut r = Reads::new(&["sit1"]).await?;
    for (lane_id, tenant_id) in [("sit2", "sit2"), ("sit3", "sit3"), ("sit4", "sit4")] {
        r.world.rig.add_swimlane(lane_id, &[tenant_id], false).await;
    }
    for lane_id in ["sit1", "sit2", "sit3"] {
        with_app(&r, lane_id).await;
    }
    r.world
        .inventory(&[("core.yml", "common: same\nport: 1\nonly1: x\n")])
        .await;
    r.world
        .second_lane("sit2", &["sit2"], &[("core.yml", "common: same\nport: 1\n")])
        .await;
    r.world
        .second_lane(
            "sit3",
            &["sit3"],
            &[
                ("core.yml", "common: same\nport: 2\n"),
                ("core-sit3.yml", "port: 3\n"),
            ],
        )
        .await;
    r.world
        .second_lane("sit4", &["sit4"], &[("other.yml", "a: 1\n")])
        .await;
    Some(r)
}

fn grid_query(lanes: &[&str], only_differing: bool, limit: u32) -> GridQuery {
    GridQuery {
        file: logical("core.yml"),
        swimlanes: lanes.iter().map(|l| lane(l)).collect(),
        channel: None,
        only_differing,
        page: page(limit),
    }
}

#[tokio::test]
async fn the_grid_has_a_row_for_each_setting_and_a_cell_for_each_swimlane_in_the_order_asked() {
    let Some(r) = grid_world().await else { return };
    let v = Reads::viewer();
    let named = r
        .service
        .grid_named(&v, grid_query(&["sit3", "sit1", "sit4", "sit2"], false, 100))
        .await
        .unwrap();
    let grid = named.value;
    assert_eq!(
        grid.swimlanes.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["sit3", "sit1", "sit4", "sit2"],
        "the columns are in the order asked for"
    );
    let rows: Vec<&str> = grid.rows.items.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        rows,
        ["common", "only1", "port"],
        "every setting any swimlane has, by path"
    );
    let cell = |row: &str, col: usize| {
        grid.rows
            .items
            .iter()
            .find(|r| r.path.as_str() == row)
            .unwrap()
            .cells[col]
            .value
            .as_ref()
            .map(|v| v.text.clone())
    };
    assert_eq!(
        cell("port", 0).as_deref(),
        Some("3"),
        "sit3 reads its own tenant file"
    );
    assert_eq!(cell("port", 1).as_deref(), Some("1"));
    assert_eq!(cell("port", 2), None, "sit4 has no core.yml: absent");
    assert_eq!(cell("only1", 1).as_deref(), Some("x"));
    assert_eq!(
        cell("only1", 3),
        None,
        "a swimlane that lacks the setting is absent for it"
    );
    assert!(
        grid.rows.items[2].cells[0].location.is_some() && grid.rows.items[2].cells[2].location.is_none(),
        "a cell that has a value has its lines"
    );
    assert_eq!(
        named
            .tenants
            .iter()
            .map(|(s, t)| format!("{s}={t}"))
            .collect::<Vec<_>>(),
        ["sit3=sit3", "sit1=sit1", "sit4=sit4", "sit2=sit2"],
        "A30: the response names the tenant that stands for each swimlane"
    );
}

#[tokio::test]
async fn only_differing_hides_the_rows_where_every_swimlane_agrees() {
    let Some(r) = grid_world().await else { return };
    let v = Reads::viewer();
    let g = r
        .service
        .grid(&v, grid_query(&["sit1", "sit2", "sit3"], true, 100))
        .await
        .unwrap();
    let rows: Vec<&str> = g.rows.items.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(rows, ["only1", "port"], "`common` is the same everywhere");
    // All three the same: nothing differs.
    let same = r
        .service
        .grid(&v, grid_query(&["sit1", "sit2"], true, 100))
        .await
        .unwrap();
    let rows: Vec<&str> = same.rows.items.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(rows, ["only1"], "sit2 lacks `only1`");
}

#[tokio::test]
async fn the_grid_pages_by_setting_path_and_its_cursor_belongs_to_its_columns() {
    let Some(r) = grid_world().await else { return };
    let v = Reads::viewer();
    let q = |limit, cursor: Option<domain::Cursor>| GridQuery {
        page: Page::new(limit, cursor).unwrap(),
        ..grid_query(&["sit1", "sit2", "sit3"], false, 1)
    };
    let mut seen = Vec::new();
    let mut next = None;
    loop {
        let g = r.service.grid(&v, q(1, next.clone())).await.unwrap();
        assert_eq!(g.rows.items.len(), 1);
        seen.push(g.rows.items[0].path.to_string());
        match g.rows.next {
            Some(c) => next = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, ["common", "only1", "port"]);

    let first = r.service.grid(&v, q(1, None)).await.unwrap().rows.next.unwrap();
    let other_columns = GridQuery {
        page: Page::new(1, Some(first.clone())).unwrap(),
        ..grid_query(&["sit1", "sit3"], false, 1)
    };
    assert_eq!(
        r.service.grid(&v, other_columns).await.unwrap_err(),
        ReadError::BadRequest
    );
    let other_file = GridQuery {
        file: logical("other.yml"),
        page: Page::new(1, Some(first)).unwrap(),
        ..grid_query(&["sit1", "sit2", "sit3"], false, 1)
    };
    assert_eq!(
        r.service.grid(&v, other_file).await.unwrap_err(),
        ReadError::BadRequest
    );
}

#[tokio::test]
async fn the_grid_refuses_no_swimlane_too_many_and_an_unknown_one() {
    let Some(r) = grid_world().await else { return };
    let v = Reads::viewer();
    assert_eq!(
        r.service.grid(&v, grid_query(&[], false, 10)).await.unwrap_err(),
        ReadError::BadRequest
    );
    assert_eq!(
        r.service
            .grid(&v, grid_query(&["sit1", "nowhere"], false, 10))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    let many = GridQuery {
        swimlanes: (0..=GridQuery::MAX_SWIMLANES).map(|_| lane("sit1")).collect(),
        ..grid_query(&[], false, 10)
    };
    assert_eq!(r.service.grid(&v, many).await.unwrap_err(), ReadError::BadRequest);
}

#[tokio::test]
async fn a_grid_cell_never_shows_a_flagged_value() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    with_app(&r, "sit1").await;
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    r.world
        .inventory(&[("core.yml", &format!("token: {secret}\nother: 1\n"))])
        .await;
    let v = Reads::viewer();
    let g = r
        .service
        .grid(&v, grid_query(&["sit1"], false, 100))
        .await
        .unwrap();
    let text = serde_json::to_string(&g).unwrap();
    assert!(!text.contains(secret) && text.contains("[REDACTED: "), "{text}");
}
