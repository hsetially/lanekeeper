//! T11 (D85, D87, Q37, S4): tell the config-server what changed, after a write or on request, and restart the config-server when
//! check C11 asks for it. Notify is Operator only, audited exactly once, an approval user proposes it, and what it sends is
//! exactly the paths of the request. Everything runs through the real hub over a real Postgres.
#![allow(clippy::too_many_lines)]

use std::time::Duration;

use domain::{
    DomainEvent, NotifyRequest, RejectReason, RestartRequest, Role, ServiceRef, SwimlaneId, User, UserStatus,
    WriteOutcome,
};
use hub_writes::{AutoNotifier, NO_HASH, ProposalDesk as _, ProposalQuery, WriteConfig};
use ports::conformance::sample::nfs;
use ports::{AuditAction, AuditActor, AuditVia, WriteError, WriteService, verify_chain};
use serde_json::json;

use crate::rejected;
use crate::support::fake_agent::Fault;
use crate::support::rig::{Rig, START, applied, at, ctx, edit};

const SWIMLANE: &str = "sit1";

fn notify_req(s: &SwimlaneId, paths: &[&str]) -> NotifyRequest {
    NotifyRequest {
        swimlane: s.clone(),
        paths: paths.iter().map(|p| nfs(p)).collect(),
    }
}

async fn rig() -> Option<(Rig, SwimlaneId)> {
    rig_with(WriteConfig::default()).await
}

async fn rig_with(config: WriteConfig) -> Option<(Rig, SwimlaneId)> {
    let rig = Rig::with_config(crate::support::pg::TestDb::ready().await?, config).await;
    let s = rig.swimlane(SWIMLANE, true).await;
    rig.tenant(&s, "sit1").await;
    rig.seed(
        &s,
        &[
            ("application.yml", "a: 1\n"),
            ("tx-api/tx-api.yml", "k: 1\n"),
            ("tx-api/receipt.bmp", "BM"),
            ("tx-ledger/tx-ledger.yml", "l: 1\n"),
        ],
    );
    Some((rig, s))
}

fn svc(namespace: &str, name: &str) -> ServiceRef {
    ServiceRef::new(namespace, name).unwrap()
}

// ------------------------------------------------------------------------------------------------------------- manual notify

#[tokio::test]
async fn notify_sends_exactly_the_written_paths_relative_to_the_config_root() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    let paths = ["tx-api/tx-api.yml", "application.yml", "tx-api/receipt.bmp"];

    let out = rig
        .hub
        .notify(&ctx(&operator, "n-1"), notify_req(&s, &paths))
        .await
        .unwrap();
    let (hash, audit_id) = applied(&out);
    assert_eq!(hash, None, "a notification leaves no file hash");
    assert!(audit_id.get() >= 1);

    // One command, with the paths of the request: in order, relative to the config root, nothing added and nothing left out.
    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 1, "one call to the config-server");
    let texts: Vec<&str> = sent[0].iter().map(domain::NfsPath::as_str).collect();
    assert_eq!(texts, paths);
    assert!(
        texts.iter().all(|p| !p.starts_with('/') && !p.contains("..")),
        "relative to the root: {texts:?}"
    );

    // A path named twice is sent once.
    let again = rig
        .hub
        .notify(
            &ctx(&operator, "n-2"),
            notify_req(&s, &["tx-api/tx-api.yml", "tx-api/tx-api.yml", "application.yml"]),
        )
        .await
        .unwrap();
    assert!(matches!(again, WriteOutcome::Applied { .. }));
    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[1].iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
        ["tx-api/tx-api.yml", "application.yml"]
    );

    // And a notification is no write: no file was touched.
    assert_eq!(rig.agent.writes_applied(), 0);
}

#[tokio::test]
async fn notify_is_audited_and_requires_operator() {
    let Some((rig, s)) = rig().await else { return };
    let viewer = rig.user(1, Role::Viewer).await;
    let editor = rig.user(2, Role::Editor).await;
    let operator = rig.user(3, Role::Operator).await;
    let pending = rig
        .user_with_status(4, Some(Role::Admin), UserStatus::Pending)
        .await;

    // Below Operator: nothing is claimed, sent or recorded.
    for who in [&viewer, &editor, &pending] {
        assert_eq!(
            rig.hub
                .notify(&ctx(who, "n-0"), notify_req(&s, &["application.yml"]))
                .await,
            Err(WriteError::Forbidden)
        );
    }
    assert_eq!(rig.agent.notified_paths().len(), 0);
    assert_eq!(rig.count("idempotency_keys").await, 0);
    assert_eq!(rig.chain().await.len(), 0);
    assert_eq!(rig.bus.published().await.len(), 0);

    // An Operator: one call and exactly one chained audit event that names the user, the swimlane and the paths.
    let out = rig
        .hub
        .notify(
            &ctx(&operator, "n-1"),
            notify_req(&s, &["tx-api/tx-api.yml", "application.yml"]),
        )
        .await
        .unwrap();
    let (_, audit_id) = applied(&out);
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1, "exactly one audit event");
    verify_chain(&chain).unwrap();
    let e = &chain[0].event;
    assert_eq!(e.action, AuditAction::ConfigServerNotified);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: operator.id.clone()
        }
    );
    assert_eq!(e.via, AuditVia::Ui);
    assert_eq!(e.swimlane.as_ref(), Some(&s));
    assert_eq!(e.hash_before, None);
    assert_eq!(e.hash_after, None);
    assert_eq!(e.author, None);
    assert_eq!(e.approval, None);
    assert_eq!(
        e.diff.as_deref(),
        Some("notify config-server of 2 paths\ntx-api/tx-api.yml\napplication.yml")
    );
    assert_eq!(chain[0].id, audit_id);

    // The same key replays: one call, one event.
    let replay = rig
        .hub
        .notify(
            &ctx(&operator, "n-1"),
            notify_req(&s, &["tx-api/tx-api.yml", "application.yml"]),
        )
        .await
        .unwrap();
    assert_eq!(replay, out);
    assert_eq!(rig.agent.notified_paths().len(), 1);
    assert_eq!(rig.chain().await.len(), 1);

    // The same key for other paths is a misuse of the key.
    assert_eq!(
        rig.hub
            .notify(&ctx(&operator, "n-1"), notify_req(&s, &["application.yml"]))
            .await,
        Err(WriteError::IdempotencyConflict)
    );
}

