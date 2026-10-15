//! The structure checks C9, C10 and C11 (T10, A45, D82 to D88; S10, rule 7, rule 10): the findings the engine's lane state does not carry,
//! computed by the registry from the files, the property views, the Deployments' environment names, the folders and `channels.yml`, written
//! by fingerprint like the findings of C1 to C8, and cheap: a computation of a swimlane that nothing moved in reads nothing, and one that a
//! one-file change moved reads one blob and evaluates one file. Over a real Postgres, with the real ingestion, pipeline and blob store.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use hub_platform::jobs::JobError;
use hub_registry::Structure;
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::StructureResult;
use hub_registry::pipeline::c9_c11::{LaneOutcome, Mode};
use serde_json::Value;
use sqlx::Row;
use support::cluster::{dep, report};
use support::log_capture::LogCapture;
use support::rig::T;
use support::statements::statements;
use support::world::World;

/// When the config-server of a test started: after the first inventory (at `T`), before the first change (`T + 1,000`).
const STARTED: i64 = T + 500;

/// One open finding of the structure kinds, as the table holds it.
#[derive(Debug, Clone, PartialEq)]
struct Found {
    id: i64,
    kind: String,
    path: Option<String>,
    severity: String,
    rule_ids: Vec<String>,
    message: String,
    details: Value,
    locations: Value,
    detected_ms: i64,
}

struct Bed {
    w: World,
    st: Structure,
}

impl Bed {
    /// A swimlane `sit1` that deploys `tenants`, with the inventory `files` computed by the pipeline.
    async fn new(tenants: &[&str], files: &[(&str, &str)]) -> Option<Self> {
        Self::with(tenants, files, RegistryConfig::default()).await
    }

    async fn with(tenants: &[&str], files: &[(&str, &str)], config: RegistryConfig) -> Option<Self> {
        let mut w = World::with_config(tenants, config).await?;
        w.inventory(files).await;
        w.whole().await;
        let st = w.pipeline.structure();
        Some(Self { w, st })
    }

    fn lane(&self) -> domain::SwimlaneId {
        self.w.lane()
    }

    /// Compute the swimlane's checks as the sweeper does: a swimlane that nothing moved in is left alone.
    async fn run(&self) -> LaneOutcome {
        self.recompute(Mode::Gated).await
    }

    /// The full pass: whatever the gate says.
    async fn full(&self) -> LaneOutcome {
        self.recompute(Mode::Full).await
    }

    async fn recompute(&self, mode: Mode) -> LaneOutcome {
        let mut report = self.st.recompute(&[self.lane()], mode).await;
        assert!(report.failed.is_empty(), "{report:?}");
        report.lanes.remove(0)
    }

    /// Files changed on NFS, then the pipeline's batch for them.
    async fn change(&mut self, files: &[(&str, &str)]) {
        self.w.nfs_change(files).await;
        let paths: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
        self.w.paths(&paths).await;
    }

    async fn delete(&mut self, paths: &[&str]) {
        self.w.nfs_delete(paths).await;
        self.w.paths(paths).await;
    }

    /// The Deployments the agent reports (a full report) and the config-server's start; then the pipeline's batch that the new
    /// application names make (they decide which files are property sources, so the swimlane is computed whole, as its job does).
    async fn cluster(&self, deployments: Vec<domain::DeploymentInfo>, started: Option<i64>) {
        self.w
            .rig
            .cluster(report(true, deployments, started))
            .await
            .unwrap();
        self.w.whole().await;
    }

    async fn sql(&self, statement: &str) {
        sqlx::raw_sql(statement).execute(&self.w.rig.pool).await.unwrap();
    }

    /// The open findings of C9 to C11, by kind and path.
    async fn found(&self) -> Vec<Found> {
        let rows = sqlx::query(
            "SELECT id, kind, path, severity, rule_ids, message, details, locations, \
                    (EXTRACT(EPOCH FROM detected_at) * 1000)::bigint AS detected_ms \
             FROM findings WHERE swimlane_id = $1 AND resolved_at IS NULL \
               AND kind IN ('ambiguous_file_name', 'unresolved_placeholder', 'config_server_restart_required') \
             ORDER BY kind, path, id LIMIT 10000",
        )
        .bind(self.lane().as_str())
        .fetch_all(&self.w.rig.pool)
        .await
        .unwrap();
        rows.iter()
            .map(|r| Found {
                id: r.get("id"),
                kind: r.get("kind"),
                path: r.get("path"),
                severity: r.get("severity"),
                rule_ids: r.get("rule_ids"),
                message: r.get("message"),
                details: r.get("details"),
                locations: r.get("locations"),
                detected_ms: r.get("detected_ms"),
            })
            .collect()
    }

    async fn of_kind(&self, kind: &str) -> Vec<Found> {
        self.found()
            .await
            .into_iter()
            .filter(|f| f.kind == kind)
            .collect()
    }

    /// Every finding row ever written for the structure kinds, resolved or not (to say that a finding was resolved, not deleted).
    async fn resolved(&self, kind: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM findings WHERE swimlane_id = $1 AND kind = $2 AND resolved_at IS NOT NULL",
        )
        .bind(self.lane().as_str())
        .bind(kind)
        .fetch_one(&self.w.rig.pool)
        .await
        .unwrap()
    }

    async fn registry_row(&self) -> (i64, i32) {
        let (version, _, count, _) = self.w.registry_row().await;
        (version, count)
    }

    async fn findings_changed(&self) -> usize {
        self.w.findings_changed_events().await
    }
}

/// What the code under test sent: the statements sqlx logged, without the event bus's relay, which reads and marks the outbox on a task of its
/// own on the same thread (how often it looks is the timing of the test).
fn sent(capture: &LogCapture) -> Vec<String> {
    statements(capture)
        .into_iter()
        .map(|s| s.text)
        .filter(|t| !t.contains("outbox"))
        .collect()
}

