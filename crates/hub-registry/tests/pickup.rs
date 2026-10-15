//! Pickup state (T10, D85, A2, A11, A20, A37; S4, S10, rule 8, rule 10): when a change takes effect for each service that reads it, as the
//! registry computes it, stores it, announces it and serves it. Over a real Postgres 16, the real ingestion, pipeline and read service.
//!
//! The four states have a test each (`resource_file_is_live_within_ttl_with_a_live_by_time`, `property_source_needs_notify_or_restart`,
//! `new_folder_or_channels_yml_needs_a_config_server_restart`, `a_notified_or_restarted_service_is_live`); the rest pin what the plan's
//! rows ask: the first inventory is not a change (A11), `PickupChanged` is in the outbox in the transaction of the state change (D80), the
//! sweeper turns time into state (A37) and is idempotent, a denied file is never listed, nothing of a value or a path is logged (S10).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::{PickupState, ServiceState, SwimlaneId, Timestamp};
use hub_platform::jobs::JobError;
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::PickupResult;
use hub_registry::{Pickups, RegistryApi};
use ports::{RegistryRead, SinkError};
use support::cluster::{HOUR, MIN, Stored, T0, dep, old_events, pickup_events, report, stored};
use support::log_capture::LogCapture;
use support::reads::{Reads, lane, page};
use support::rig::T;

/// When the first change of a test is detected: the scans of `World` are a second apart from `T`, the first inventory at `T`.
const CHANGE_1: i64 = T + 1_000;
const CHANGE_2: i64 = T + 2_000;

/// A swimlane `sit1` with an inventory of `files`, the read service over it, and the pickup computation of its pipeline.
struct Bed {
    r: Reads,
    pickups: Pickups,
}

impl Bed {
    async fn new(files: &[(&str, &str)]) -> Option<Self> {
        Self::with(files, RegistryConfig::default()).await
    }

    async fn with(files: &[(&str, &str)], config: RegistryConfig) -> Option<Self> {
        let mut r = Reads::with(&["sit1"], config).await?;
        r.world.inventory(files).await;
        let pickups = r.world.pipeline.pickups();
        Some(Self { r, pickups })
    }

    fn lane(&self) -> SwimlaneId {
        self.r.lane()
    }

    /// The Deployments the agent reports (a full report: the list there is), and the config-server's start when it is known.
    async fn cluster(&self, deployments: Vec<domain::DeploymentInfo>, config_server_started: Option<i64>) {
        self.r
            .world
            .rig
            .cluster(report(true, deployments, config_server_started))
            .await
            .unwrap();
    }

    /// Files changed on NFS, then the computation the pipeline's job runs after the batch.
    async fn change(&mut self, files: &[(&str, &str)]) {
        self.r.world.nfs_change(files).await;
        self.recompute().await;
    }

    async fn recompute(&self) {
        let done = self.pickups.recompute(&[self.lane()]).await;
        assert!(done.failed.is_empty(), "{done:?}");
    }

    /// What `GET pending-restarts` answers: the services that have not picked up everything.
    async fn pending(&self) -> Vec<ServiceState> {
        self.r
            .service
            .pending_restarts(&Reads::viewer(), &self.lane())
            .await
            .unwrap()
    }

    async fn pending_of(&self, name: &str) -> Option<ServiceState> {
        self.pending()
            .await
            .into_iter()
            .find(|s| s.service.name() == name)
    }

    async fn stored(&self, name: &str) -> Option<Stored> {
        stored(&self.r.world.rig.pool, name).await
    }

    async fn events(&self) -> Vec<Vec<String>> {
        pickup_events(&self.r.world.rig).await
    }

    fn clock(&self) -> &hub_registry::ManualClock {
        &self.r.world.rig.clock
    }

    async fn sweep(&self) -> hub_registry::PickupSweepReport {
        self.pickups.run_once().await.unwrap()
    }

    async fn sql(&self, statement: &str) {
        self.r.sql(statement).await;
    }
}

/// What prompt 06's notify action writes (and this crate only reads): the time the services were told.
fn notified_at(at_ms: i64) -> String {
    format!("UPDATE swimlanes SET last_notified_at = timestamptz 'epoch' + interval '{at_ms} milliseconds'")
}

fn ms(t: Option<Timestamp>) -> Option<i64> {
    t.map(Timestamp::unix_millis)
}

fn files(s: &ServiceState) -> Vec<String> {
    s.pending_files.iter().map(ToString::to_string).collect()
}

// ------------------------------------------------------------------------------------------------ the four states