#[tokio::test]
async fn notify_requests_that_cannot_be_served_send_nothing() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;

    // A swimlane that does not exist.
    let nowhere = SwimlaneId::parse("nowhere").unwrap();
    let out = rig
        .hub
        .notify(&ctx(&operator, "n-1"), notify_req(&nowhere, &["application.yml"]))
        .await
        .unwrap();
    assert_eq!(out, rejected(RejectReason::UnknownSwimlane));

    // No path, and more paths than the contract allows, are not requests.
    assert_eq!(
        rig.hub.notify(&ctx(&operator, "n-2"), notify_req(&s, &[])).await,
        Err(WriteError::BadRequest)
    );
    let many: Vec<String> = (0..=NotifyRequest::MAX_PATHS)
        .map(|n| format!("d/f{n}.yml"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        rig.hub
            .notify(&ctx(&operator, "n-3"), notify_req(&s, &many))
            .await,
        Err(WriteError::BadRequest)
    );
    // Exactly the most is fine.
    let most: Vec<&str> = many[..NotifyRequest::MAX_PATHS].to_vec();
    let out = rig
        .hub
        .notify(&ctx(&operator, "n-4"), notify_req(&s, &most))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");

    assert_eq!(rig.agent.notified_paths().len(), 1, "only the last one was sent");
    assert_eq!(rig.chain().await.len(), 1);
}

/// The contract is one to 100 paths as sent: a request of more is refused before anything is deduplicated, so a long run of the
/// same path cannot be made to look like one (and cannot cost a scan of everything that was sent).
#[tokio::test]
async fn a_request_of_more_than_the_most_paths_is_refused_even_if_they_are_all_the_same() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;

    let same = vec!["application.yml"; NotifyRequest::MAX_PATHS + 1];
    assert_eq!(
        rig.hub
            .notify(&ctx(&operator, "n-1"), notify_req(&s, &same))
            .await,
        Err(WriteError::BadRequest)
    );
    assert!(rig.agent.notified_paths().is_empty(), "nothing was sent");
    assert_eq!(rig.chain().await.len(), 0);

    // Up to the most, a repeated path still counts once.
    let twice = vec!["application.yml"; NotifyRequest::MAX_PATHS];
    let out = rig
        .hub
        .notify(&ctx(&operator, "n-2"), notify_req(&s, &twice))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.agent.notified_paths().len(), 1);
}

#[tokio::test]
async fn the_answer_of_the_config_server_decides_whether_a_notification_happened() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    let req = || notify_req(&s, &["application.yml"]);

    // The config-server refused (a server error): not recorded, the key is free for a retry, and nothing is hinted.
    rig.agent.set_notify_status(503);
    let out = rig.hub.notify(&ctx(&operator, "n-1"), req()).await.unwrap();
    assert_eq!(out, rejected(RejectReason::AgentUnavailable));
    // The agent could not act at all.
    rig.agent.set_notify_status(200);
    rig.agent.fault_next_write(Fault::Io);
    let out = rig.hub.notify(&ctx(&operator, "n-1"), req()).await.unwrap();
    assert_eq!(out, rejected(RejectReason::AgentUnavailable));
    // The agent is not connected.
    rig.agent.set_down(true);
    let out = rig.hub.notify(&ctx(&operator, "n-1"), req()).await.unwrap();
    assert_eq!(out, rejected(RejectReason::AgentUnavailable));
    rig.agent.set_down(false);
    // The answer was lost: the notification may have gone out and it cannot be known. Not recorded; the retry sends it again,
    // which is harmless (a notification refreshes a cache).
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    assert_eq!(
        rig.hub.notify(&ctx(&operator, "n-1"), req()).await,
        Err(WriteError::Unavailable)
    );
    assert_eq!(rig.chain().await.len(), 0, "nothing was recorded");
    assert_eq!(rig.bus.published().await.len(), 0);
    assert_eq!(rig.count("idempotency_keys").await, 0, "and no key is held");

    // The same key now works, and is recorded once.
    let out = rig.hub.notify(&ctx(&operator, "n-1"), req()).await.unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.chain().await.len(), 1);
}

