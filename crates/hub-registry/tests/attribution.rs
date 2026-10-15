//! T7 (D72, D73, A19, P14): a local edit on the NFS VM becomes `sentinel_login` (high, naming the user), a change in a sync window stays the
//! Job's, a change nothing explains becomes `nfs_client` after the grace, a late record upgrades an earlier `unknown`, an attribution only
//! goes up, and every upgrade is one audit event and one `AttributionUpdated` written in the transaction that upgrades.
//!
//! Time is the rig's `ManualClock` (`T0`): the VM's records and the agent's scans carry the times the tests give them, and the leased job
//! runs when a test calls it. Real Postgres, as `lanekeeper_app`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::AuditOperation;
use hub_registry::Clock;
use hub_registry::ManualClock;
use hub_registry::attribution::statements::UPGRADE_SQL;
use ports::{AuditError, BusError};
use support::rig::Rig;
use support::sentinel::{Agent, batch, by, failed, op, record, user};

const T0: i64 = ManualClock::T0_MS;
const S: i64 = 1_000;

/// A rig with a sentinel for its swimlane, `ada@example.com` as an active user, and the swimlane's first inventory of `files`.
async fn world<'a>(rig: &'a Rig, files: &[(&str, &str)]) -> Agent<'a> {
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    Agent::start(rig, files).await
}

fn now(rig: &Rig) -> i64 {
    rig.clock.now().unix_millis()
}

// ================================================================================================ the verify list

#[tokio::test]
async fn a_local_vm_edit_is_sentinel_login_high_and_names_the_user() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;

    // Ada edits the file on the VM at T0 + 10 s, with `sudo vim`. The record reaches the hub before the agent's scan: there is no change
    // to explain yet.
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    assert_eq!(rig.upgrade_audits().await.len(), 0, "nothing to attribute yet");

    // The agent walks the tree and reports the new version it saw at T0 + 25 s; the hub applies it at T0 + 26 s.
    rig.clock.advance_secs(26);
    let applied = now(&rig);
    agent.change("svc/a.yml", "v: 1", T0 + 25 * S).await;
    let before = rig.attribution("svc/a.yml", 1).await;
    assert!(
        before.is("unknown", "low"),
        "first seen it is unknown: {before:?}"
    );

    // The job's next run (every 5 s) finds the record that was stored before the observation.
    rig.clock.advance_secs(5);
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(report.sentinel_login, 1);

    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(a.is("sentinel_login", "high"), "{a:?}");
    assert_eq!(
        a.user(),
        Some(user(1).to_string()),
        "the OS Login name ada_example_com is Ada's email: the actor is the user, not the login"
    );
    assert_eq!(
        a.evidence["executable"], "/usr/bin/vim",
        "the evidence names the executable"
    );
    assert_eq!(
        a.evidence["login_identity"], "ada_example_com",
        "and the original login identity, which sudo did not hide"
    );
    assert_eq!(
        a.evidence["observed_at"],
        T0 + 10 * S,
        "and the time of the record"
    );
    // P14: within 30 seconds of the change being observed.
    assert!(
        now(&rig) - applied <= 30 * S,
        "attributed {} ms after the hub applied it",
        now(&rig) - applied
    );
    // The inventory's observation is not a change and was not touched.
    assert!(rig.attribution("svc/a.yml", 0).await.is("unknown", "low"));
}

#[tokio::test]
async fn a_local_vm_edit_is_attributed_at_once_when_its_record_comes_after_the_scan() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.clock.advance_secs(26);
    agent.change("svc/a.yml", "v: 1", T0 + 25 * S).await;
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));

    // The record of an edit made at T0 + 10 s arrives late (a batch flushed after the scan): the sink attributes in its own transaction.
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(a.is("sentinel_login", "high"), "{a:?}");
    assert!(a.user().is_some());
}