async fn snapshot(b: &Bed) -> Vec<Found> {
    let mut all = b.found().await;
    // The ids and times are the history of the rows; what the table says is the rest.
    for f in &mut all {
        f.id = 0;
        f.detected_ms = 0;
    }
    all
}

async fn reasons_of(b: &Bed) -> Vec<String> {
    b.of_kind("config_server_restart_required")
        .await
        .iter()
        .map(|f| f.details["reason"].as_str().unwrap().to_owned())
        .collect()
}

fn keys_of(f: &Found) -> Vec<String> {
    f.details["keys"]
        .as_array()
        .map(|a| a.iter().map(|k| k["key"].as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------- C9

#[tokio::test]
async fn the_same_name_in_two_search_folders_is_one_finding_that_names_the_first_copy_and_goes_when_the_copies_do()
 {
    let Some(mut b) = Bed::new(
        &["sit1"],
        &[
            ("a/authentication-config.yaml", "x: 1\n"),
            ("b/authentication-config.yaml", "x: 2\n"),
            ("c/authentication-config.yaml", "x: 3\n"),
            // The tenant's copy shares the folder with its base file: one folder, not two.
            ("a/authentication-config-sit1.yaml", "x: 4\n"),
            ("a/unique.yaml", "u: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    // Git has the first copy as NFS has it: nothing is pending for the file the finding names.
    b.w.base_commit(&[("config/a/authentication-config.yaml", Some("x: 1\n"))]);
    b.w.whole().await;
    let out = b.run().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert!(out.c9_computed);
    let c9 = b.of_kind("ambiguous_file_name").await;
    assert_eq!(c9.len(), 1, "{c9:#?}");
    assert_eq!(c9[0].path.as_deref(), Some("a/authentication-config.yaml"));
    assert!(c9[0].message.contains("3 folders"), "{}", c9[0].message);
    assert_eq!(c9[0].details["name"], "authentication-config.yaml");
    assert_eq!(c9[0].details["total"], 3);
    // The engine's defaults rank no C9 (D76): it is low unless the state of the file it names is rated (a file ahead on NFS is medium, like
    // any finding about it) or an admin's rule says more.
    assert_eq!(
        (c9[0].severity.as_str(), c9[0].rule_ids.len()),
        ("low", 0),
        "{c9:#?}"
    );

    // One copy goes: two folders still. Both go but one: it is no longer ambiguous.
    b.delete(&["c/authentication-config.yaml"]).await;
    b.run().await;
    assert!(
        b.of_kind("ambiguous_file_name").await[0]
            .message
            .contains("2 folders")
    );
    b.delete(&["b/authentication-config.yaml"]).await;
    let out = b.run().await;
    assert_eq!(out.resolved, 1);
    assert!(b.of_kind("ambiguous_file_name").await.is_empty());
    assert_eq!(
        b.resolved("ambiguous_file_name").await,
        1,
        "resolved, not deleted"
    );
}

#[tokio::test]
async fn a_channel_folders_copy_is_not_an_ambiguity_and_channels_yml_is_what_says_which_folder_is_a_channel()
{
    let Some(mut b) = Bed::new(
        &["sit1"],
        &[
            ("a/x.yaml", "x: 1\n"),
            ("a/web/x.yaml", "x: 2\n"),
            ("keep.yml", "k: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    // No channels.yml: `a/web` is a folder like another, and the config-server searches it.
    b.run().await;
    assert_eq!(b.of_kind("ambiguous_file_name").await.len(), 1);
    // `channels.yml` lists `web`: its folder is how the channel overrides a file.
    b.change(&[("channels.yml", "channels:\n  - web\n")]).await;
    let out = b.run().await;
    assert!(
        out.c9_computed,
        "the channels are part of what C9 is computed from"
    );
    assert!(b.of_kind("ambiguous_file_name").await.is_empty());
}

#[tokio::test]
async fn an_earlier_copy_moves_the_path_of_a_c9_and_never_reopens_it() {
    let Some(mut b) = Bed::new(&["sit1"], &[("b/x.yaml", "x: 1\n"), ("c/x.yaml", "x: 2\n")]).await else {
        return;
    };
    b.run().await;
    let before = b.of_kind("ambiguous_file_name").await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].path.as_deref(), Some("b/x.yaml"));
    // A copy that sorts first appears: the finding's path is now that one, and it is the same finding (its identity is its name).
    b.change(&[("a/x.yaml", "x: 0\n")]).await;
    b.run().await;
    let after = b.of_kind("ambiguous_file_name").await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].path.as_deref(), Some("a/x.yaml"));
    assert_eq!(after[0].id, before[0].id, "the same row");
    assert_eq!(
        after[0].detected_ms, before[0].detected_ms,
        "open since the first time"
    );
    assert_eq!(b.resolved("ambiguous_file_name").await, 0);
}

// ---------------------------------------------------------------------------------------------------------------- C10

/// A resource with three placeholders: one the property view fills, one an environment variable fills, one nothing fills.
const TEMPLATE: &str = "<a href=\"${DB_URL}\" port=\"${PORT}\" token=\"${FROM_ENV}\"/>\n";

async fn c10_bed() -> Option<Bed> {
    let b = Bed::new(
        &["sit1"],
        &[
            ("application.properties", "PORT = 8080\n"),
            ("svc/template.xml", TEMPLATE),
            ("svc/svc.yml", "name: svc\n"),
        ],
    )
    .await?;
    b.cluster(
        vec![dep("svc-api").app("svc").names(&["FROM_ENV", "OTHER"]).build()],
        Some(STARTED),
    )
    .await;
    Some(b)
}

#[tokio::test]
async fn a_placeholder_no_view_and_no_environment_fills_is_a_finding_and_each_source_clears_it() {
    let Some(mut b) = c10_bed().await else { return };
    b.run().await;
    let c10 = b.of_kind("unresolved_placeholder").await;
    assert_eq!(c10.len(), 1, "{c10:#?}");
    assert_eq!(c10[0].path.as_deref(), Some("svc/template.xml"));
    assert_eq!(
        keys_of(&c10[0]),
        ["DB_URL"],
        "PORT is in the property view and FROM_ENV is an environment variable of the Deployment that reads the file"
    );
    assert_eq!(c10[0].details["keys"][0]["tenants"], serde_json::json!(["sit1"]));
    assert!(c10[0].message.contains("DB_URL"), "{}", c10[0].message);
    assert_eq!(
        c10[0].locations.as_array().map(Vec::len),
        Some(0),
        "an XML file has no settings to point at"
    );

    // The property view gets the key: a root property source fills it for every application.
    b.change(&[("application.properties", "PORT = 8080\nDB_URL = jdbc:x\n")])
        .await;
    let out = b.run().await;
    assert_eq!(out.resolved, 1);
    assert!(b.of_kind("unresolved_placeholder").await.is_empty());
    assert_eq!(b.resolved("unresolved_placeholder").await, 1);

    // And the other way: the key leaves the view, and the finding is back, as a new one.
    b.change(&[("application.properties", "PORT = 8080\n")]).await;
    b.run().await;
    assert_eq!(b.of_kind("unresolved_placeholder").await.len(), 1);
}

#[tokio::test]
async fn an_environment_variable_that_goes_is_seen_by_the_full_pass_and_not_by_a_look_at_the_gates() {
    let Some(b) = c10_bed().await else { return };
    b.run().await;
    assert_eq!(keys_of(&b.of_kind("unresolved_placeholder").await[0]), ["DB_URL"]);
    // The Deployment no longer has FROM_ENV. Nothing the gate holds moved (the version, the config-server's start, the rules), so the cheap
    // look leaves the swimlane alone: that is the limit of a gate, and the reason there is a full pass.
    b.cluster(
        vec![dep("svc-api").app("svc").names(&["OTHER"]).build()],
        Some(STARTED),
    )
    .await;
    assert_eq!(b.run().await.result, StructureResult::Unchanged);
    assert_eq!(keys_of(&b.of_kind("unresolved_placeholder").await[0]), ["DB_URL"]);
    let out = b.full().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert_eq!(
        keys_of(&b.of_kind("unresolved_placeholder").await[0]),
        ["DB_URL", "FROM_ENV"]
    );
    assert_eq!(out.updated, 1, "the same finding, with one more key");
}

#[tokio::test]
async fn with_two_tenants_the_finding_names_the_tenants_whose_view_lacks_the_key() {
    let Some(b) = Bed::new(
        &["sit1", "sit2"],
        &[
            ("application-sit1.properties", "ONLY_SIT1 = a\nBOTH = b\n"),
            ("application-sit2.properties", "BOTH = c\n"),
            ("svc/template.xml", "<x a=\"${ONLY_SIT1}\" b=\"${BOTH}\"/>\n"),
        ],
    )
    .await
    else {
        return;
    };
    b.run().await;
    let c10 = b.of_kind("unresolved_placeholder").await;
    assert_eq!(c10.len(), 1, "{c10:#?}");
    assert_eq!(keys_of(&c10[0]), ["ONLY_SIT1"], "BOTH is in both views");
    assert_eq!(c10[0].details["keys"][0]["tenants"], serde_json::json!(["sit2"]));
}

#[tokio::test]
async fn a_structured_resource_with_a_finding_points_at_the_settings_that_hold_the_placeholder() {
    let Some(b) = Bed::new(
        &["sit1"],
        &[
            ("svc/app.yml", "name: x\nurl: ${NOT_THERE}/api\nother: y\n"),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    b.cluster(vec![dep("api").app("svc").build()], None).await;
    b.run().await;
    let c10 = b.of_kind("unresolved_placeholder").await;
    assert_eq!(c10.len(), 1, "{c10:#?}");
    assert_eq!(c10[0].locations[0]["path"], "url");
    assert_eq!(c10[0].locations[0]["lines"]["start"], 2);
}

#[tokio::test]
async fn property_sources_and_denied_files_are_not_resources_and_are_not_checked() {
    let Some(b) = Bed::new(
        &["sit1"],
        &[
            // A property source of the application `svc`: merged setting by setting, not served whole.
            ("svc.yml", "a: ${NOT_THERE}\n"),
            // Denied by name: its content is never read.
            ("keys/server.pem", "-----BEGIN PRIVATE KEY-----\n${NOT_THERE}\n"),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    b.cluster(vec![dep("api").app("svc").build()], None).await;
    b.w.rig.blobs.forget_requests();
    let out = b.run().await;
    assert!(b.of_kind("unresolved_placeholder").await.is_empty());
    assert_eq!(out.c10_files, 0, "no resource: {out:?}");
    let pem = ports::content_hash(b"-----BEGIN PRIVATE KEY-----\n${NOT_THERE}\n");
    assert!(
        !b.w.rig.blobs.requested().contains(&pem),
        "a denied file's content is never read (D79)"
    );
}

#[tokio::test]
async fn a_resource_nobody_is_known_to_read_has_the_environment_of_every_deployment() {
    let Some(b) = Bed::new(
        &["sit1"],
        &[
            ("shared/template.xml", "<x a=\"${X_TOKEN}\" b=\"${NOPE}\"/>\n"),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    // `shared` is no application's folder, so any service may ask for the file: a variable that one Deployment has fills it.
    b.cluster(
        vec![
            dep("api").app("svc").names(&["X_TOKEN"]).build(),
            dep("batch").app("other").build(),
        ],
        None,
    )
    .await;
    b.run().await;
    let c10 = b.of_kind("unresolved_placeholder").await;
    assert_eq!(c10.len(), 1);
    assert_eq!(keys_of(&c10[0]), ["NOPE"]);
}

#[tokio::test]
async fn a_swimlane_that_deploys_no_tenant_has_no_c10() {
    let Some(b) = Bed::new(
        &[],
        &[
            ("svc/template.xml", "<x a=\"${NOT_THERE}\"/>\n"),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    let out = b.run().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert_eq!(out.c10_files, 0);
    assert!(b.of_kind("unresolved_placeholder").await.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------- C11

#[tokio::test]
async fn a_folder_made_after_the_config_server_started_needs_a_restart_and_a_restart_clears_it() {
    let Some(mut b) = Bed::new(&["sit1"], &[("old/x.yml", "a: 1\n"), ("svc/svc.yml", "n: 1\n")]).await else {
        return;
    };
    b.cluster(vec![], Some(STARTED)).await;
    b.run().await;
    assert!(
        b.of_kind("config_server_restart_required").await.is_empty(),
        "the first inventory is not a change (A11)"
    );
    // A file in a folder that did not exist when the config-server started (the change is seen at `T + 1,000`).
    b.change(&[("newdir/y.yml", "b: 2\n")]).await;
    let out = b.run().await;
    assert_eq!(out.inserted, 1);
    let c11 = b.of_kind("config_server_restart_required").await;
    assert_eq!(c11.len(), 1, "{c11:#?}");
    assert_eq!(c11[0].path.as_deref(), Some("newdir"));
    assert_eq!(c11[0].details["reason"], "newfolders");
    assert!(
        c11[0].message.contains("created after the config-server started"),
        "{}",
        c11[0].message
    );
    // The config-server restarts (its pod starts after the change): it searches the folder now.
    b.cluster(vec![], Some(T + 5_000)).await;
    b.run().await;
    assert!(b.of_kind("config_server_restart_required").await.is_empty());
    assert_eq!(b.resolved("config_server_restart_required").await, 1);
}

#[tokio::test]
async fn a_changed_channels_yml_needs_a_restart_and_a_channel_without_a_folder_says_so() {
    let Some(mut b) = Bed::new(
        &["sit1"],
        &[
            ("channels.yml", "channels:\n  - web\n"),
            ("web/x.yml", "a: 1\n"),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    b.cluster(vec![], Some(STARTED)).await;
    b.run().await;
    assert!(
        b.of_kind("config_server_restart_required").await.is_empty(),
        "the list of the first inventory is the one the config-server started with"
    );
    // A channel is added: the list changed after the start, and `atm` has no folder.
    b.change(&[("channels.yml", "channels:\n  - web\n  - atm\n")])
        .await;
    b.run().await;
    let mut reasons = reasons_of(&b).await;
    reasons.sort();
    assert_eq!(
        reasons,
        ["channellistedwithoutfolder", "channelsymlchanged"],
        "{reasons:?}"
    );
    for f in b.of_kind("config_server_restart_required").await {
        assert_eq!(f.path.as_deref(), Some("channels.yml"));
    }
    // The folder for it appears: the channel has its folder now, and the config-server, which did not know `atm` as a channel, still
    // searches it. The folder is a channel's, so it is not a new folder.
    b.change(&[("atm/x.yml", "a: 1\n")]).await;
    b.run().await;
    let mut reasons = reasons_of(&b).await;
    reasons.sort();
    assert_eq!(
        reasons,
        ["channelfoldernotlistedatstart", "channelsymlchanged"],
        "{reasons:?}"
    );
}

#[tokio::test]
async fn an_unknown_config_server_start_switches_c11_off() {
    let Some(mut b) = Bed::new(&["sit1"], &[("old/x.yml", "a: 1\n"), ("svc/svc.yml", "n: 1\n")]).await else {
        return;
    };
    b.change(&[("newdir/y.yml", "b: 2\n")]).await;
    let out = b.run().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert!(b.of_kind("config_server_restart_required").await.is_empty());
}

// ---------------------------------------------------------------------------------------------------------------- cost

#[tokio::test]
async fn a_swimlane_nothing_moved_in_is_not_read_and_a_second_pass_changes_nothing() {
    let Some(b) = c10_bed().await else { return };
    b.run().await;
    assert_eq!(b.of_kind("unresolved_placeholder").await.len(), 1);
    let (version, count) = b.registry_row().await;
    let events = b.findings_changed().await;

    // The cheap look: one statement (the gates), no row read, nothing written, no event.
    let (capture, _guard) = LogCapture::install();
    let out = b.run().await;
    let all = sent(&capture);
    assert_eq!(out.result, StructureResult::Unchanged);
    assert_eq!(all.len(), 1, "only the gates are read: {all:#?}");
    assert!(all[0].contains("swimlane_registry"));
    assert_eq!(b.registry_row().await, (version, count));
    assert_eq!(b.findings_changed().await, events);

    // The full pass reads the swimlane and finds the findings as they are: it writes nothing, opens no transaction.
    let (capture, _guard2) = LogCapture::install();
    let out = b.full().await;
    let all = sent(&capture);
    assert_eq!(out.result, StructureResult::Computed);
    assert_eq!(out.written(), 0);
    assert!(
        !all.iter()
            .any(|s| s.contains("INSERT") || s.contains("UPDATE") || s.contains("BEGIN")),
        "a computation that changes nothing writes nothing: {all:#?}"
    );
    assert_eq!(b.registry_row().await, (version, count));
    assert_eq!(b.findings_changed().await, events);
}

/// `n` resources in `n / 10` folders, each with an unresolved placeholder of its own, an application per folder, and one property source.
fn many(n: usize) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> =
        vec![("application.properties".to_owned(), "PORT = 1\n".to_owned())];
    for i in 0..n {
        files.push((
            format!("app{}/file{i}.xml", i % 10),
            format!("<a p=\"${{PORT}}\" k=\"${{MISSING_{i}}}\"/>\n"),
        ));
    }
    for a in 0..10 {
        files.push((format!("app{a}/app{a}.yml"), "n: 1\n".to_owned()));
    }
    files
}

async fn many_bed(n: usize) -> Option<Bed> {
    let files = many(n);
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let b = Bed::new(&["sit1"], &refs).await?;
    let deps = (0..10)
        .map(|a| dep(&format!("d{a}")).app(&format!("app{a}")).build())
        .collect();
    b.cluster(deps, Some(STARTED)).await;
    Some(b)
}

#[tokio::test]
async fn a_one_file_change_reads_one_blob_and_evaluates_one_file() {
    let Some(mut b) = many_bed(60).await else { return };
    let first = b.run().await;
    assert_eq!(first.c10_files, 60);
    assert_eq!(
        first.c10_evaluated, 60,
        "the first computation evaluates every file"
    );
    assert_eq!(b.of_kind("unresolved_placeholder").await.len(), 60);

    // One resource changes: another key is missing in it.
    let changed = "<a p=\"${PORT}\" k=\"${CHANGED_KEY}\"/>\n";
    b.change(&[("app3/file3.xml", changed)]).await;
    b.w.rig.blobs.forget_requests();
    let (capture, _guard) = LogCapture::install();
    let out = b.run().await;
    let small = sent(&capture).len();
    assert_eq!(out.result, StructureResult::Computed);
    assert_eq!(out.c10_files, 60);
    assert_eq!((out.c10_evaluated, out.c10_cached), (1, 59), "{out:?}");
    assert!(
        !out.c9_computed,
        "no file came or went: the names are the ones C9 was computed from"
    );
    assert_eq!(
        b.w.rig.blobs.requested(),
        vec![ports::content_hash(changed.as_bytes())],
        "one blob read: the file that changed"
    );
    assert_eq!(out.blobs_read, 1);
    assert_eq!(
        (out.inserted, out.updated, out.resolved),
        (0, 1, 0),
        "the same finding, with another key"
    );
    assert!(
        keys_of(
            &b.of_kind("unresolved_placeholder")
                .await
                .into_iter()
                .find(|f| f.path.as_deref() == Some("app3/file3.xml"))
                .unwrap()
        ) == ["CHANGED_KEY"]
    );
    // The statements of this computation do not depend on how many files the swimlane has (a bigger one is below).
    let Some(mut big) = many_bed(300).await else {
        return;
    };
    big.run().await;
    big.change(&[("app3/file3.xml", changed)]).await;
    let (capture, _guard2) = LogCapture::install();
    let out = big.run().await;
    assert_eq!((out.c10_evaluated, out.c10_cached), (1, 299), "{out:?}");
    assert_eq!(
        sent(&capture).len(),
        small,
        "the same statements for 60 files and for 300"
    );
}

#[tokio::test]
async fn a_change_to_a_property_source_evaluates_the_files_of_the_applications_it_reaches_and_no_others() {
    let Some(mut b) = Bed::new(
        &["sit1"],
        &[
            ("core/core.yml", "n: 1\n"),
            ("pay/pay.yml", "n: 1\n"),
            ("core/a.xml", "<a v=\"${CORE_ONLY}\"/>\n"),
            ("core/b.xml", "<a v=\"${CORE_ONLY}\"/>\n"),
            ("pay/c.xml", "<a v=\"${CORE_ONLY}\"/>\n"),
        ],
    )
    .await
    else {
        return;
    };
    b.cluster(
        vec![dep("core").app("core").build(), dep("pay").app("pay").build()],
        None,
    )
    .await;
    b.run().await;
    assert_eq!(b.of_kind("unresolved_placeholder").await.len(), 3);
    // `core.yml` is a property source of the application `core` only (its name is the application's).
    b.change(&[("core/core.yml", "n: 1\nCORE_ONLY: yes\n")]).await;
    let out = b.run().await;
    assert_eq!(out.c10_files, 3);
    assert_eq!(
        (out.c10_evaluated, out.c10_cached),
        (2, 1),
        "the two files of `core`; the file of `pay` has the digest it had: {out:?}"
    );
    let left: Vec<String> = b
        .of_kind("unresolved_placeholder")
        .await
        .into_iter()
        .filter_map(|f| f.path)
        .collect();
    assert_eq!(
        left,
        ["pay/c.xml"],
        "the property view of `core` has the key now; `pay`'s has not"
    );
}

#[tokio::test]
async fn a_cold_computation_says_what_the_warm_one_did_after_every_kind_of_change() {
    let Some(mut b) = c10_bed().await else { return };
    let mut steps: Vec<String> = Vec::new();
    macro_rules! settle {
        ($what:expr) => {{
            b.run().await;
            b.full().await;
            let warm = snapshot(&b).await;
            b.st.forget();
            let cold = b.full().await;
            assert_eq!(
                cold.written(),
                0,
                "{}: a cold computation wrote what the warm one did not: {cold:?}",
                $what
            );
            assert_eq!(snapshot(&b).await, warm, "{}", $what);
            steps.push($what.to_owned());
        }};
    }
    settle!("the first computation");
    b.change(&[(
        "svc/template.xml",
        "<a k=\"${DB_URL}\" t=\"${FROM_ENV}\" n=\"${NEW}\"/>\n",
    )])
    .await;
    settle!("a resource gains a placeholder");
    b.change(&[("application.properties", "PORT = 8080\nNEW = 1\n")])
        .await;
    settle!("a property source gains a key");
    b.cluster(
        vec![dep("svc-api").app("svc").names(&["OTHER", "DB_URL"]).build()],
        Some(STARTED),
    )
    .await;
    settle!("a Deployment's environment changes");
    b.change(&[("other/template.xml", "<a k=\"${DB_URL}\"/>\n")])
        .await;
    settle!("a resource nobody is known to read appears");
    b.change(&[("a/dup.yaml", "x: 1\n"), ("b/dup.yaml", "x: 2\n")])
        .await;
    settle!("two copies of one name");
    b.change(&[("newdir/z.yml", "z: 1\n")]).await;
    settle!("a folder after the config-server started");
    b.delete(&["svc/template.xml"]).await;
    settle!("a resource goes");
    b.w.add_tenant("sit2").await;
    b.w.whole().await;
    settle!("a tenant is added");
    assert_eq!(steps.len(), 9);
}

// ---------------------------------------------------------------------------------------------------------------- findings

#[tokio::test]
async fn an_admins_rule_raises_what_a_structure_finding_is_rated_and_the_defaults_never_lower_it() {
    let Some(b) = c10_bed().await else { return };
    b.run().await;
    let before = b.of_kind("unresolved_placeholder").await;
    assert_eq!(before.len(), 1);
    // The engine's defaults rank no C10; the rating is that of the file it names (not on Git: medium, "any other out-of-band change").
    assert_eq!(before[0].severity, "medium");
    assert_eq!(before[0].rule_ids, ["default.out_of_band"]);

    // An admin's rule rates every C10 high. The rules are part of what a swimlane is computed under: the pipeline computes it whole, its
    // fingerprint moves, and the next look at the gate computes the checks again.
    b.sql(
        "INSERT INTO severity_rules (id, scope, matcher, severity) \
         VALUES ('placeholders-matter', 'finding', '{\"finding_kinds\": [\"unresolved_placeholder\"]}', 'high')",
    )
    .await;
    b.w.whole().await;
    let out = b.run().await;
    assert_eq!(
        out.result,
        StructureResult::Computed,
        "the gate moved with the rules' fingerprint"
    );
    assert_eq!(out.updated, 1);
    let after = b.of_kind("unresolved_placeholder").await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before[0].id, "the same finding, rated again");
    assert_eq!(after[0].severity, "high");
    assert!(
        after[0].rule_ids.contains(&"placeholders-matter".to_owned()),
        "{:?}",
        after[0].rule_ids
    );

    // A rule that says less changes nothing: the highest of the rules that match is the rating.
    b.sql(
        "INSERT INTO severity_rules (id, scope, matcher, severity) \
         VALUES ('placeholders-are-minor', 'finding', '{\"finding_kinds\": [\"unresolved_placeholder\"]}', 'low')",
    )
    .await;
    b.w.whole().await;
    b.run().await;
    assert_eq!(b.of_kind("unresolved_placeholder").await[0].severity, "high");
}

#[tokio::test]
async fn a_change_of_the_findings_bumps_the_version_counts_every_kind_and_publishes_one_findings_changed() {
    let Some(mut b) = c10_bed().await else { return };
    let open = |b: &Bed| {
        let pool = b.w.rig.pool.clone();
        let lane = b.lane();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM findings WHERE swimlane_id = $1 AND resolved_at IS NULL",
            )
            .bind(lane.as_str())
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let (version0, count0) = b.registry_row().await;
    assert!(
        count0 > 0,
        "the pipeline's own findings (drift of the files Git does not have) are counted"
    );
    let events0 = b.findings_changed().await;

    let out = b.run().await;
    assert_eq!(out.inserted, 1);
    let (version1, count1) = b.registry_row().await;
    assert_eq!(
        version1,
        version0 + 1,
        "an ETag over the swimlane moves with its findings"
    );
    assert_eq!(i64::from(count1), open(&b).await, "every kind is counted");
    assert_eq!(i64::from(count1), i64::from(count0) + 1);
    assert_eq!(
        b.findings_changed().await,
        events0 + 1,
        "one event for the change"
    );

    // A computation that changes nothing moves nothing.
    b.full().await;
    assert_eq!(b.registry_row().await, (version1, count1));
    assert_eq!(b.findings_changed().await, events0 + 1);

    // The pipeline's next batch keeps the count of every kind (it counts the table, not its own rows).
    b.change(&[("svc/svc.yml", "name: svc\nextra: 1\n")]).await;
    let (_, count2) = b.registry_row().await;
    assert_eq!(i64::from(count2), open(&b).await);
    assert!(
        i64::from(count2) > i64::from(count0),
        "the structure finding is still in the count"
    );
}

#[tokio::test]
async fn the_findings_are_read_through_the_findings_port_for_a_viewer_and_a_pending_user_is_refused() {
    use domain::{FindingKind, FindingsQuery, Page, UserStatus};
    use ports::{ReadError, RegistryRead};
    use support::reads::Reads;

    let Some(mut r) = Reads::with(&["sit1"], RegistryConfig::default()).await else {
        return;
    };
    r.world
        .inventory(&[
            ("a/x.yaml", "x: 1\n"),
            ("b/x.yaml", "x: 2\n"),
            ("svc/template.xml", TEMPLATE),
        ])
        .await;
    r.world.whole().await;
    let structure = r.world.pipeline.structure();
    let report = structure.recompute(&[r.lane()], Mode::Gated).await;
    assert!(report.failed.is_empty());
    let query = FindingsQuery {
        swimlane: Some(r.lane()),
        kinds: vec![FindingKind::AmbiguousFileName, FindingKind::UnresolvedPlaceholder],
        min_severity: None,
        page: Page::new(50, None).unwrap(),
    };
    let found = r.service.findings(&Reads::viewer(), query.clone()).await.unwrap();
    let kinds: Vec<FindingKind> = found.items.iter().map(|f| f.kind).collect();
    assert!(kinds.contains(&FindingKind::AmbiguousFileName), "{kinds:?}");
    assert!(kinds.contains(&FindingKind::UnresolvedPlaceholder), "{kinds:?}");
    let pending = ports::conformance::sample::user(9, None, UserStatus::Pending);
    assert_eq!(
        r.service.findings(&pending, query).await.unwrap_err(),
        ReadError::Forbidden,
        "authorization stays where it was: the port refuses a user who is not active, whatever the kind"
    );
    r.world.paths(&["svc/template.xml"]).await;
}

// ---------------------------------------------------------------------------------------------------------------- S10, rule 10

#[tokio::test]
async fn nothing_of_a_value_a_secret_or_an_environment_value_reaches_a_log_a_finding_or_an_event() {
    const SETTING: &str = "hunter2-setting-value-7f3a";
    const TEXT: &str = "text-only-in-the-file-5c1d";
    const ENV_VALUE: &str = "env-secret-value-9b2e";
    let Some(b) = Bed::new(
        &["sit1"],
        &[
            (
                "application.properties",
                "PORT = 8080\nPASSWORD = hunter2-setting-value-7f3a\n",
            ),
            (
                "svc/app.yml",
                "note: text-only-in-the-file-5c1d\nurl: ${DB_URL}\npassword: ${PASSWORD}\n",
            ),
            ("svc/svc.yml", "n: 1\n"),
        ],
    )
    .await
    else {
        return;
    };
    let (capture, _guard) = LogCapture::install();
    // A Deployment with a variable whose value is a secret: the agent sends a value only for the allowlist (D88), and the registry keeps
    // only those; what is kept of the others is their names.
    b.cluster(
        vec![
            dep("api")
                .app("svc")
                .env("DB_PASSWORD", ENV_VALUE)
                .names(&["DB_PASSWORD"])
                .build(),
        ],
        Some(STARTED),
    )
    .await;
    let out = b.run().await;
    assert!(out.written() > 0);
    let found = b.found().await;
    assert!(
        found.iter().any(|f| f.kind == "unresolved_placeholder"),
        "{found:#?}"
    );
    let logs = capture.text();
    assert!(
        logs.contains("structure checks computed"),
        "the capture works: {logs}"
    );
    let stored = serde_json::to_string(
        &found
            .iter()
            .map(|f| (&f.message, &f.details, &f.locations))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let events = format!("{:?}", b.w.rig.bus.published().await);
    for (name, planted) in [
        ("a setting value", SETTING),
        ("file text", TEXT),
        ("an environment value", ENV_VALUE),
    ] {
        assert!(!logs.contains(planted), "{name} is in a log line");
        assert!(!stored.contains(planted), "{name} is in a finding");
        assert!(!events.contains(planted), "{name} is in an event");
    }
    // No path and no key name goes to a log line either: counts and ids.
    for named in ["svc/app.yml", "DB_URL", "application.properties"] {
        assert!(
            !logs
                .lines()
                .filter(|l| l.contains("hub_registry"))
                .any(|l| l.contains(named)),
            "`{named}` is in a log line of the registry"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------- the sweeper

#[tokio::test]
async fn the_sweeper_computes_what_moved_and_leaves_the_rest_and_the_full_pass_comes_when_it_is_due() {
    let Some(mut b) = c10_bed().await else { return };
    b.w.second_lane(
        "lane2",
        &["sit1"],
        &[("a/x.yaml", "x: 1\n"), ("b/x.yaml", "x: 2\n")],
    )
    .await;
    b.w.pipeline
        .refresh_lane(&domain::SwimlaneId::parse("lane2").unwrap())
        .await
        .unwrap();

    // The first run is a full pass (nothing has been seen): both swimlanes, and what they say is written.
    let first = b.st.run_once().await.unwrap();
    assert!(first.full);
    assert_eq!(
        (first.computed, first.unchanged, first.failed),
        (2, 0, 0),
        "{first:?}"
    );
    assert!(first.written >= 2);
    // The second look finds both as they were.
    let second = b.st.run_once().await.unwrap();
    assert!(!second.full);
    assert_eq!(
        (second.computed, second.unchanged, second.written),
        (0, 2, 0),
        "{second:?}"
    );
    // A change to one swimlane: only that one is computed.
    b.change(&[("svc/template.xml", "<a k=\"${DB_URL}\" n=\"${NEW_KEY}\"/>\n")])
        .await;
    let third = b.st.run_once().await.unwrap();
    assert_eq!((third.computed, third.unchanged), (1, 1), "{third:?}");
    assert_eq!(third.written, 1);
    // Ten minutes later it is the full pass again (the interval is the clock's, never a sleep).
    b.w.rig.clock.advance_secs(10 * 60 + 1);
    let fourth = b.st.run_once().await.unwrap();
    assert!(fourth.full);
    assert_eq!(fourth.computed, 2);
    assert!(!b.st.run_once().await.unwrap().full);
}

#[tokio::test]
async fn a_run_computes_at_most_the_configured_number_of_swimlanes_and_the_ones_it_leaves_are_not_starved() {
    let config = RegistryConfig {
        structure_sweep_lanes: 1,
        ..RegistryConfig::default()
    };
    let Some(b) = Bed::with(
        &["sit1"],
        &[("a/x.yaml", "x: 1\n"), ("b/x.yaml", "x: 2\n")],
        config,
    )
    .await
    else {
        return;
    };
    b.w.second_lane(
        "lane2",
        &["sit1"],
        &[("a/y.yaml", "y: 1\n"), ("b/y.yaml", "y: 2\n")],
    )
    .await;
    b.w.pipeline
        .refresh_lane(&domain::SwimlaneId::parse("lane2").unwrap())
        .await
        .unwrap();
    let first = b.st.run_once().await.unwrap();
    assert_eq!((first.computed, first.truncated), (1, true), "{first:?}");
    let second = b.st.run_once().await.unwrap();
    assert_eq!(
        (second.computed, second.unchanged, second.truncated),
        (1, 1, false),
        "the swimlane the first run left is the one this run computes, and the one it did is not read again: {second:?}"
    );
    let third = b.st.run_once().await.unwrap();
    assert_eq!((third.computed, third.unchanged), (0, 2), "{third:?}");
}

#[tokio::test]
async fn a_run_ends_with_its_lease_before_the_next_swimlane() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Some(b) = Bed::new(&["sit1"], &[("a/x.yaml", "x: 1\n"), ("b/x.yaml", "x: 2\n")]).await else {
        return;
    };
    b.w.second_lane(
        "lane2",
        &["sit1"],
        &[("a/y.yaml", "y: 1\n"), ("b/y.yaml", "y: 2\n")],
    )
    .await;
    b.w.pipeline
        .refresh_lane(&domain::SwimlaneId::parse("lane2").unwrap())
        .await
        .unwrap();
    // The lease is held at the start and before the first swimlane, and gone before the second.
    let asked = AtomicUsize::new(0);
    let result =
        b.st.sweep(|| {
            if asked.fetch_add(1, Ordering::SeqCst) >= 2 {
                Err(JobError::LeaseLost)
            } else {
                Ok(())
            }
        })
        .await;
    assert_eq!(result, Err(JobError::LeaseLost));
    // The swimlanes go in id order: `lane2` was done before the loss, `sit1` was not reached.
    let open_in = |lane: &'static str| {
        let pool = b.w.rig.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM findings WHERE swimlane_id = $1 AND resolved_at IS NULL AND kind = 'ambiguous_file_name'",
            )
            .bind(lane)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!((open_in("lane2").await, open_in("sit1").await), (1, 0));
    // The next run goes on with the pass where it stopped: the swimlane that was done is not computed again, the other one is.
    let next = b.st.run_once().await.unwrap();
    assert!(next.full, "a pass that was cut short is not over");
    assert_eq!((next.computed, next.unchanged), (1, 1), "{next:?}");
    assert_eq!((open_in("lane2").await, open_in("sit1").await), (1, 1));
    // And now it is over.
    assert!(!b.st.run_once().await.unwrap().full);
}

#[tokio::test]
async fn the_job_has_its_spec_and_runs_a_sweep() {
    use hub_platform::jobs::JobContext;
    let Some(b) = c10_bed().await else { return };
    let jobs = b.w.pipeline.jobs().unwrap();
    let names: Vec<&str> = jobs.iter().map(|(spec, _)| spec.name()).collect();
    assert_eq!(names.last(), Some(&"registry-structure-sweeper"), "{names:?}");
    assert_eq!(
        Structure::spec(&RegistryConfig::default()).unwrap().name(),
        hub_registry::pipeline::c9_c11::JOB_NAME
    );
    let (ctx, _lease): (JobContext, _) =
        JobContext::held_until_told_otherwise(hub_registry::pipeline::c9_c11::JOB_NAME);
    let job = jobs
        .into_iter()
        .find(|(spec, _)| spec.name() == hub_registry::pipeline::c9_c11::JOB_NAME)
        .map(|(_, job)| job)
        .unwrap();
    job.run(&ctx).await.unwrap();
    assert_eq!(
        b.of_kind("unresolved_placeholder").await.len(),
        1,
        "the job did the sweep"
    );
}

// ---------------------------------------------------------------------------------------------------------------- failures

#[tokio::test]
async fn a_swimlane_the_pipeline_has_not_computed_or_that_has_no_inventory_is_not_ready() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    // An inventory, and no batch of the pipeline yet: its derived state is not there to check.
    w.inventory(&[("a/x.yaml", "x: 1\n"), ("b/x.yaml", "x: 2\n")])
        .await;
    let st = w.pipeline.structure();
    let report = st.recompute(&[w.lane()], Mode::Full).await;
    assert_eq!(report.lanes[0].result, StructureResult::NotReady);
    // A swimlane that does not exist is as ready as one with no inventory: nothing to check.
    let nowhere = domain::SwimlaneId::parse("nowhere").unwrap();
    let report = st.recompute(&[nowhere], Mode::Gated).await;
    assert_eq!(report.lanes[0].result, StructureResult::NotReady);
    assert!(report.failed.is_empty());
    // Once the pipeline has computed it, it is.
    w.whole().await;
    assert_eq!(
        st.recompute(&[w.lane()], Mode::Gated).await.lanes[0].result,
        StructureResult::Computed
    );
}

#[tokio::test]
async fn a_blob_store_that_fails_fails_the_swimlane_and_writes_nothing_and_the_next_run_tries_again() {
    let Some(b) = c10_bed().await else { return };
    b.w.rig.blobs.set_failing_reads(true);
    let report = b.st.recompute(&[b.lane()], Mode::Gated).await;
    assert_eq!(report.failed, [b.lane()]);
    assert!(b.found().await.is_empty(), "nothing was written");
    assert!(b.w.rig.metrics.structure_lane_count(StructureResult::Failed) >= 1);
    // The store is back: the swimlane is computed (a failed one was never remembered as done).
    b.w.rig.blobs.set_failing_reads(false);
    let out = b.run().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert_eq!(b.of_kind("unresolved_placeholder").await.len(), 1);
}

#[tokio::test]
async fn a_file_the_store_does_not_have_says_nothing_and_is_tried_again() {
    let Some(b) = c10_bed().await else { return };
    // The blob of the template is gone from the store (a collected blob, a store that lost it).
    let hash = ports::content_hash(TEMPLATE.as_bytes());
    sqlx::query("DELETE FROM blobs WHERE hash = $1::bytea")
        .bind(hash.as_bytes().as_slice())
        .execute(&b.w.rig.pool)
        .await
        .unwrap();
    let out = b.run().await;
    assert_eq!(out.result, StructureResult::Computed);
    assert!(
        b.of_kind("unresolved_placeholder").await.is_empty(),
        "no content, no claim"
    );
    assert_eq!(out.c10_evaluated, 0);
}