#[tokio::test]
async fn notify_for_an_approval_user_becomes_a_proposal() {
    let Some((rig, s)) = rig().await else { return };
    let careful = rig.approval_user(1, Role::Operator).await;
    let admin = rig.user(2, Role::Admin).await;
    let paths = ["tx-api/tx-api.yml", "application.yml"];

    let id = match rig
        .hub
        .notify(&ctx(&careful, "n-1"), notify_req(&s, &paths))
        .await
        .unwrap()
    {
        WriteOutcome::ProposalCreated { id } => id,
        other => panic!("expected a proposal, got {other:?}"),
    };
    // Nothing went out, and the only audit event is the proposal's.
    assert_eq!(rig.agent.notified_paths().len(), 0);
    let chain = rig.chain().await;
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].event.action, AuditAction::ProposalCreated);
    let (action, listed): (String, Vec<String>) =
        sqlx::query_as("SELECT action, paths FROM proposals WHERE id = $1")
            .bind(id.get())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(action, "notify");
    assert_eq!(listed, paths);

    // The queue lists it. The view has one path and no action, so an approver sees the first path and no content (a residual risk).
    let page = rig
        .hub
        .list_proposals(
            &admin,
            ProposalQuery {
                status: None,
                swimlane: None,
                page: domain::Page::new(10, None).unwrap(),
            },
        )
        .await
        .unwrap();
    let view = page.items.iter().find(|v| v.id == id.get()).expect("listed");
    assert_eq!(view.path.as_str(), paths[0]);
    assert_eq!(view.new_hash, NO_HASH);
    assert!(view.hunks.is_empty() && view.setting_changes.is_empty());

    // The author cannot approve their own proposal; an admin can, and the notification then goes out with the proposal's paths.
    assert_eq!(
        rig.hub.approve_proposal(&ctx(&careful, "a-0"), id, None).await,
        Err(WriteError::Forbidden)
    );
    assert_eq!(rig.agent.notified_paths().len(), 0);
    let out = rig
        .hub
        .approve_proposal(&ctx(&admin, "a-1"), id, None)
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { hash: None, .. }), "{out:?}");
    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
        paths
    );
    let chain = rig.chain().await;
    verify_chain(&chain).unwrap();
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::ConfigServerNotified);
    assert_eq!(
        e.actor,
        AuditActor::User {
            user: admin.id.clone()
        }
    );
    assert_eq!(e.author.as_ref(), Some(&careful.id));
    assert_eq!(e.approval, Some(id));
    let status: String = sqlx::query_scalar("SELECT status FROM proposals WHERE id = $1")
        .bind(id.get())
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(status, "applied");
    assert_eq!(
        chain.len(),
        2,
        "the proposal, then the notification it carried out"
    );
}

// ------------------------------------------------------------------------------------------------------------- pickup states

struct Seeded {
    id: i64,
}

/// A Deployment the cluster report lists, with the pickup state the registry last computed for it.
#[allow(clippy::too_many_arguments)]
async fn deployment(
    rig: &Rig,
    s: &SwimlaneId,
    name: &str,
    env: serde_json::Value,
    state: &str,
    pending: &[&str],
    pending_minutes_ago: i64,
) -> Seeded {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) VALUES ($1, 'sit1', $2, $3) RETURNING id",
    )
    .bind(s.as_str())
    .bind(name)
    .bind(env)
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    let since_ms = START - pending_minutes_ago * 60_000;
    sqlx::query(
        "INSERT INTO service_state (service_id, pickup_state, pending_files, pending_since, live_by) \
         VALUES ($1, $2, $3, to_timestamp($4::bigint / 1000.0), \
                 CASE WHEN $2 = 'live_within_ttl' THEN to_timestamp(($4::bigint + 1200000) / 1000.0) END)",
    )
    .bind(id)
    .bind(state)
    .bind(json!(pending))
    .bind(since_ms)
    .execute(&rig.pool)
    .await
    .unwrap();
    Seeded { id }
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    state: String,
    pending: Vec<String>,
    live_by_ms: Option<i64>,
    pending_since_ms: Option<i64>,
}

async fn row(rig: &Rig, d: &Seeded) -> Row {
    let (state, pending, live_by_ms, pending_since_ms): (String, serde_json::Value, Option<i64>, Option<i64>) =
        sqlx::query_as(
            "SELECT pickup_state, pending_files, (EXTRACT(EPOCH FROM live_by) * 1000)::bigint, \
                    (EXTRACT(EPOCH FROM pending_since) * 1000)::bigint FROM service_state WHERE service_id = $1",
        )
        .bind(d.id)
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    Row {
        state,
        pending: serde_json::from_value(pending).unwrap(),
        live_by_ms,
        pending_since_ms,
    }
}

fn listens() -> serde_json::Value {
    json!({ "CONFIG_CLIENT_MONITOR_ACTIVEMQ_ENABLED": "true" })
}

#[allow(clippy::needless_pass_by_value)]
fn app(name: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut v = json!({ "SPRING_APPLICATION_NAME": name });
    for (k, val) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = val;
    }
    v
}