#[tokio::test]
async fn sentinel_record_attributes_sentinel_login_within_30_seconds() {
    // P14 end to end, with the job running every `attribution_interval` (5 s) on the registry's clock: the change is attributed no later
    // than 30 s after the hub applied it, whichever of the record and the scan comes first.
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0"), ("svc/b.yml", "v: 0")]).await;
    let tick = i64::try_from(
        hub_registry::RegistryConfig::default()
            .attribution_interval
            .as_secs(),
    )
    .unwrap();
    assert!(tick <= 10);
    rig.sentinel_hello().await.unwrap();

    // a: the record first. b: the scan first, and its record comes in the sentinel's next batch.
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(20);
    let (applied_a, applied_b) = (now(&rig), now(&rig));
    agent.change("svc/a.yml", "v: 1", T0 + 19 * S).await;
    agent.change("svc/b.yml", "v: 1", T0 + 19 * S).await;

    let (mut done_a, mut done_b) = (None, None);
    for step in 1..=6 {
        rig.clock.advance_secs(tick);
        if step == 2 {
            rig.records(batch(2, vec![record("svc/b.yml", T0 + 12 * S)]))
                .await
                .unwrap();
        }
        rig.sentinel.grace().run_once().await.unwrap();
        if done_a.is_none() && rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high") {
            done_a = Some(now(&rig) - applied_a);
        }
        if done_b.is_none() && rig.attribution("svc/b.yml", 1).await.is("sentinel_login", "high") {
            done_b = Some(now(&rig) - applied_b);
        }
    }
    let (a, b) = (done_a.expect("a is attributed"), done_b.expect("b is attributed"));
    assert_eq!(
        a,
        tick * S,
        "a record that was here first waits for the next run of the job and no more"
    );
    assert_eq!(
        b,
        2 * tick * S,
        "a record that comes after the scan is attributed by the batch that carries it"
    );
    assert!(a <= 30 * S && b <= 30 * S, "P14: {a} ms and {b} ms");
}

#[tokio::test]
async fn a_sync_window_change_is_sync_job() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    // A sync Job ran from T0 - 1 s: the window is open when the agent sees the new version.
    sqlx::query(
        "INSERT INTO sync_windows (swimlane_id, job_name, job_uid, opened_at) VALUES ($1, 'sync-sit1', 'uid-9', \
         (timestamptz 'epoch' + ($2::bigint::text || ' milliseconds')::interval))",
    )
    .bind(rig.swimlane.as_str())
    .bind(T0 - S)
    .execute(&rig.pool)
    .await
    .unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(a.is("sync_job", "high"), "{a:?}");
    assert_eq!(a.actor.as_ref().unwrap()["kind"], "job");

    // A sentinel record on the same path within the minute (someone on the VM touched it too) does not take it from the Job: the gate is
    // confidence, and an equal one never replaces. The record is stored and visible, and nothing is audited.
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 4 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(40);
    rig.sentinel.grace().run_once().await.unwrap();
    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(
        a.is("sync_job", "high"),
        "stays the Job's, and never an NFS client's: {a:?}"
    );
    assert_eq!(rig.stored_records().await.len(), 1);
    assert!(rig.upgrade_audits().await.is_empty());
    assert!(rig.attribution_updates().await.is_empty());
}

#[tokio::test]
async fn an_nfs_write_outside_any_job_is_nfs_client() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    let applied = now(&rig);

    // Within the grace the sentinel's record may still be on its way: nothing is decided.
    rig.clock.advance_secs(20);
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(report.nfs_client, 0);
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));

    // The sentinel has been seen since the change (its next batch: another file was written on the VM), and after the grace nothing
    // explains the change: an NFS client wrote it.
    rig.records(batch(1, vec![record("svc/other.yml", T0 + 20 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(11);
    assert!(now(&rig) - applied > 30 * S);
    let report = rig.sentinel.grace().run_once().await.unwrap();
    assert_eq!(report.nfs_client, 1);
    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(a.is("nfs_client", "medium"), "{a:?}");
    assert_eq!(a.actor, None, "an NFS client is not a user");
    assert_eq!(a.evidence["observed_at"], T0 + 5 * S);
    assert_eq!(rig.upgrade_audits().await.len(), 1);
    assert_eq!(rig.attribution_updates().await, vec!["svc/a.yml"]);
}

#[tokio::test]
async fn a_late_sentinel_record_upgrades_an_earlier_unknown() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0"), ("svc/b.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent
        .change_all(&[("svc/a.yml", "v: 1"), ("svc/b.yml", "v: 1")], T0 + 5 * S)
        .await;
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));

    // a: the record arrives within the grace. b: the grace passes first, so b is an NFS client's until its record comes.
    rig.records(batch(
        1,
        vec![record("svc/a.yml", T0 + 3 * S), record("svc/c.yml", T0 + 4 * S)],
    ))
    .await
    .unwrap();
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"),
        "upgraded by the sink"
    );
    assert!(rig.attribution("svc/b.yml", 1).await.is("unknown", "low"));

    rig.clock.advance_secs(40);
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(rig.attribution("svc/b.yml", 1).await.is("nfs_client", "medium"));

    // The late record of b's edit: nfs_client is below high, so it is upgraded.
    rig.records(batch(2, vec![record("svc/b.yml", T0 + 2 * S)]))
        .await
        .unwrap();
    let b = rig.attribution("svc/b.yml", 1).await;
    assert!(
        b.is("sentinel_login", "high"),
        "a late record upgrades an NFS client's: {b:?}"
    );
    assert!(b.user().is_some());
}