#[tokio::test]
async fn resource_file_is_live_within_ttl_with_a_live_by_time() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;

    let pending = b.pending().await;
    assert_eq!(pending.len(), 1, "{pending:?}");
    let api = &pending[0];
    assert_eq!(api.service.name(), "tx-api");
    assert_eq!(api.pickup, PickupState::LiveWithinTtl);
    assert_eq!(
        ms(api.live_by),
        Some(CHANGE_1 + 20 * MIN),
        "live by the detection time plus the client's cache lifetime (20 minutes by default)"
    );
    assert_eq!(ms(api.pending_since), Some(CHANGE_1));
    assert_eq!(files(api), ["tx-api/layout.xsl"]);
    assert_eq!((api.cache_ttl_secs, api.notifications_enabled), (1200, false));
    assert_eq!(
        b.events().await,
        vec![vec!["tx-api".to_owned()]],
        "PickupChanged names the service, and carries nothing else"
    );
    assert_eq!(old_events(&b.r.world.rig).await, 0);
}

#[tokio::test]
async fn a_deployments_own_cache_lifetime_sets_live_by() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![
            dep("tx-api")
                .app("tx-api")
                .ttl(900)
                .pod("tx-api-1", T0 - HOUR)
                .build(),
        ],
        None,
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(ms(api.live_by), Some(CHANGE_1 + 15 * MIN));
    assert_eq!(api.cache_ttl_secs, 900);
}