#[tokio::test]
async fn pickup_states_after_notify() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    let now = |minutes: i64| START - minutes * 60_000;

    // A property source for a client that listens, and one for a client that does not.
    let listener = deployment(
        &rig,
        &s,
        "tx-api",
        app("tx-api", listens()),
        "needs_notify_or_restart",
        &["tx-api/tx-api.yml"],
        3,
    )
    .await;
    let deaf = deployment(
        &rig,
        &s,
        "tx-ledger",
        app("tx-ledger", json!({})),
        "needs_notify_or_restart",
        &["tx-ledger/tx-ledger.yml"],
        3,
    )
    .await;
    // A resource file for a client that listens, one that does not (default lifetime 20 minutes), and one with a short lifetime
    // that has run out.
    let card = deployment(
        &rig,
        &s,
        "tx-card",
        app("tx-card", listens()),
        "live_within_ttl",
        &["tx-card/receipt.bmp"],
        3,
    )
    .await;
    let bank = deployment(
        &rig,
        &s,
        "tx-bank",
        app("tx-bank", json!({})),
        "live_within_ttl",
        &["tx-bank/receipt.bmp"],
        3,
    )
    .await;
    let quick = deployment(
        &rig,
        &s,
        "tx-quick",
        app("tx-quick", json!({ "CONFIG_CLIENT_CACHE_TTL": "2m" })),
        "live_within_ttl",
        &["tx-quick/receipt.bmp"],
        3,
    )
    .await;
    // A service whose folder was not notified, and one that needs the config-server restarted.
    let other = deployment(
        &rig,
        &s,
        "tx-other",
        app("tx-other", listens()),
        "needs_notify_or_restart",
        &["application-sit1.yml"],
        3,
    )
    .await;
    let fresh = deployment(
        &rig,
        &s,
        "tx-new",
        app("tx-new", listens()),
        "needs_config_server_restart",
        &["tx-new/folder/a.yml"],
        3,
    )
    .await;
    // And one with nothing pending.
    let quiet = deployment(&rig, &s, "tx-quiet", app("tx-quiet", listens()), "live", &[], 0).await;
    let before_other = row(&rig, &other).await;
    let before_fresh = row(&rig, &fresh).await;
    let before_quiet = row(&rig, &quiet).await;

    let out = rig
        .hub
        .notify(
            &ctx(&operator, "n-1"),
            notify_req(
                &s,
                &[
                    "tx-api/tx-api.yml",
                    "tx-ledger/tx-ledger.yml",
                    "tx-card/receipt.bmp",
                    "tx-bank/receipt.bmp",
                    "tx-quick/receipt.bmp",
                    "tx-new/a.yml",
                    "tx-quiet/q.yml",
                ],
            ),
        )
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");

    // A property source goes live for the client that listens; for one that does not, only a restart helps.
    assert_eq!(
        row(&rig, &listener).await,
        Row {
            state: "live".into(),
            pending: vec![],
            live_by_ms: None,
            pending_since_ms: None
        }
    );
    assert_eq!(
        row(&rig, &deaf).await,
        Row {
            state: "needs_notify_or_restart".into(),
            pending: vec!["tx-ledger/tx-ledger.yml".into()],
            live_by_ms: None,
            pending_since_ms: Some(now(3))
        }
    );
    // A resource file goes live for the client that listens; for the others it is live within the cache lifetime, by the time
    // the lifetime says; and live already when the lifetime has run out.
    assert_eq!(
        row(&rig, &card).await,
        Row {
            state: "live".into(),
            pending: vec![],
            live_by_ms: None,
            pending_since_ms: None
        }
    );
    assert_eq!(
        row(&rig, &bank).await,
        Row {
            state: "live_within_ttl".into(),
            pending: vec!["tx-bank/receipt.bmp".into()],
            live_by_ms: Some(now(3) + 20 * 60_000),
            pending_since_ms: Some(now(3))
        }
    );
    assert_eq!(
        row(&rig, &quick).await,
        Row {
            state: "live".into(),
            pending: vec![],
            live_by_ms: None,
            pending_since_ms: None
        }
    );
    // What the notification did not concern stays as the registry computed it.
    assert_eq!(row(&rig, &other).await, before_other, "another application");
    assert_eq!(
        row(&rig, &fresh).await,
        before_fresh,
        "a notification does not help a new folder"
    );
    assert_eq!(row(&rig, &quiet).await, before_quiet);

    // The swimlane remembers when its services were told (the registry's pickup states read it).
    let told: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM last_notified_at) * 1000)::bigint FROM swimlanes WHERE id = $1",
    )
    .bind(s.as_str())
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(told, Some(START));

    // The screens are told which services changed, and only those.
    let hinted: Vec<DomainEvent> = rig
        .bus
        .published()
        .await
        .into_iter()
        .filter(|e| matches!(e, DomainEvent::PickupChanged { .. }))
        .collect();
    assert_eq!(
        hinted,
        vec![DomainEvent::PickupChanged {
            swimlane: s.clone(),
            services: vec![
                svc("sit1", "tx-api"),
                svc("sit1", "tx-card"),
                svc("sit1", "tx-quick")
            ],
        }]
    );

    // A file in the root folder is read by every service: the one that was left alone goes live now.
    rig.hub
        .notify(&ctx(&operator, "n-2"), notify_req(&s, &["application.yml"]))
        .await
        .unwrap();
    assert_eq!(row(&rig, &other).await.state, "live");
    assert_eq!(
        row(&rig, &fresh).await,
        before_fresh,
        "still waits for the config-server"
    );
}