#[tokio::test]
async fn an_upgrade_is_never_a_downgrade() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(
        &rig,
        &[
            ("svc/a.yml", "v: 0"),
            ("svc/b.yml", "v: 0"),
            ("svc/c.yml", "v: 0"),
        ],
    )
    .await;
    rig.add_user(2, "bob@example.com", "active").await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent
        .change_all(
            &[
                ("svc/a.yml", "v: 1"),
                ("svc/b.yml", "v: 1"),
                ("svc/c.yml", "v: 1"),
            ],
            T0 + 5 * S,
        )
        .await;
    // a is a tool write (certain), b is a sync Job's (high), c is Ada's already (high).
    rig.attribute_by_hand("svc/a.yml", 1, "tool_write", "certain")
        .await;
    rig.attribute_by_hand("svc/b.yml", 1, "sync_job", "high").await;
    rig.records(batch(1, vec![record("svc/c.yml", T0 + 4 * S)]))
        .await
        .unwrap();
    assert!(rig.attribution("svc/c.yml", 1).await.is("sentinel_login", "high"));
    let ada_on_c = rig.attribution("svc/c.yml", 1).await;
    let audits = rig.upgrade_audits().await.len();

    // Records for all three, by Bob this time, and the job after the grace.
    let bob = |p: &str| by(record(p, T0 + 5 * S), "bob_example_com", "bob_example_com");
    rig.records(batch(
        2,
        vec![bob("svc/a.yml"), bob("svc/b.yml"), bob("svc/c.yml")],
    ))
    .await
    .unwrap();
    rig.clock.advance_secs(60);
    rig.sentinel.grace().run_once().await.unwrap();

    assert!(
        rig.attribution("svc/a.yml", 1).await.is("tool_write", "certain"),
        "a tool write is final"
    );
    assert!(
        rig.attribution("svc/b.yml", 1).await.is("sync_job", "high"),
        "equal never replaces"
    );
    assert_eq!(
        rig.attribution("svc/c.yml", 1).await,
        ada_on_c,
        "the first sentinel answer stands, user and evidence"
    );
    assert_eq!(
        rig.upgrade_audits().await.len(),
        audits,
        "nothing was upgraded, so nothing was audited"
    );
}

#[tokio::test]
async fn the_update_statement_refuses_to_touch_a_row_that_is_not_below_the_new_confidence() {
    // The race that the chosen set cannot see: an observation reaches `high` between the look and the write. The statement re-tests it
    // on the row it writes, so the loser changes nothing.
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    let id: i64 = sqlx::query_scalar("SELECT max(id) FROM file_observations WHERE path = 'svc/a.yml'")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    let write = |confidences: Vec<String>| {
        let pool = rig.pool.clone();
        let lane = rig.swimlane.as_str().to_owned();
        async move {
            sqlx::query_scalar::<_, i64>(UPGRADE_SQL)
                .bind(lane)
                .bind(vec![id])
                .bind(vec!["sentinel_login"])
                .bind(vec!["high"])
                .bind(vec![None::<String>])
                .bind(vec!["{}".to_owned()])
                .bind(confidences)
                .fetch_all(&pool)
                .await
                .unwrap()
        }
    };
    rig.attribute_by_hand("svc/a.yml", 1, "sync_job", "high").await;
    assert!(
        write(vec!["low".into(), "medium".into()]).await.is_empty(),
        "high is not below high"
    );
    rig.attribute_by_hand("svc/a.yml", 1, "tool_write", "certain")
        .await;
    assert!(
        write(vec!["low".into(), "medium".into()]).await.is_empty(),
        "certain is not below high"
    );
    rig.attribute_by_hand("svc/a.yml", 1, "nfs_client", "medium")
        .await;
    assert_eq!(
        write(vec!["low".into(), "medium".into()]).await,
        vec![id],
        "medium is below high"
    );
    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
}