#[tokio::test]
async fn property_source_needs_notify_or_restart() {
    let Some(mut b) = Bed::new(&[("tx-api/tx-api.yml", "a: 1\n")]).await else {
        return;
    };
    b.cluster(
        vec![
            dep("tx-api")
                .app("tx-api")
                .listens()
                .pod("tx-api-1", T0 - HOUR)
                .build(),
            dep("tx-api-b").app("tx-api").pod("tx-api-b-1", T0 - HOUR).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/tx-api.yml", "a: 2\n")]).await;

    let pending = b.pending().await;
    assert_eq!(
        pending.len(),
        2,
        "both run the application the file is a property source of"
    );
    for s in &pending {
        assert_eq!(
            s.pickup,
            PickupState::NeedsNotifyOrRestart,
            "{}",
            s.service.name()
        );
        assert_eq!(s.live_by, None, "waiting does not help a property source");
        assert_eq!(files(s), ["tx-api/tx-api.yml"]);
    }
    assert_eq!(
        pending
            .iter()
            .map(|s| s.notifications_enabled)
            .collect::<Vec<_>>(),
        [true, false],
        "the client that listens is told apart from the one that does not"
    );
    // An hour later nothing has changed: time is not what a property source waits for.
    b.clock().advance_secs(3 * 3600);
    let sweep = b.sweep().await;
    assert_eq!(sweep.changed, 0);
    assert_eq!(b.pending().await.len(), 2);
}

#[tokio::test]
async fn new_folder_or_channels_yml_needs_a_config_server_restart() {
    let Some(mut b) = Bed::new(&[("channels.yml", "- web\n"), ("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![
            dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build(),
            dep("newapp").app("newapp").pod("newapp-1", T0 - HOUR).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    // The list of channels is the config-server's own and read once, when it starts: every service waits for it.
    b.change(&[("channels.yml", "- web\n- mobile\n")]).await;
    let pending = b.pending().await;
    assert_eq!(pending.len(), 2);
    assert!(
        pending
            .iter()
            .all(|s| s.pickup == PickupState::NeedsConfigServerRestart)
    );
    assert!(pending.iter().all(|s| files(s) == ["channels.yml"]));
    // A folder that did not exist when the config-server started is not one of its search locations.
    b.change(&[("newapp/new.xsl", "<n/>")]).await;
    let newapp = b.pending_of("newapp").await.unwrap();
    assert_eq!(newapp.pickup, PickupState::NeedsConfigServerRestart);
    assert_eq!(files(&newapp), ["channels.yml", "newapp/new.xsl"]);
    assert_eq!(
        ms(newapp.pending_since),
        Some(CHANGE_1),
        "the oldest change that is not live"
    );
    assert_eq!(
        files(&b.pending_of("tx-api").await.unwrap()),
        ["channels.yml"],
        "the new folder is not read by the other application"
    );

    // Neither a restart of the service nor a notification helps: only the config-server's restart does.
    b.cluster(
        vec![
            dep("tx-api")
                .app("tx-api")
                .listens()
                .pod("tx-api-2", T0 - 10_000)
                .build(),
            dep("newapp").app("newapp").pod("newapp-1", T0 - HOUR).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.sql(&notified_at(CHANGE_2 + 60_000)).await;
    b.sweep().await;
    assert_eq!(b.pending().await.len(), 2, "still waiting for the config-server");
    // It restarts, after both changes. The channels are live for everybody. The file of the new folder is no longer one that the config-server
    // cannot see: from now on it is the change to a resource that it is, which goes live with the client's cache. Both services are named,
    // in the transaction of the report.
    let before = b.events().await.len();
    b.cluster(
        vec![
            dep("tx-api").app("tx-api").pod("tx-api-2", T0 - 10_000).build(),
            dep("newapp").app("newapp").pod("newapp-1", T0 - HOUR).build(),
        ],
        Some(T0 - 1_000),
    )
    .await;
    let left = b.pending().await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].service.name(), "newapp");
    assert_eq!(left[0].pickup, PickupState::LiveWithinTtl);
    assert_eq!(ms(left[0].live_by), Some(CHANGE_2 + 20 * MIN));
    assert_eq!(files(&left[0]), ["newapp/new.xsl"]);
    let events = b.events().await;
    assert_eq!(
        events.len(),
        before + 1,
        "one PickupChanged, in the transaction of the report"
    );
    let mut named = events.last().unwrap().clone();
    named.sort();
    assert_eq!(named, ["newapp", "tx-api"]);
}

#[tokio::test]
async fn a_notified_or_restarted_service_is_live() {
    // `application.yml` at the root is a property source of every application, so every service reads it.
    let Some(mut b) = Bed::new(&[("application.yml", "a: 1\n")]).await else {
        return;
    };
    b.cluster(
        vec![
            dep("listening")
                .app("one")
                .listens()
                .pod("l-1", T0 - HOUR)
                .build(),
            dep("deaf").app("two").pod("d-1", T0 - HOUR).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("application.yml", "a: 2\n")]).await;
    assert_eq!(b.pending().await.len(), 2);

    // Told before the change (prompt 06 wrote the time of an earlier notification): it does not count.
    b.sql(&notified_at(CHANGE_1 - 60_000)).await;
    assert_eq!(b.sweep().await.changed, 0);
    assert_eq!(b.pending().await.len(), 2);

    // Told after it: a client that listens refreshes, one that does not, does not. Read, never written, by this crate.
    b.sql(&notified_at(CHANGE_1 + 60_000)).await;
    let before = b.events().await.len();
    let sweep = b.sweep().await;
    assert_eq!(sweep.changed, 1);
    assert_eq!(b.events().await[before..], [vec!["listening".to_owned()]]);
    let pending = b.pending().await;
    assert_eq!(
        pending.iter().map(|s| s.service.name()).collect::<Vec<_>>(),
        ["deaf"]
    );
    assert_eq!(pending[0].pickup, PickupState::NeedsNotifyOrRestart);
    assert_eq!(
        b.r.world
            .rig
            .count("SELECT count(*) FROM swimlanes WHERE last_notified_at IS NOT NULL")
            .await,
        1,
        "the notification time is whatever 06 wrote"
    );

    // A pod that starts after the change has what the config-server has now.
    b.cluster(
        vec![
            dep("listening")
                .app("one")
                .listens()
                .pod("l-1", T0 - HOUR)
                .build(),
            dep("deaf").app("two").pod("d-2", T0 - 10_000).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    assert!(b.pending().await.is_empty());
    assert_eq!(b.events().await.last().unwrap(), &["deaf".to_owned()]);
    let stored = b.stored("deaf").await.unwrap();
    assert_eq!(
        stored,
        Stored {
            state: "live".to_owned(),
            live_by: None,
            pending_since: None,
            files: Vec::new()
        }
    );
}

// ------------------------------------------------------------------------------------------------ what is a change

#[tokio::test]
async fn the_first_inventory_is_not_a_change() {
    // A11: auditing 2,000 files and marking every service pending would be false alarms. The pods are older than the whole inventory.
    let Some(b) = Bed::new(&[
        ("application.yml", "a: 1\n"),
        ("tx-api/tx-api.yml", "a: 1\n"),
        ("tx-api/layout.xsl", "<a/>"),
        ("channels.yml", "- web\n"),
    ])
    .await
    else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T - HOUR).build()],
        Some(T - 2 * HOUR),
    )
    .await;
    b.recompute().await;
    assert!(b.pending().await.is_empty());
    assert!(b.events().await.is_empty());
    assert_eq!(
        b.r.world
            .rig
            .count("SELECT count(*) FROM file_observations WHERE initial")
            .await,
        4,
        "T1 marked all four versions of the first inventory"
    );
}

#[tokio::test]
async fn a_service_with_no_pod_has_nothing_to_pick_up() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(vec![dep("tx-api").app("tx-api").build()], Some(T0 - 2 * HOUR))
        .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    assert!(
        b.pending().await.is_empty(),
        "scaled to zero: it asks for everything when a pod starts"
    );
}

#[tokio::test]
async fn a_config_server_start_nobody_reported_leaves_channels_and_new_folders_as_ordinary_changes() {
    // Nothing is known of when the config-server started, so nothing can be said about a restart: the two files that would need one are the
    // resources they look like, and go live with the cache, instead of waiting for ever for a restart that nobody can see.
    let Some(mut b) = Bed::new(&[("channels.yml", "- web\n"), ("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        None,
    )
    .await;
    b.change(&[
        ("channels.yml", "- web\n- mobile\n"),
        ("tx-api/new/x.xsl", "<n/>"),
    ])
    .await;
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(api.pickup, PickupState::LiveWithinTtl);
    assert_eq!(ms(api.live_by), Some(CHANGE_1 + 20 * MIN));
    assert_eq!(files(&api), ["channels.yml", "tx-api/new/x.xsl"]);
}

#[tokio::test]
async fn a_report_that_changes_nothing_computes_nothing() {
    let Some(b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    let metrics = &b.r.world.rig.metrics;
    let report = || vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()];
    b.cluster(report(), Some(T0 - 2 * HOUR)).await;
    let first =
        metrics.pickup_lane_count(PickupResult::Unchanged) + metrics.pickup_lane_count(PickupResult::Changed);
    assert_eq!(first, 1, "the Deployment is new: the state is computed");
    for _ in 0..3 {
        b.cluster(report(), Some(T0 - 2 * HOUR)).await;
    }
    let after =
        metrics.pickup_lane_count(PickupResult::Unchanged) + metrics.pickup_lane_count(PickupResult::Changed);
    assert_eq!(
        after, first,
        "the same report, again and again: nothing to compute"
    );
    // A changed pod is a change.
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-2", T0 - 1_000).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    let restarted =
        metrics.pickup_lane_count(PickupResult::Unchanged) + metrics.pickup_lane_count(PickupResult::Changed);
    assert_eq!(restarted, first + 1);
}

#[tokio::test]
async fn a_denied_file_counts_toward_the_state_and_is_never_listed() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    // A keystore the agent flagged as denied: a name, a size and a hash, no bytes.
    b.r.world
        .nfs_raw(|at| {
            let mut e = support::rig::entry("tx-api/server.p12", b"keystore", at);
            e.denied = true;
            e.bytes = None;
            vec![e]
        })
        .await;
    b.recompute().await;
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(
        api.pickup,
        PickupState::LiveWithinTtl,
        "the service is as stale as for any other file"
    );
    assert!(api.pending_files.is_empty(), "and nothing names the file");
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(files(&api), ["tx-api/layout.xsl"]);
    let stored = b.stored("tx-api").await.unwrap();
    assert!(!stored.files.iter().any(|f| f.contains("p12")), "{stored:?}");

    // The hub's own list is applied again where the list is read: a row written before the globs were what they are.
    b.sql(
        "UPDATE service_state SET pending_files = '[\"tx-api/layout.xsl\", \"tx-api/leaked.pem\"]'::jsonb \
         WHERE service_id IN (SELECT id FROM services WHERE name = 'tx-api')",
    )
    .await;
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(
        files(&api),
        ["tx-api/layout.xsl"],
        "a path the hub's globs deny is not served"
    );
}

#[tokio::test]
async fn a_change_is_read_by_the_services_the_rules_name_and_by_nobody_else() {
    let Some(mut b) = Bed::new(&[
        ("tx-api/layout.xsl", "<a/>"),
        ("portal/layout.xsl", "<a/>"),
        ("reports/summary.xsl", "<a/>"),
        ("reports2/summary.xsl", "<a/>"),
        ("nobody/asks.txt", "x"),
    ])
    .await
    else {
        return;
    };
    b.cluster(
        vec![
            dep("api").app("tx-api").pod("api-1", T0 - HOUR).build(),
            dep("web").app("portal").pod("web-1", T0 - HOUR).build(),
            dep("batch").pod("batch-1", T0 - HOUR).build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    // `reports` is linked to the Deployment `batch` by a person; `reports2` only by a suggestion that nobody confirmed.
    b.sql(
        "INSERT INTO folder_mappings (folder_path, namespace, deployment, confirmed) VALUES \
           ('reports', 'csp', 'batch', true), ('reports2', 'csp', 'batch', false)",
    )
    .await;
    b.change(&[
        ("tx-api/layout.xsl", "<b/>"),
        ("portal/layout.xsl", "<b/>"),
        ("reports/summary.xsl", "<b/>"),
        ("reports2/summary.xsl", "<b/>"),
        ("nobody/asks.txt", "y"),
    ])
    .await;
    let pending = b.pending().await;
    let by_name = |name: &str| pending.iter().find(|s| s.service.name() == name).map(files);
    assert_eq!(by_name("api"), Some(vec!["tx-api/layout.xsl".to_owned()]));
    assert_eq!(by_name("web"), Some(vec!["portal/layout.xsl".to_owned()]));
    assert_eq!(
        by_name("batch"),
        Some(vec!["reports/summary.xsl".to_owned()]),
        "the confirmed link, and not the suggestion"
    );
}

#[tokio::test]
async fn a_tenant_file_of_a_tenant_nobody_deploys_changes_nothing_for_anybody() {
    let Some(mut w) = Reads::new(&["sit1"]).await else {
        return;
    };
    // Git has the branches, so that the pipeline reads `core-sit2.yml` as the copy of sit2, a tenant that sit1 does not deploy.
    w.world.base_commit(&[("config/tx-api/core.yml", Some("a: 0\n"))]);
    for branch in ["sit1", "sit2"] {
        w.world
            .tenant_commit(branch, &[("data/config/tx-api/core.yml", Some("t: 0\n"))]);
    }
    w.world
        .inventory(&[("tx-api/core.yml", "a: 0\n"), ("tx-api/core-sit1.yml", "t: 0\n")])
        .await;
    w.world.whole().await;
    let pickups = w.world.pipeline.pickups();
    w.world
        .rig
        .cluster(report(
            true,
            vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
            Some(T0 - 2 * HOUR),
        ))
        .await
        .unwrap();
    // sit2's copy first: the pipeline classifies it (logical file `tx-api/core.yml`, tenant sit2), nothing reads it.
    w.world.nfs_change(&[("tx-api/core-sit2.yml", "t: 2\n")]).await;
    w.world.paths(&["tx-api/core-sit2.yml"]).await;
    assert!(pickups.recompute(&[lane("sit1")]).await.failed.is_empty());
    let none = w
        .service
        .pending_restarts(&Reads::viewer(), &lane("sit1"))
        .await
        .unwrap();
    assert!(none.is_empty(), "{none:?}");
    // sit1's own copy is read.
    w.world.nfs_change(&[("tx-api/core-sit1.yml", "t: 1\n")]).await;
    w.world.paths(&["tx-api/core-sit1.yml"]).await;
    assert!(pickups.recompute(&[lane("sit1")]).await.failed.is_empty());
    let some = w
        .service
        .pending_restarts(&Reads::viewer(), &lane("sit1"))
        .await
        .unwrap();
    assert_eq!(some.len(), 1);
    assert_eq!(files(&some[0]), ["tx-api/core-sit1.yml"]);
}

#[tokio::test]
async fn a_removed_service_is_cleared_and_announced() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    assert_eq!(b.pending().await.len(), 1);
    let before = b.events().await.len();
    // The next full report does not name it any more: the Deployment is gone and so is what it had to pick up.
    b.cluster(
        vec![dep("other").app("other").pod("o-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    assert!(b.pending().await.is_empty());
    assert_eq!(b.events().await[before..], [vec!["tx-api".to_owned()]]);
    assert_eq!(b.stored("tx-api").await.unwrap().state, "live");
}

#[tokio::test]
async fn the_state_is_computed_from_the_newest_changes_and_says_when_it_cut_them() {
    let config = RegistryConfig {
        pickup_max_files: 100,
        ..RegistryConfig::default()
    };
    let initial: Vec<(String, String)> = (0..150)
        .map(|i| (format!("tx-api/f{i:03}.xsl"), "<a/>".to_owned()))
        .collect();
    let initial: Vec<(&str, &str)> = initial.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let Some(mut b) = Bed::with(&initial, config).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    let changed: Vec<(String, String)> = initial
        .iter()
        .map(|(p, _)| ((*p).to_owned(), "<b/>".to_owned()))
        .collect();
    let changed: Vec<(&str, &str)> = changed.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    b.change(&changed).await;
    assert_eq!(
        b.r.world.rig.metrics.pickup_truncated_count(),
        1,
        "150 changes, 100 read"
    );
    let api = b.pending_of("tx-api").await.unwrap();
    assert_eq!(api.pickup, PickupState::LiveWithinTtl);
    assert_eq!(api.pending_files.len(), 100);
}

// ------------------------------------------------------------------------------------------------ the sweeper (A37)

#[tokio::test]
async fn the_sweeper_turns_live_within_ttl_into_live_when_live_by_passes() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    let live_by = ms(b.pending_of("tx-api").await.unwrap().live_by).unwrap();
    let events = b.events().await.len();

    // One millisecond before: nothing to say.
    b.clock().set_ms(live_by - 1);
    let early = b.sweep().await;
    assert_eq!((early.changed, early.failed), (0, 0));
    assert_eq!(
        b.pending_of("tx-api").await.unwrap().pickup,
        PickupState::LiveWithinTtl
    );
    assert_eq!(b.events().await.len(), events);

    // At live_by the cache can no longer hold the old file: live, and the service is named, once.
    b.clock().set_ms(live_by);
    let at = b.sweep().await;
    assert_eq!((at.swimlanes, at.changed, at.failed), (1, 1, 0));
    assert!(b.pending().await.is_empty());
    assert_eq!(b.events().await[events..], [vec!["tx-api".to_owned()]]);
    assert_eq!(
        b.stored("tx-api").await.unwrap(),
        Stored {
            state: "live".to_owned(),
            live_by: None,
            pending_since: None,
            files: Vec::new()
        },
        "a live service holds nothing"
    );
    assert!(
        b.r.world.rig.metrics.pickup_lane_count(PickupResult::Changed) >= 2,
        "the change, and the sweep"
    );
}

#[tokio::test]
async fn the_sweeper_is_idempotent() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    b.clock().advance_secs(2 * 3600);
    let first = b.sweep().await;
    assert_eq!(first.changed, 1);
    let (events, rows, audit) = (
        b.r.world.rig.bus.count().await,
        b.r.world.rig.count("SELECT count(*) FROM service_state").await,
        b.r.world.rig.count("SELECT count(*) FROM audit_events").await,
    );
    for _ in 0..3 {
        let again = b.sweep().await;
        assert_eq!((again.changed, again.failed), (0, 0));
    }
    assert_eq!(b.r.world.rig.bus.count().await, events, "no event");
    assert_eq!(
        b.r.world.rig.count("SELECT count(*) FROM service_state").await,
        rows
    );
    assert_eq!(
        b.r.world.rig.count("SELECT count(*) FROM audit_events").await,
        audit,
        "the pickup state is derived: it writes no audit event (A12)"
    );
    // The same computation again, from the pipeline's side, says the same.
    b.recompute().await;
    assert_eq!(b.r.world.rig.bus.count().await, events);
}

#[tokio::test]
async fn the_sweeper_looks_at_every_swimlane_now_and_then_and_at_the_pending_ones_always() {
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    let full = b.sweep().await;
    assert!(full.full, "the first run of a replica looks at everything");
    let state = b.stored("tx-api").await.unwrap();
    // The state is lost behind the sweeper's back (a computation that failed): the swimlane has nothing pending as far as the table says.
    b.sql("UPDATE service_state SET pickup_state = 'live', live_by = NULL, pending_since = NULL, pending_files = '[]'")
        .await;
    let quick = b.sweep().await;
    assert!(!quick.full, "ten minutes have not passed");
    assert_eq!(
        (quick.swimlanes, quick.changed),
        (0, 0),
        "a swimlane with nothing pending is not looked at"
    );
    b.clock().advance_secs(10 * 60);
    let again = b.sweep().await;
    assert!(again.full);
    assert_eq!(
        (again.swimlanes, again.changed),
        (1, 1),
        "the full look repairs it"
    );
    assert_eq!(b.stored("tx-api").await.unwrap(), state);
    assert_eq!(b.events().await.last().unwrap(), &["tx-api".to_owned()]);
}

#[tokio::test]
async fn the_sweeper_stops_when_the_lease_is_lost() {
    let Some(b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    let ended = b.pickups.sweep(|| Err(JobError::LeaseLost)).await;
    assert!(matches!(ended, Err(JobError::LeaseLost)), "{ended:?}");
}

#[tokio::test]
async fn the_job_has_its_spec_and_runs_a_sweep() {
    let Some(b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    let jobs = b.r.world.pipeline.jobs().unwrap();
    let names: Vec<&str> = jobs.iter().map(|(spec, _)| spec.name()).collect();
    assert_eq!(
        names,
        ["registry-pipeline", "registry-nightly", "registry-pickup-sweeper"]
    );
    assert_eq!(
        Pickups::spec(&RegistryConfig::default()).unwrap().name(),
        hub_registry::pickup::JOB_NAME
    );
    let (ctx, _lease): (hub_platform::jobs::JobContext, _) =
        hub_platform::jobs::JobContext::held_until_told_otherwise(hub_registry::pickup::JOB_NAME);
    jobs.into_iter()
        .find(|(spec, _)| spec.name() == hub_registry::pickup::JOB_NAME)
        .unwrap()
        .1
        .run(&ctx)
        .await
        .unwrap();
}

// ------------------------------------------------------------------------------------------------ where it is computed

#[tokio::test]
async fn the_pipelines_job_computes_the_pickup_state_after_a_batch() {
    use hub_registry::metrics::LaneResult;
    use support::world::{RunningJob, wait_until};
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.r.world
        .base_commit(&[("config/tx-api/layout.xsl", Some("<a/>"))]);
    b.r.world.whole().await;
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    let wholes = b.r.world.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&b.r.world.pipeline);
    wait_until(
        "the takeover to load the swimlane",
        std::time::Duration::from_secs(20),
        || async {
            b.r.world.rig.metrics.pipeline_lane_count(LaneResult::Whole) > wholes
                && !b.r.world.pipeline.lanes_in_memory().await.is_empty()
        },
    )
    .await;
    // A change reaches the bus, the window closes, the batch commits, and the state follows.
    b.r.world.nfs_change(&[("tx-api/layout.xsl", "<b/>")]).await;
    wait_until(
        "the pickup state after a batch",
        std::time::Duration::from_secs(20),
        || async { !b.events().await.is_empty() },
    )
    .await;
    assert_eq!(
        b.pending_of("tx-api").await.unwrap().pickup,
        PickupState::LiveWithinTtl
    );
    assert!(matches!(job.lose_lease().await, Err(JobError::LeaseLost)));
}

#[tokio::test]
async fn pickup_changed_is_written_in_the_transaction_of_the_state_change() {
    // D80, rule 8: the state and its event are one write. A bus that cannot take the event rolls the whole report back, the state included,
    // and the agent's retry writes both.
    use ports::fakes::FakeGit;
    let Some(rig) = support::rig::Rig::new().await else {
        return;
    };
    rig.first_scan(&[("tx-api/layout.xsl", "<a/>")]).await;
    rig.delta(support::rig::delta(
        2,
        Some(support::rig::root(1)),
        support::rig::root(2),
        vec![support::rig::entry("tx-api/layout.xsl", b"<b/>", CHANGE_1)],
    ))
    .await
    .unwrap();
    let pending = report(
        true,
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    );
    let events = rig.bus.count().await;
    rig.bus.set_failing(true);
    assert_eq!(rig.cluster(pending.clone()).await, Err(SinkError::Unavailable));
    assert_eq!(
        rig.count("SELECT count(*) FROM services").await,
        0,
        "the Deployment is rolled back too"
    );
    assert_eq!(rig.count("SELECT count(*) FROM service_state").await, 0);
    assert_eq!(rig.bus.count().await, events);

    rig.bus.set_failing(false);
    rig.cluster(pending).await.unwrap();
    assert_eq!(
        stored(&rig.pool, "tx-api").await.unwrap().state,
        "live_within_ttl"
    );
    assert_eq!(
        pickup_events(&rig).await,
        vec![vec!["tx-api".to_owned()]],
        "exactly one"
    );

    // The same for the sweeper: the state that time changed is not written if its event cannot be.
    let (pipeline, _handle) = rig.pipeline(Arc::new(FakeGit::new()), RegistryConfig::default());
    let pickups = pipeline.pickups();
    rig.clock.advance_secs(3600);
    rig.bus.set_failing(true);
    let failed = pickups.run_once().await.unwrap();
    assert_eq!((failed.failed, failed.changed), (1, 0));
    assert_eq!(
        stored(&rig.pool, "tx-api").await.unwrap().state,
        "live_within_ttl"
    );
    assert_eq!(pickup_events(&rig).await.len(), 1);
    rig.bus.set_failing(false);
    let done = pickups.run_once().await.unwrap();
    assert_eq!((done.failed, done.changed), (0, 1));
    assert_eq!(stored(&rig.pool, "tx-api").await.unwrap().state, "live");
    assert_eq!(pickup_events(&rig).await.len(), 2);
}

#[tokio::test]
async fn pending_restart_changed_is_not_published_any_more() {
    // A2: the old event stays in the contract so that old rows parse, and nothing publishes it. A scan of the source says so; the tests above
    // say what is published instead.
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                seen += 1;
                let text = std::fs::read_to_string(&path).unwrap();
                let production = text
                    .lines()
                    .take_while(|l| l.trim() != "#[cfg(test)]")
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    !production.contains("PendingRestartChanged"),
                    "{} names the superseded event",
                    path.display()
                );
            }
        }
    }
    assert!(seen > 20, "the scan covered the crate ({seen} files)");
}

// ------------------------------------------------------------------------------------------------ security

#[tokio::test]
async fn the_state_is_served_to_an_active_viewer_and_to_nobody_else() {
    // S4: authorization on the port as well as on the route (the route-table and `http_authz` tests hold the route). Over a swimlane that has
    // a real pickup state, so that a refusal is not the answer for an empty list.
    use domain::{Role, UserStatus};
    use ports::conformance::sample::user;
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    b.cluster(
        vec![dep("tx-api").app("tx-api").pod("tx-api-1", T0 - HOUR).build()],
        Some(T0 - 2 * HOUR),
    )
    .await;
    b.change(&[("tx-api/layout.xsl", "<b/>")]).await;
    let service = &b.r.service;
    let s = lane("sit1");
    for role in [Role::Viewer, Role::Editor, Role::Operator, Role::Admin] {
        let u = user(5, Some(role), UserStatus::Active);
        let seen = service.pending_restarts(&u, &s).await.unwrap();
        assert_eq!(seen.len(), 1, "{role:?}");
        assert_eq!(
            service
                .pending_restarts_page(&u, &s, page(10))
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
    for u in [
        user(7, None, UserStatus::Pending),
        user(8, Some(Role::Admin), UserStatus::Pending),
        user(9, Some(Role::Viewer), UserStatus::Disabled),
        user(10, None, UserStatus::Active),
    ] {
        assert_eq!(
            service.pending_restarts(&u, &s).await.unwrap_err(),
            ports::ReadError::Forbidden
        );
        assert_eq!(
            service.pending_restarts_page(&u, &s, page(10)).await.unwrap_err(),
            ports::ReadError::Forbidden
        );
    }
    // And the swimlane's detail, which carries the same states, is as closed.
    let denied = user(7, None, UserStatus::Pending);
    assert!(service.swimlane(&denied, &s).await.is_err());
}

#[tokio::test]
async fn no_value_and_no_denied_path_reaches_a_log_an_event_or_the_stored_state() {
    // S10, S14, rule 10: the three variables the computation reads are read by key, and nothing it logs or publishes holds a value, a path
    // that is denied, or the text of a file. A secret that sits in `env_values` (the allowlist keeps them out, but the column is JSON and an
    // admin may widen the list) is never loaded.
    const SECRET: &str = "hunter2-planted-pickup-secret";
    const CONTENT: &str = "planted-file-content-for-pickup";
    const DENIED: &str = "keystore-planted-name.p12";
    let Some(mut b) = Bed::new(&[("tx-api/layout.xsl", "<a/>")]).await else {
        return;
    };
    let (capture, _guard) = LogCapture::install();
    b.cluster(
        vec![
            dep("tx-api")
                .app("tx-api")
                .listens()
                .ttl(600)
                .env("DB_PASSWORD", SECRET)
                .pod("tx-api-1", T0 - HOUR)
                .build(),
        ],
        Some(T0 - 2 * HOUR),
    )
    .await;
    // The column holds the secret as if the allowlist had let it in. Bound, so that the statement the test itself runs does not carry it
    // into the capture.
    sqlx::query("UPDATE services SET env_values = env_values || jsonb_build_object($1::text, $2::text)")
        .bind("TOKEN")
        .bind(SECRET)
        .execute(&b.r.world.rig.pool)
        .await
        .unwrap();
    b.r.world
        .nfs_raw(|at| {
            let mut e = support::rig::entry(&format!("tx-api/{DENIED}"), CONTENT.as_bytes(), at);
            e.denied = true;
            e.bytes = None;
            vec![e]
        })
        .await;
    b.change(&[("tx-api/layout.xsl", CONTENT)]).await;
    b.clock().advance_secs(3 * 3600);
    b.sweep().await;
    b.sweep().await;
    // The read side, with the state in place and after it was cleared.
    b.pending().await;
    let _ = b.r.service.swimlane(&Reads::viewer(), &b.lane()).await;

    let logs = capture.text();
    assert!(
        !logs.is_empty(),
        "the capture works: the computation says something"
    );
    for planted in [SECRET, CONTENT, DENIED, "DB_PASSWORD", "TOKEN"] {
        assert!(!logs.contains(planted), "a log line holds {planted:?}: {logs}");
    }
    // The events of this component: the registry's `FileObserved` names the paths that were scanned, denied ones included (the tree lists
    // them with class `denied`); `PickupChanged` is ids only.
    let picked: Vec<_> =
        b.r.world
            .rig
            .bus
            .published()
            .await
            .into_iter()
            .filter(|e| matches!(e, domain::DomainEvent::PickupChanged { .. }))
            .collect();
    assert!(!picked.is_empty());
    let events = format!("{picked:?}");
    for planted in [SECRET, CONTENT, DENIED, "TOKEN", "DB_PASSWORD"] {
        assert!(!events.contains(planted), "an event holds {planted:?}");
    }
    let stored: String = sqlx::query_scalar("SELECT string_agg(pending_files::text, ' ') FROM service_state")
        .fetch_one(&b.r.world.rig.pool)
        .await
        .unwrap();
    assert!(
        !stored.contains(DENIED),
        "the stored list does not name a denied file: {stored}"
    );
    // Metrics: counts, with the closed vocabulary of labels.
    let mut registry = prometheus_client::registry::Registry::default();
    b.r.world.rig.metrics.register(&mut registry);
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    for planted in [SECRET, DENIED, "sit1", "tx-api"] {
        assert!(!text.contains(planted), "a metric holds {planted:?}");
    }
}