/// A change that lands while the notification is on its way may not be in what the config-server refreshes. The time the
/// services count as "last told" is when the hub began to tell them, not when it got round to recording it.
#[tokio::test]
async fn a_change_that_arrives_while_the_notification_is_sent_is_not_counted_as_notified() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    // The registry saw this change a minute after the notification began (a negative age).
    let late = deployment(
        &rig,
        &s,
        "tx-api",
        app("tx-api", listens()),
        "needs_notify_or_restart",
        &["tx-api/tx-api.yml"],
        -1,
    )
    .await;
    // The agent takes its time with the command; two minutes pass at the hub meanwhile, so the hub records the notification
    // two minutes after it began.
    rig.agent.set_delay(Duration::from_millis(300));
    let request = ctx(&operator, "n-1");
    let sent = rig.hub.notify(&request, notify_req(&s, &["tx-api/tx-api.yml"]));
    let time_passes = async {
        while !rig
            .agent
            .commands()
            .iter()
            .any(|c| matches!(c, domain::HubCommand::NotifyConfigServer { .. }))
        {
            tokio::task::yield_now().await;
        }
        rig.advance_secs(120);
    };
    let (out, ()) = tokio::join!(sent, time_passes);
    assert!(matches!(out.unwrap(), WriteOutcome::Applied { .. }));

    let after = row(&rig, &late).await;
    assert_eq!(
        (after.state.as_str(), after.pending.as_slice()),
        (
            "needs_notify_or_restart",
            ["tx-api/tx-api.yml".to_owned()].as_slice()
        ),
        "a change seen after the notification began is still waiting: {after:?}"
    );
}

/// A Deployment that declares no `SPRING_APPLICATION_NAME` is known to the config-server by its own name, as the module says and
/// as the statement already matches it. Its folder is its own: a notification for a file there reaches that Deployment and no
/// other, and `<name>.yml` in it is a property source.
#[tokio::test]
async fn a_deployment_without_a_declared_name_is_the_application_of_its_own_folder() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    // No declared name, and a client that does not listen: a property source needs a restart; a resource file would be live
    // within the cache lifetime.
    let plain = deployment(
        &rig,
        &s,
        "tx-plain",
        json!({}),
        "needs_notify_or_restart",
        &["tx-plain/tx-plain.yml"],
        3,
    )
    .await;
    // Another application, which listens, with a change of its own waiting.
    let other = deployment(
        &rig,
        &s,
        "tx-other",
        app("tx-other", listens()),
        "needs_notify_or_restart",
        &["tx-other/tx-other.yml"],
        3,
    )
    .await;
    let before_other = row(&rig, &other).await;
    let out = rig
        .hub
        .notify(&ctx(&operator, "n-1"), notify_req(&s, &["tx-plain/tx-plain.yml"]))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    let after = row(&rig, &plain).await;
    assert_eq!(
        (after.state.as_str(), after.pending.as_slice()),
        (
            "needs_notify_or_restart",
            ["tx-plain/tx-plain.yml".to_owned()].as_slice()
        ),
        "a property source for a client that does not listen: {after:?}"
    );
    assert_eq!(
        row(&rig, &other).await,
        before_other,
        "the notification was for another application's folder"
    );
}

/// `PENDING` reads a page of services. A swimlane with more services waiting than one page holds must not be left half updated.
#[tokio::test]
async fn a_notification_updates_every_waiting_service_not_only_the_first_page() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    // One page is 2,000 services (`MAX_PICKUP_SERVICES`); this is a page and a bit.
    let total: i64 = 2_150;
    sqlx::query(
        "INSERT INTO services (swimlane_id, namespace, name, env_values) \
         SELECT $1, 'sit1', 'svc' || g, $2 FROM generate_series(1, $3::int) g",
    )
    .bind(s.as_str())
    .bind(listens())
    .bind(total)
    .execute(&rig.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO service_state (service_id, pickup_state, pending_files, pending_since) \
         SELECT id, 'needs_notify_or_restart', $2, to_timestamp($3::bigint / 1000.0) FROM services WHERE swimlane_id = $1",
    )
    .bind(s.as_str())
    .bind(json!(["application.yml"]))
    .bind(START - 3 * 60_000)
    .execute(&rig.pool)
    .await
    .unwrap();

    let out = rig
        .hub
        .notify(&ctx(&operator, "n-1"), notify_req(&s, &["application.yml"]))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM service_state st JOIN services s ON s.id = st.service_id \
         WHERE s.swimlane_id = $1 AND st.pickup_state <> 'live'",
    )
    .bind(s.as_str())
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(waiting, 0, "services left stale after the notification");
}

#[tokio::test]
async fn a_notification_that_changes_no_pickup_state_hints_nothing() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    deployment(
        &rig,
        &s,
        "tx-ledger",
        app("tx-ledger", json!({})),
        "needs_notify_or_restart",
        &["tx-ledger/tx-ledger.yml"],
        3,
    )
    .await;
    rig.hub
        .notify(
            &ctx(&operator, "n-1"),
            notify_req(&s, &["tx-ledger/tx-ledger.yml"]),
        )
        .await
        .unwrap();
    assert!(
        !rig.bus
            .published()
            .await
            .iter()
            .any(|e| matches!(e, DomainEvent::PickupChanged { .. })),
        "the client does not listen, so nothing changed"
    );
    assert_eq!(rig.chain().await.len(), 1, "but the notification is audited");
}

// ------------------------------------------------------------------------------------------------------------- auto-notify

/// A config whose batch window is short enough for a test to wait for.
fn quick() -> WriteConfig {
    WriteConfig {
        auto_notify_window: Duration::from_millis(300),
        ..WriteConfig::default()
    }
}

async fn auto_on(rig: &Rig, s: &SwimlaneId) {
    sqlx::query("UPDATE swimlanes SET auto_notify = true WHERE id = $1")
        .bind(s.as_str())
        .execute(&rig.pool)
        .await
        .unwrap();
}

/// Run the batcher until the test says stop.
fn run(batcher: AutoNotifier) -> (tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(batcher.run(async move {
        let _ = stopped.await;
    }));
    (stop, task)
}