#[tokio::test]
async fn an_upgrade_writes_one_audit_event_and_one_attribution_updated() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(26);
    agent.change("svc/a.yml", "v: 1", T0 + 25 * S).await;
    let audits_before = rig.count("SELECT count(*) FROM audit_events").await;

    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    assert_eq!(
        rig.upgrade_audits().await,
        vec![(Some("svc/a.yml".to_owned()), Some("sentinel_login".to_owned()))],
        "one audit event, with the attribution it recorded"
    );
    assert_eq!(
        rig.attribution_updates().await,
        vec!["svc/a.yml"],
        "and one event for the clients"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM audit_events").await - audits_before,
        1,
        "nothing else was audited (rule 8: one event for one state change)"
    );
    let who: (String, Option<String>) =
        sqlx::query_as("SELECT actor_kind, via FROM audit_events WHERE action = 'attribution_upgraded'")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(who, ("sentinel".to_owned(), Some("system".to_owned())));

    // The resend of the batch, the job, and a second batch that names the same change change nothing and write nothing.
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(60);
    rig.sentinel.grace().run_once().await.unwrap();
    rig.records(batch(2, vec![record("svc/a.yml", T0 + 11 * S)]))
        .await
        .unwrap();
    assert_eq!(rig.upgrade_audits().await.len(), 1);
    assert_eq!(rig.attribution_updates().await.len(), 1);
}

#[tokio::test]
async fn an_upgrade_and_its_events_are_one_transaction_and_the_batch_is_acknowledged_only_after_it_commits() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(26);
    agent.change("svc/a.yml", "v: 1", T0 + 25 * S).await;
    let batch1 = || batch(1, vec![record("svc/a.yml", T0 + 10 * S)]);

    // The audit log fails: nothing of the batch is kept, and the sentinel gets no acknowledgement, so it will send it again.
    rig.audit.fail_with(Some(AuditError::Unavailable));
    assert_eq!(rig.records(batch1()).await, Err(ports::SinkError::Unavailable));
    assert_eq!(
        rig.stored_records().await.len(),
        0,
        "the records were rolled back with the upgrade"
    );
    assert_eq!(
        rig.last_acked().await,
        0,
        "last_acked_seq moves only with the commit"
    );
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
    assert_eq!(rig.attribution_updates().await.len(), 0);
    rig.audit.fail_with(None);

    // The outbox fails: the same.
    rig.bus.fail_with(Some(BusError::Unavailable));
    assert_eq!(rig.records(batch1()).await, Err(ports::SinkError::Unavailable));
    assert_eq!((rig.stored_records().await.len(), rig.last_acked().await), (0, 0));
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
    rig.bus.fail_with(None);

    // The resend, once both work, is applied once.
    assert_eq!(rig.records(batch1()).await.unwrap().get(), 1);
    assert_eq!((rig.stored_records().await.len(), rig.last_acked().await), (1, 1));
    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
    assert_eq!(rig.upgrade_audits().await.len(), 1);
    assert_eq!(rig.attribution_updates().await.len(), 1);
}

// ================================================================================================ the rules of a match