/// Wait until the agent has been told `n` times, or fail after a generous bound.
async fn until_notified(rig: &Rig, n: usize) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while rig.agent.notified_paths().len() < n {
            rig.agent.notified.notified().await;
        }
    })
    .await
    .expect("the agent was told");
}

/// Wait until `n` notifications are in the audit chain: the agent hears the command before the hub commits its record.
async fn until_audited(rig: &Rig, n: usize) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let count = rig
                .chain()
                .await
                .iter()
                .filter(|e| e.event.action == AuditAction::ConfigServerNotified)
                .count();
            if count >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the notification was recorded");
}

#[tokio::test]
async fn auto_notify_batches_a_multi_file_write_into_one_call() {
    // The default window: three writes made one after the other take far less than the two seconds it is open.
    let Some((rig, s)) = rig().await else { return };
    auto_on(&rig, &s).await;
    let operator = rig.user(1, Role::Operator).await;
    let batcher = rig
        .hub
        .auto_notifier()
        .expect("the first call hands out the batcher");
    assert!(
        rig.hub.auto_notifier().is_none(),
        "and there is one batcher for each hub"
    );
    let (stop, task) = run(batcher);

    // Three files written one after the other: one notification, with the three paths.
    for (n, (path, from, to)) in [
        ("application.yml", "a: 1\n", "a: 2\n"),
        ("tx-api/tx-api.yml", "k: 1\n", "k: 2\n"),
        ("tx-ledger/tx-ledger.yml", "l: 1\n", "l: 2\n"),
    ]
    .into_iter()
    .enumerate()
    {
        let out = rig
            .hub
            .edit(&ctx(&operator, &format!("e-{n}")), edit(&s, path, at(from), to))
            .await
            .unwrap();
        assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    }
    until_notified(&rig, 1).await;
    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 1, "one call for the whole write");
    let mut texts: Vec<&str> = sent[0].iter().map(domain::NfsPath::as_str).collect();
    texts.sort_unstable();
    assert_eq!(
        texts,
        ["application.yml", "tx-api/tx-api.yml", "tx-ledger/tx-ledger.yml"]
    );

    // The notification is audited once, as the Operator whose writes caused it, and says it was automatic.
    until_audited(&rig, 1).await;
    let chain = rig.chain().await;
    verify_chain(&chain).unwrap();
    let notified: Vec<_> = chain
        .iter()
        .filter(|e| e.event.action == AuditAction::ConfigServerNotified)
        .collect();
    assert_eq!(notified.len(), 1);
    assert_eq!(
        notified[0].event.actor,
        AuditActor::User {
            user: operator.id.clone()
        }
    );
    assert_eq!(notified[0].event.via, AuditVia::System);
    assert_eq!(chain.len(), 4, "three edits and one notification");

    // A write after the window is a batch of its own.
    rig.hub
        .edit(
            &ctx(&operator, "e-9"),
            edit(&s, "application.yml", at("a: 2\n"), "a: 3\n"),
        )
        .await
        .unwrap();
    until_notified(&rig, 2).await;
    until_audited(&rig, 2).await;
    assert_eq!(
        rig.agent.notified_paths()[1]
            .iter()
            .map(domain::NfsPath::as_str)
            .collect::<Vec<_>>(),
        ["application.yml"]
    );
    let _ = stop.send(());
    task.await.unwrap();
}

#[tokio::test]
async fn auto_notify_is_off_unless_the_swimlane_turns_it_on_and_only_for_an_operators_writes() {
    let Some((rig, s)) = rig_with(quick()).await else {
        return;
    };
    let operator = rig.user(1, Role::Operator).await;
    let editor = rig.user(2, Role::Editor).await;
    let (stop, task) = run(rig.hub.auto_notifier().unwrap());

    // Off by default.
    rig.hub
        .edit(
            &ctx(&operator, "e-1"),
            edit(&s, "application.yml", at("a: 1\n"), "a: 2\n"),
        )
        .await
        .unwrap();
    // On, but an Editor's write is not an Operator's request to call the config-server (Q37).
    auto_on(&rig, &s).await;
    rig.hub
        .edit(
            &ctx(&editor, "e-2"),
            edit(&s, "tx-api/tx-api.yml", at("k: 1\n"), "k: 2\n"),
        )
        .await
        .unwrap();
    // The Operator's next write is the first that notifies, and it carries only its own path.
    rig.hub
        .edit(
            &ctx(&operator, "e-3"),
            edit(&s, "tx-ledger/tx-ledger.yml", at("l: 1\n"), "l: 2\n"),
        )
        .await
        .unwrap();
    until_notified(&rig, 1).await;
    let _ = stop.send(());
    task.await.unwrap();

    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
        ["tx-ledger/tx-ledger.yml"]
    );
}