#[tokio::test]
async fn a_record_explains_a_change_within_sixty_seconds_inclusive_and_not_beyond() {
    let Some(rig) = Rig::new().await else { return };
    let files: Vec<String> = (0..6).map(|i| format!("svc/f{i}.yml")).collect();
    let initial: Vec<(&str, &str)> = files.iter().map(|f| (f.as_str(), "v: 0")).collect();
    let mut agent = world(&rig, &initial).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(200);
    let seen = T0 + 100 * S;
    let changes: Vec<(&str, &str)> = files.iter().map(|f| (f.as_str(), "v: 1")).collect();
    agent.change_all(&changes, seen).await;
    // f0: 60 s before, f1: 60.001 s before, f2: 60 s after, f3: 60.001 s after, f4: at the same moment, f5: 59.999 s before.
    let at = [
        seen - 60 * S,
        seen - 60 * S - 1,
        seen + 60 * S,
        seen + 60 * S + 1,
        seen,
        seen - 60 * S + 1,
    ];
    let records = files.iter().zip(at).map(|(f, t)| record(f, t)).collect();
    rig.records(batch(1, records)).await.unwrap();
    let got: Vec<bool> = {
        let mut v = Vec::new();
        for f in &files {
            v.push(rig.attribution(f, 1).await.is("sentinel_login", "high"));
        }
        v
    };
    assert_eq!(got, vec![true, false, true, false, true, true]);
}

#[tokio::test]
async fn the_record_nearest_in_time_wins_and_a_tie_goes_to_the_earlier() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0"), ("svc/b.yml", "v: 0")]).await;
    rig.add_user(2, "bob@example.com", "active").await;
    rig.add_user(3, "eve@example.com", "active").await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(100);
    let seen = T0 + 50 * S;
    agent
        .change_all(&[("svc/a.yml", "v: 1"), ("svc/b.yml", "v: 1")], seen)
        .await;
    let ada = |p: &str, t| by(record(p, t), "ada_example_com", "root");
    let bob = |p: &str, t| by(record(p, t), "bob_example_com", "root");
    let eve = |p: &str, t| by(record(p, t), "eve_example_com", "root");
    // a: Ada 30 s before, Bob 5 s before, Eve 20 s after: Bob. b: Ada 10 s before, Bob 10 s after: the earlier, Ada.
    rig.records(batch(
        1,
        vec![
            ada("svc/a.yml", seen - 30 * S),
            bob("svc/a.yml", seen - 5 * S),
            eve("svc/a.yml", seen + 20 * S),
            bob("svc/b.yml", seen + 10 * S),
            ada("svc/b.yml", seen - 10 * S),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.attribution("svc/a.yml", 1).await.user(),
        Some(user(2).to_string())
    );
    assert_eq!(
        rig.attribution("svc/b.yml", 1).await.user(),
        Some(user(1).to_string())
    );
    assert_eq!(
        rig.upgrade_audits().await.len(),
        2,
        "one event for each observation, not for each record"
    );
}

#[tokio::test]
async fn the_operation_has_to_fit_the_change() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(
        &rig,
        &[
            ("svc/a.yml", "v: 0"),
            ("svc/b.yml", "v: 0"),
            ("svc/c.yml", "v: 0"),
            ("svc/keep.yml", "v: 0"),
        ],
    )
    .await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(30);
    // a is rewritten, b is rewritten, c is deleted (and keep is the file whose time gives the deletion its time).
    agent
        .change_all(&[("svc/a.yml", "v: 1"), ("svc/b.yml", "v: 1")], T0 + 10 * S)
        .await;
    agent
        .delete_seen_at("svc/c.yml", ("svc/keep.yml", "v: 0"), T0 + 10 * S)
        .await;
    // A `delete` record does not explain a rewrite, a `write` record does not explain a deletion, a `rename` explains either, and a
    // `create` explains a version.
    rig.records(batch(
        1,
        vec![
            op(record("svc/a.yml", T0 + 9 * S), AuditOperation::Delete),
            op(record("svc/b.yml", T0 + 9 * S), AuditOperation::Create),
            record("svc/c.yml", T0 + 9 * S),
        ],
    ))
    .await
    .unwrap();
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("unknown", "low"),
        "a delete does not explain a rewrite"
    );
    assert!(
        rig.attribution("svc/b.yml", 1).await.is("sentinel_login", "high"),
        "a create explains a version"
    );
    assert!(
        rig.attribution("svc/c.yml", 1).await.is("unknown", "low"),
        "a write does not explain a deletion"
    );
    rig.records(batch(
        2,
        vec![op(record("svc/c.yml", T0 + 9 * S), AuditOperation::Delete)],
    ))
    .await
    .unwrap();
    let c = rig.attribution("svc/c.yml", 1).await;
    assert!(
        c.is("sentinel_login", "high"),
        "a delete explains a deletion: {c:?}"
    );
    let updates = rig.attribution_updates().await;
    assert!(updates.contains(&"svc/c.yml".to_owned()), "{updates:?}");
    // A rename explains a rewrite and a deletion alike.
    rig.records(batch(
        3,
        vec![op(record("svc/a.yml", T0 + 9 * S), AuditOperation::Rename)],
    ))
    .await
    .unwrap();
    assert!(
        rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"),
        "a rename explains a rewrite"
    );
}