#[tokio::test]
async fn auto_notify_never_runs_for_a_user_who_is_no_longer_an_operator() {
    let Some((rig, s)) = rig_with(quick()).await else {
        return;
    };
    auto_on(&rig, &s).await;
    let operator = rig.user(1, Role::Operator).await;
    let (stop, task) = run(rig.hub.auto_notifier().unwrap());
    rig.hub
        .edit(
            &ctx(&operator, "e-1"),
            edit(&s, "application.yml", at("a: 1\n"), "a: 2\n"),
        )
        .await
        .unwrap();
    // Demoted before the window closes: the role is read when the notification is made, not when the write was.
    let mut demoted = operator.clone();
    demoted.role = Some(Role::Viewer);
    assert!(rig.users.insert(demoted));
    // A write that follows from an Admin shows the batcher is alive and has dealt with the first batch.
    let admin = rig.user(2, Role::Admin).await;
    rig.hub
        .edit(
            &ctx(&admin, "e-2"),
            edit(&s, "tx-api/tx-api.yml", at("k: 1\n"), "k: 2\n"),
        )
        .await
        .unwrap();
    until_notified(&rig, 1).await;
    let _ = stop.send(());
    task.await.unwrap();
    let sent = rig.agent.notified_paths();
    assert_eq!(sent.len(), 1, "the demoted operator's write was not announced");
    assert_eq!(
        sent[0].iter().map(domain::NfsPath::as_str).collect::<Vec<_>>(),
        ["tx-api/tx-api.yml"]
    );
}

#[tokio::test]
async fn a_full_queue_drops_the_path_and_never_fails_the_write() {
    let Some((rig, s)) = rig_with(WriteConfig {
        auto_notify_queue: 1,
        ..WriteConfig::default()
    })
    .await
    else {
        return;
    };
    auto_on(&rig, &s).await;
    let operator = rig.user(1, Role::Operator).await;
    let edit_of = |n: u32, path: &str, from: &str, to: &str| {
        let hub = rig.hub.clone();
        let (c, r) = (ctx(&operator, &format!("e-{n}")), edit(&s, path, at(from), to));
        async move { hub.edit(&c, r).await.unwrap() }
    };

    // No batcher has been asked for: a write announces nothing and costs nothing, whatever the switch says.
    let out = edit_of(1, "application.yml", "a: 1\n", "a: 2\n").await;
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.hub.auto_notify_dropped(), 0);

    // With a batcher that does not run, the queue holds one path; the next is dropped and counted, and its write is applied.
    let _idle = rig.hub.auto_notifier().expect("a batcher");
    let out = edit_of(2, "tx-api/tx-api.yml", "k: 1\n", "k: 2\n").await;
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.hub.auto_notify_dropped(), 0, "the first fits");
    let out = edit_of(3, "tx-ledger/tx-ledger.yml", "l: 1\n", "l: 2\n").await;
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.hub.auto_notify_dropped(), 1, "the second does not");
    assert_eq!(rig.agent.notified_paths().len(), 0);
    assert_eq!(rig.chain().await.len(), 3, "three edits, and no notification");
}

// ------------------------------------------------------------------------------------------------------ config-server restart

async fn c11(rig: &Rig, s: &SwimlaneId, resolved: bool) {
    sqlx::query(
        "INSERT INTO findings (swimlane_id, kind, severity, message, fingerprint, resolved_at) \
         VALUES ($1, 'config_server_restart_required', 'high', 'a folder is new', $2, \
                 CASE WHEN $3 THEN now() END)",
    )
    .bind(s.as_str())
    .bind(format!("fp-{resolved}"))
    .bind(resolved)
    .execute(&rig.pool)
    .await
    .unwrap();
}

fn restart_req(s: &SwimlaneId, name: &str) -> RestartRequest {
    RestartRequest {
        swimlane: s.clone(),
        service: svc("sit1", name),
    }
}

#[tokio::test]
async fn config_server_restart_is_offered_when_c11_is_open_and_names_affected_services() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    for name in ["csp-configuration-server", "payments", "ledger", "gone"] {
        rig.service(&s, "sit1", name, None).await;
    }
    sqlx::query("UPDATE services SET removed_at = now() WHERE name = 'gone'")
        .execute(&rig.pool)
        .await
        .unwrap();
    // Another swimlane's open finding is not this swimlane's.
    let other = rig.swimlane("sit2", true).await;
    c11(&rig, &other, false).await;
    let ask = |key: &'static str| {
        let (hub, s, operator) = (rig.hub.clone(), s.clone(), operator.clone());
        async move { hub.config_server_restart_offer(&ctx(&operator, key), &s).await }
    };

    // C11 is not open: no offer.
    assert_eq!(ask("o-1").await, Ok(None));
    // A resolved finding is not open.
    c11(&rig, &s, true).await;
    assert_eq!(ask("o-2").await, Ok(None));

    // Open: the offer names the config-server and, for the confirmation, the services that cannot be served while it is down.
    c11(&rig, &s, false).await;
    let offer = ask("o-3").await.unwrap().expect("offered");
    assert_eq!(offer.service, svc("sit1", "csp-configuration-server"));
    assert_eq!(
        offer.affected,
        vec![svc("sit1", "ledger"), svc("sit1", "payments")],
        "every other service of the swimlane, and not the removed one"
    );

    // The restart is the ordinary restart of that service: Operator, one lease, one audit event.
    let before = rig.chain().await.len();
    let out = rig
        .hub
        .restart(
            &ctx(&operator, "r-1"),
            restart_req(&s, "csp-configuration-server"),
        )
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied { .. }), "{out:?}");
    assert_eq!(rig.agent.restarts_applied(), 1);
    let chain = rig.chain().await;
    assert_eq!(chain.len(), before + 1);
    let e = &chain.last().unwrap().event;
    assert_eq!(e.action, AuditAction::ServiceRestarted);
    assert_eq!(
        e.diff.as_deref(),
        Some("restart deployment sit1/csp-configuration-server")
    );
}