#[tokio::test]
async fn a_deletion_event_names_the_version_that_went_away() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/c.yml", "v: 0"), ("svc/keep.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(30);
    agent
        .delete_seen_at("svc/c.yml", ("svc/keep.yml", "v: 0"), T0 + 10 * S)
        .await;
    rig.records(batch(
        1,
        vec![op(record("svc/c.yml", T0 + 9 * S), AuditOperation::Delete)],
    ))
    .await
    .unwrap();
    let (before, after): (Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT hash_before::bytea, hash_after::bytea FROM audit_events WHERE action = 'attribution_upgraded'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        before.as_deref(),
        Some(ports::content_hash(b"v: 0").as_bytes().as_slice())
    );
    assert_eq!(after, None, "a deletion has nothing after it");
    assert_eq!(rig.attribution_updates().await, vec!["svc/c.yml"]);
}

#[tokio::test]
async fn a_record_that_failed_names_nobody_and_stops_an_nfs_client_from_being_called_one_by_nobody() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    // A write that was refused (permission denied) changed nothing: it explains nothing.
    rig.records(batch(1, vec![failed(record("svc/a.yml", T0 + 4 * S))]))
        .await
        .unwrap();
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
    assert_eq!(
        rig.stored_records().await.len(),
        1,
        "it is stored: it is evidence of an attempt"
    );
    // After the grace, and with the sentinel seen since, the change is an NFS client's: the failed record does not count as a local write.
    rig.clock.advance_secs(40);
    rig.records(batch(2, vec![record("svc/other.yml", T0 + 45 * S)]))
        .await
        .unwrap();
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(rig.attribution("svc/a.yml", 1).await.is("nfs_client", "medium"));
}

#[tokio::test]
async fn a_record_that_names_no_one_neither_attributes_nor_makes_an_nfs_client() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    // A local write whose login and effective user are both empty: somebody wrote it on the VM, and nobody can be named. It is not an
    // NFS client's either, so the change stays unknown.
    let mut r = record("svc/a.yml", T0 + 4 * S);
    r.login_user = support::sentinel::text("");
    r.effective_user = support::sentinel::text("");
    rig.records(batch(1, vec![r])).await.unwrap();
    rig.clock.advance_secs(40);
    rig.records(batch(2, vec![record("svc/other.yml", T0 + 45 * S)]))
        .await
        .unwrap();
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(rig.attribution("svc/a.yml", 1).await.is("unknown", "low"));
}

#[tokio::test]
async fn the_effective_user_stands_in_when_there_is_no_login_identity() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(6);
    agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await;
    let mut r = by(record("svc/a.yml", T0 + 4 * S), "", "backup_job");
    r.exe = support::sentinel::text("");
    r.comm = support::sentinel::text("rsync");
    rig.records(batch(1, vec![r])).await.unwrap();
    let a = rig.attribution("svc/a.yml", 1).await;
    assert!(a.is("sentinel_login", "high"), "{a:?}");
    assert_eq!(a.actor.as_ref().unwrap()["kind"], "os_login");
    assert_eq!(a.actor.as_ref().unwrap()["login"], "backup_job");
    assert_eq!(
        a.evidence["executable"], "rsync",
        "the command name stands in for an executable that was not given"
    );
}

#[tokio::test]
async fn the_first_inventory_is_not_a_change_and_is_never_attributed() {
    let Some(rig) = Rig::new().await else { return };
    let _agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    // A record at the very time of the inventory (T): the file was written on the VM when the swimlane was first scanned.
    rig.records(batch(1, vec![record("svc/a.yml", support::rig::T)]))
        .await
        .unwrap();
    rig.clock.advance_secs(60);
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(rig.attribution("svc/a.yml", 0).await.is("unknown", "low"));
    assert!(rig.upgrade_audits().await.is_empty());
}

#[tokio::test]
async fn one_record_explains_every_change_of_its_path_within_the_minute_a_residual_risk_of_the_rule() {
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(100);
    agent.change("svc/a.yml", "v: 1", T0 + 20 * S).await;
    agent.change("svc/a.yml", "v: 2", T0 + 40 * S).await;
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 30 * S)]))
        .await
        .unwrap();
    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
    assert!(
        rig.attribution("svc/a.yml", 2).await.is("sentinel_login", "high"),
        "a record is not used up (D73 states the rule as a window): the threat model says so"
    );
}

#[tokio::test]
async fn the_order_in_which_the_scan_the_record_and_the_job_come_does_not_change_the_answer() {
    // The final attribution is Ada's in every order, and what is audited is one event for each step that changed it: an NFS client's
    // that a late record then upgrades is two.
    for (order, upgrades) in [
        ("scan record job", 1),
        ("record scan job", 1),
        ("scan job record", 2),
        ("record job scan job", 1),
    ] {
        let Some(rig) = Rig::new().await else { return };
        let mut agent = world(&rig, &[("svc/a.yml", "v: 0"), ("svc/other.yml", "v: 0")]).await;
        rig.sentinel_hello().await.unwrap();
        rig.clock.advance_secs(6);
        let mut batches = 0;
        for step in order.split(' ') {
            match step {
                "scan" => agent.change("svc/a.yml", "v: 1", T0 + 5 * S).await,
                "record" => {
                    batches += 1;
                    rig.records(batch(batches, vec![record("svc/a.yml", T0 + 3 * S)]))
                        .await
                        .unwrap();
                }
                "job" => {
                    rig.clock.advance_secs(40);
                    // The sentinel is seen since the change: an unrelated batch.
                    batches += 1;
                    rig.records(batch(batches, vec![record("svc/other.yml", now(&rig))]))
                        .await
                        .unwrap();
                    rig.sentinel.grace().run_once().await.unwrap();
                }
                other => unreachable!("{other}"),
            }
        }
        let a = rig.attribution("svc/a.yml", 1).await;
        assert!(a.is("sentinel_login", "high"), "{order}: {a:?}");
        assert!(a.user().is_some(), "{order}");
        assert_eq!(rig.upgrade_audits().await.len(), upgrades, "{order}");
        assert_eq!(rig.attribution_updates().await.len(), upgrades, "{order}");
    }
}

#[tokio::test]
async fn a_denied_files_change_is_attributed_by_name_and_no_content_or_blob_is_involved() {
    // T9's rule (D79, D104): a denied file's history shows who changed it, never what it holds. A sentinel sends no content and the
    // registry's sink has no blob store, so attributing the change touches no blob; the audit event has no diff.
    let Some(rig) = Rig::new().await else { return };
    let mut agent = world(&rig, &[("keys/app.pem", "key: 0")]).await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(26);
    agent.change_denied("keys/app.pem", "key: 1", T0 + 25 * S).await;
    let blobs = rig.count("SELECT count(*) FROM blobs").await;
    let denied = rig.state("keys/app.pem").await.unwrap();
    assert!(denied.denied, "the setup: the file is denied");

    rig.records(batch(1, vec![record("keys/app.pem", T0 + 10 * S)]))
        .await
        .unwrap();
    let a = rig.attribution("keys/app.pem", 1).await;
    assert!(a.is("sentinel_login", "high"), "{a:?}");
    assert!(a.user().is_some());
    assert_eq!(
        rig.count("SELECT count(*) FROM blobs").await,
        blobs,
        "no blob was read or written for it"
    );
    let diff: Option<String> = sqlx::query_scalar(
        "SELECT event_json::jsonb ->> 'diff' FROM audit_events WHERE action = 'attribution_upgraded'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(diff, None, "the audit event carries a name and a hash, no text");
    // The stored record carries a path, an operation and names, and nothing that is a file's content.
    assert_eq!(rig.stored_records().await.len(), 1);
}