/// The services a config-server restart takes away are listed up to `MAX_RESTART_OFFER`; more than that and the offer says it is
/// not the whole of them. The config-server's own row is not one of the services, and does not make the list look full.
#[tokio::test]
async fn a_config_server_offer_that_leaves_services_out_says_so() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    c11(&rig, &s, false).await;
    rig.service(&s, "sit1", "csp-configuration-server", None).await;
    let max = i32::try_from(hub_writes::MAX_RESTART_OFFER).unwrap();
    let others = |n: i32| {
        let (pool, s) = (rig.pool.clone(), s.clone());
        async move {
            sqlx::query("DELETE FROM services WHERE swimlane_id = $1 AND name <> 'csp-configuration-server'")
                .bind(s.as_str())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO services (swimlane_id, namespace, name, env_values) \
                 SELECT $1, 'sit1', 'svc' || lpad(g::text, 5, '0'), '{}' FROM generate_series(1, $2) g",
            )
            .bind(s.as_str())
            .bind(n)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    others(max).await;
    let offer = rig
        .hub
        .config_server_restart_offer(&ctx(&operator, "o-1"), &s)
        .await
        .unwrap()
        .expect("offered");
    assert_eq!(
        (offer.affected.len(), offer.truncated),
        (hub_writes::MAX_RESTART_OFFER, false)
    );
    others(max + 1).await;
    let offer = rig
        .hub
        .config_server_restart_offer(&ctx(&operator, "o-2"), &s)
        .await
        .unwrap()
        .expect("offered");
    assert_eq!(
        (offer.affected.len(), offer.truncated),
        (hub_writes::MAX_RESTART_OFFER, true)
    );
}

#[tokio::test]
async fn config_server_restart_is_not_offered_when_the_swimlane_has_no_single_config_server() {
    let Some((rig, s)) = rig().await else { return };
    let operator = rig.user(1, Role::Operator).await;
    c11(&rig, &s, false).await;
    let ask = |key: &'static str| {
        let (hub, s, operator) = (rig.hub.clone(), s.clone(), operator.clone());
        async move { hub.config_server_restart_offer(&ctx(&operator, key), &s).await }
    };
    // The cluster report lists no Deployment of that name.
    rig.service(&s, "sit1", "payments", None).await;
    assert_eq!(ask("o-1").await, Ok(None));
    // Two of that name in two namespaces: the hub does not guess which one.
    rig.service(&s, "sit1", "csp-configuration-server", None).await;
    rig.service(&s, "sit2", "csp-configuration-server", None).await;
    assert_eq!(ask("o-2").await, Ok(None));
    // A swimlane that does not exist has none.
    let nowhere = SwimlaneId::parse("nowhere").unwrap();
    assert_eq!(
        rig.hub
            .config_server_restart_offer(&ctx(&operator, "o-3"), &nowhere)
            .await,
        Ok(None)
    );
    // And a configuration that names another Deployment is followed.
    let config = WriteConfig {
        config_server_deployment: "payments".to_owned(),
        ..WriteConfig::default()
    };
    let hub = rig.hub_with(config, None);
    let offer = hub
        .config_server_restart_offer(&ctx(&operator, "o-4"), &s)
        .await
        .unwrap()
        .expect("offered");
    assert_eq!(offer.service, svc("sit1", "payments"));
}

#[tokio::test]
async fn a_config_server_restart_by_an_approval_user_is_a_proposal() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "csp-configuration-server", None).await;
    let careful = rig.approval_user(1, Role::Operator).await;
    let out = rig
        .hub
        .restart(&ctx(&careful, "r-1"), restart_req(&s, "csp-configuration-server"))
        .await
        .unwrap();
    assert!(matches!(out, WriteOutcome::ProposalCreated { .. }), "{out:?}");
    assert_eq!(rig.agent.restarts_applied(), 0);
}

#[tokio::test]
async fn restart_and_notify_need_operator() {
    let Some((rig, s)) = rig().await else { return };
    rig.service(&s, "sit1", "csp-configuration-server", None).await;
    c11(&rig, &s, false).await;
    let viewer = rig.user(1, Role::Viewer).await;
    let editor = rig.user(2, Role::Editor).await;
    let disabled: User = rig
        .user_with_status(3, Some(Role::Admin), UserStatus::Disabled)
        .await;
    let roleless = rig.user_with_status(4, None, UserStatus::Active).await;

    for (who, name) in [
        (&viewer, "viewer"),
        (&editor, "editor"),
        (&disabled, "disabled"),
        (&roleless, "no role"),
    ] {
        let c = ctx(who, "k-1");
        assert_eq!(
            rig.hub
                .restart(&c, restart_req(&s, "csp-configuration-server"))
                .await,
            Err(WriteError::Forbidden),
            "restart as {name}"
        );
        assert_eq!(
            rig.hub.notify(&c, notify_req(&s, &["application.yml"])).await,
            Err(WriteError::Forbidden),
            "notify as {name}"
        );
        assert_eq!(
            rig.hub.config_server_restart_offer(&c, &s).await,
            Err(WriteError::Forbidden),
            "the config-server restart offer as {name}"
        );
        assert_eq!(
            rig.hub.restart_offer(&c, &s, &nfs("application.yml")).await,
            Err(WriteError::Forbidden),
            "the restart-all offer as {name}"
        );
    }
    assert_eq!(rig.agent.commands().len(), 0, "nothing was sent");
    assert_eq!(rig.count("idempotency_keys").await, 0, "nothing was claimed");
    assert_eq!(rig.chain().await.len(), 0, "nothing was recorded");
}
