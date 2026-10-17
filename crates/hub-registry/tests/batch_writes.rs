//! P15 (T1): ingestion writes the audit and outbox events of a chunk as batches, over the real `PgAuditLog` and `PgEventBus`, and what
//! it writes is what it wrote when it did it one event at a time.
//!
//! - **Equivalence.** The same deltas run through a registry that audits and publishes one event at a time (the reference: the default
//!   `record_many` and `publish_many_in_tx` of the ports, which loop over the single calls of the same real log and bus) and through
//!   the batch path. Every column of `audit_events` but the one the database stamps with the time of the insert, every row of
//!   `outbox` but its `created_at`, and the result of the nightly verifier over the chain (S9) are equal.
//! - **No silent fallback.** A decorator that forwarded only `record` and `publish_in_tx` would still work, through the ports' default
//!   loops, and would be as slow as before: the statements are counted, from sqlx's own statement log, and a chunk of 50 paths must
//!   be one insert into `audit_events` and one into `outbox`, not 50 of each. The same count on the reference shows the counter can
//!   tell the two apart.
//! - **Errors.** A port that refuses a batch for size, which the registry's own cut should make impossible, is an internal error: the
//!   agent hears `Unavailable`, the transaction rolls back, and the log has the stage and nothing of the batch.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::sync::Arc;

use domain::JobRef;
use hub_identity::audit::{ChainVerifier, EcdsaP256Verifier, MemoryBucket};
use hub_registry::RegistryConfig;
use ports::{AuditError, BusError, SinkError, ToolWrites, Tx, content_hash};
use sqlx::{PgPool, Row};
use support::log_capture::LogCapture;
use support::rig::{Rig, T, delta, entry, path, root, text};
use support::statements::{Statement, inserts, statements};

fn chunks_of(n: usize) -> RegistryConfig {
    RegistryConfig {
        chunk_paths: n,
        ..RegistryConfig::default()
    }
}

// ------------------------------------------------------------------------------------------------ what the database holds

type Dump = Vec<Vec<Option<String>>>;

/// The columns of `audit_events` that the two paths must write alike: all of them but `recorded_at`, which the database sets to the time
/// of the insert.
const AUDIT_COLUMNS: [&str; 20] = [
    "seq",
    "prev_hash",
    "hash",
    "event_json",
    "at",
    "actor_kind",
    "actor_ref",
    "action",
    "via",
    "swimlane_id",
    "path",
    "hash_before",
    "hash_after",
    "github_login",
    "approval_id",
    "request_id",
    "attribution_source",
    "attribution_confidence",
    "attribution_actor",
    "attribution_evidence",
];

/// Likewise for `outbox`: `created_at` is the time of the insert, and `published_at` is the relay's, which does not run here.
const OUTBOX_COLUMNS: [&str; 2] = ["id", "event"];

/// Every column the comparison skips must be one that is time-derived, and every other column must be compared: a column added to
/// either table is a failure here until someone decides which it is.
async fn assert_columns_covered(pool: &PgPool, table: &str, compared: &[&str], time_derived: &[&str]) {
    let mut held: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns WHERE table_name = $1 AND table_schema = 'public'",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .unwrap();
    held.sort();
    let mut known: Vec<String> = compared
        .iter()
        .chain(time_derived)
        .map(|c| (*c).to_owned())
        .collect();
    known.sort();
    assert_eq!(
        held, known,
        "{table}: a column is neither compared nor known to be time-derived"
    );
}

async fn dump(pool: &PgPool, table: &str, columns: &[&str]) -> Dump {
    // The names are this file's constants, never input. `::text` for every column, so that one reader serves all of them. A hash
    // column is a domain over `bytea`: `encode` shows it as hex.
    let list: Vec<String> = columns
        .iter()
        .map(|c| match *c {
            "prev_hash" | "hash" | "hash_before" | "hash_after" => format!("encode({c}::bytea, 'hex')"),
            // Unix milliseconds, not the text of a `timestamptz`, which carries the time zone of the session.
            "at" => format!("(EXTRACT(EPOCH FROM {c}) * 1000)::bigint::text"),
            other => format!("{other}::text"),
        })
        .collect();
    // `table.column`, because a bare name in ORDER BY means the output column of that name, which is the text of the number.
    let sql = format!(
        "SELECT {} FROM {table} ORDER BY {table}.{} LIMIT 50000",
        list.join(", "),
        columns[0]
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await.unwrap();
    rows.iter()
        .map(|r| {
            (0..columns.len())
                .map(|i| r.get::<Option<String>, _>(i))
                .collect()
        })
        .collect()
}

/// A dump as text, one line for each row and `column=value` for each value the row has, so that a snapshot reads and diffs.
fn rendered(table: &str, columns: &[&str], rows: &Dump) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for row in rows {
        out.push_str(table);
        for (column, value) in columns.iter().zip(row) {
            if let Some(value) = value {
                let _ = write!(out, " {column}={value}");
            }
        }
        out.push('\n');
    }
    out
}

/// The nightly verifier (S9) over the whole chain: how many events it checked and whether it found nothing wrong.
async fn verified(pool: &PgPool) -> (u64, bool) {
    let verifier = ChainVerifier::new(
        pool.clone(),
        Arc::new(MemoryBucket::new()),
        Arc::new(EcdsaP256Verifier::default()),
    );
    let report = verifier
        .verify()
        .await
        .expect("the verifier could not read the audit tables");
    (report.events_checked, report.is_clean())
}

// ------------------------------------------------------------------------------------------------ the script

const NAMES: [&str; 9] = [
    "svc/a.yml",
    "svc/b.yml",
    "svc/c.yml",
    "svc/d.yml",
    "svc/e.yml",
    "svc/f.yml",
    "svc/g.yml",
    "other/h.yml",
    "other/i.yml",
];

/// A tool write, registered the way prompt 06 does before it writes the file: through the port, in its own transaction.
async fn tool_write(rig: &Rig, p: &str, content: &[u8]) {
    let user = domain::UserId::new(
        domain::Guid::parse("11111111-1111-1111-1111-111111111111").unwrap(),
        domain::Guid::parse("22222222-2222-2222-2222-222222222222").unwrap(),
    );
    let mut tx = rig.pool.begin().await.unwrap();
    rig.tool_writes
        .register(
            &mut Tx::from_pg(&mut tx),
            &rig.swimlane,
            &path(p),
            &content_hash(content),
            &user,
            &domain::RequestId::parse("req-batch-1").unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Everything that makes the registry audit and publish, in chunks of three paths, so that the chunks are batches of three, of two
/// and of one:
///
/// 1. the first inventory (nothing is audited, nine events are published);
/// 2. an increment: five files changed, one of them three times in the delta (one audit event for the path, D93), one removed
///    (seven paths: chunks of 3, 3 and 1);
/// 3. a change made during a sync job (audited as a sync, a chunk of 2);
/// 4. a tool write and a change made by hand in one chunk (the tool audits its own write: one audit event, two published; the
///    registration publishes nothing, the scan that explains it publishes `FileObserved` with source `tool_write`);
/// 5. the second delta sent again, as an agent does that was not acknowledged (nothing changes);
/// 6. a full scan that leaves two files out (they are gone: removals, a chunk of 2) and changes one.
async fn script(rig: &Rig) {
    rig.hello().await;
    let inventory = NAMES.iter().map(|n| entry(n, b"v: 0", T)).collect();
    rig.delta(delta(1, None, root(1), inventory)).await.unwrap();

    rig.clock.advance_secs(30);
    let mut d2 = delta(
        2,
        Some(root(1)),
        root(2),
        vec![
            entry("svc/a.yml", b"v: 1", T + 5_000),
            entry("svc/b.yml", b"v: 1", T + 5_001),
            entry("svc/c.yml", b"v: 1", T + 5_002),
            entry("svc/d.yml", b"v: 1", T + 5_003),
            entry("svc/e.yml", b"v: 1", T + 5_004),
            entry("svc/g.yml", b"v: 1", T + 6_000),
            entry("svc/g.yml", b"v: 2", T + 7_000),
            entry("svc/g.yml", b"v: 3", T + 8_000),
        ],
    );
    d2.removed = vec![path("other/i.yml")];
    rig.delta(d2.clone()).await.unwrap();

    rig.clock.advance_secs(30);
    let mut d3 = delta(
        3,
        Some(root(2)),
        root(3),
        vec![
            entry("svc/a.yml", b"v: 2", T + 20_000),
            entry("svc/b.yml", b"v: 2", T + 20_001),
        ],
    );
    d3.during_job = Some(JobRef::new("sync-sit1", "uid-77").unwrap());
    rig.delta(d3).await.unwrap();

    rig.clock.advance_secs(30);
    tool_write(rig, "svc/c.yml", b"v: edited by the tool").await;
    rig.delta(delta(
        4,
        Some(root(3)),
        root(4),
        vec![
            entry("svc/c.yml", b"v: edited by the tool", T + 30_000),
            entry("svc/d.yml", b"v: 4", T + 30_001),
        ],
    ))
    .await
    .unwrap();

    rig.clock.advance_secs(30);
    rig.delta(d2).await.unwrap();

    rig.clock.advance_secs(30);
    let full = [
        ("svc/a.yml", "v: 2"),
        ("svc/b.yml", "v: 2"),
        ("svc/c.yml", "v: edited by the tool"),
        ("svc/d.yml", "v: 4"),
        ("svc/e.yml", "v: 9"),
        ("svc/g.yml", "v: 3"),
    ];
    let entries = full
        .iter()
        .map(|(n, c)| entry(n, c.as_bytes(), T + 40_000))
        .collect();
    rig.delta(delta(6, None, root(6), entries)).await.unwrap();
}

#[tokio::test]
async fn the_batch_path_writes_what_the_per_event_path_wrote() {
    let Some(batched) = Rig::with("sit1", chunks_of(3)).await else {
        return;
    };
    let Some(reference) = Rig::per_event("sit1", chunks_of(3)).await else {
        return;
    };
    script(&batched).await;
    script(&reference).await;

    // The two really took different paths: batches in one, a call for every event in the other.
    // Batches of 3, 3, 2 and 2 events; the chunks of one event are single calls.
    assert_eq!(batched.audit.batch_calls(), 4);
    assert_eq!(batched.audit.single_calls(), 3);
    assert_eq!(
        reference.audit.batch_calls(),
        0,
        "the reference writes one event at a time"
    );
    assert_eq!(reference.audit.single_calls(), 13);

    for rig in [&batched, &reference] {
        assert_columns_covered(&rig.pool, "audit_events", &AUDIT_COLUMNS, &["recorded_at"]).await;
        assert_columns_covered(
            &rig.pool,
            "outbox",
            &OUTBOX_COLUMNS,
            &["created_at", "published_at"],
        )
        .await;
    }

    let (audit_batched, audit_reference) = (
        dump(&batched.pool, "audit_events", &AUDIT_COLUMNS).await,
        dump(&reference.pool, "audit_events", &AUDIT_COLUMNS).await,
    );
    assert_eq!(audit_batched, audit_reference, "audit_events");

    let (outbox_batched, outbox_reference) = (
        dump(&batched.pool, "outbox", &OUTBOX_COLUMNS).await,
        dump(&reference.pool, "outbox", &OUTBOX_COLUMNS).await,
    );
    // 9 for the inventory, 7 for the increment, 2 for the sync, the two paths of the chunk that has the tool's write (the registration
    // publishes nothing: the scan that its hash explains publishes the observation, with source `tool_write`), and the full scan's 3.
    // The replay publishes nothing.
    assert_eq!(outbox_batched.len(), 9 + 7 + 2 + 2 + 3, "outbox events");
    assert_eq!(outbox_batched, outbox_reference, "outbox");

    // And both are what the registry wrote before it batched: the snapshot was recorded by the code of this crate at `e2bec86`, whose
    // apply path called `AuditLog::record` and `EventBus::publish_in_tx` once for each path, over the same script, and holds every
    // column that is not the time of the insert, the chain's hashes included. A change of an event's content or of the order of
    // the events is a change of this snapshot and of the chain. One change was made on purpose, with the port `ToolWrites` that replaced the
    // old seam (05/T1): the registration of the tool's write publishes no `FileObserved` any more (a registration is not an observation),
    // and the observation that it explains is published once, with source `tool_write`; the audit chain is the same, event for event.
    insta::assert_snapshot!(
        "the_rows_of_the_script",
        format!(
            "{}{}",
            rendered("audit", &AUDIT_COLUMNS, &audit_batched),
            rendered("outbox", &OUTBOX_COLUMNS, &outbox_batched)
        )
    );

    // S9: the chain that the batches wrote is the chain the nightly verifier accepts, and it is the same chain.
    let rows = i64::try_from(audit_batched.len()).unwrap();
    assert_eq!(
        verified(&batched.pool).await,
        (u64::try_from(rows).unwrap(), true),
        "the verifier checked every event of the batched chain and found nothing wrong"
    );
    assert_eq!(verified(&reference.pool).await, verified(&batched.pool).await);

    // One audit event for each path that moved and that nobody else audits, counted from the script (rule 8, D93): chunks 3+3+1
    // (the path that changed three times counts once), 2 (the sync), 1 (the tool's own write is not audited), and the full scan's
    // 2 removals and 1 change. The replay of the second delta adds none.
    assert_eq!(audit_batched.len(), 7 + 2 + 1 + 3);
}

// ------------------------------------------------------------------------------------------------ statements

/// An inventory of `n` files, then a delta that changes the first `changed` of them, with the statements of that delta counted.
async fn statements_of_a_delta(rig: &Rig, n: usize, changed: usize) -> Vec<Statement> {
    rig.hello().await;
    let names: Vec<String> = (0..n).map(|i| format!("svc/f{i:04}.yml")).collect();
    let inventory = names.iter().map(|p| entry(p, b"v: 0", T)).collect();
    rig.delta(delta(1, None, root(1), inventory)).await.unwrap();

    let (capture, _guard) = LogCapture::install();
    let edits = names[..changed]
        .iter()
        .map(|p| entry(p, b"v: 1", T + 5_000))
        .collect();
    rig.delta(delta(2, Some(root(1)), root(2), edits)).await.unwrap();
    statements(&capture)
}

#[tokio::test]
async fn a_chunk_of_fifty_paths_is_one_audit_insert_and_one_outbox_insert() {
    let Some(rig) = Rig::new().await else { return };
    let all = statements_of_a_delta(&rig, 60, 50).await;
    assert!(
        !all.is_empty(),
        "no statement was captured: the counts below would pass for nothing"
    );

    assert_eq!(
        inserts(&all, "INSERT INTO audit_events"),
        vec![50],
        "one insert of 50 audit rows, not one for each path: the audit log took the batch path"
    );
    assert_eq!(
        inserts(&all, "INSERT INTO outbox"),
        vec![50],
        "one insert of 50 outbox rows: the bus took the batch path"
    );
    // The audit log reads the head of the chain once and takes its lock once for the chunk, as `record_many` promises. (The
    // registry's own lock of the swimlane is another `pg_advisory_xact_lock`: it is not the chain's.)
    assert_eq!(inserts(&all, "SELECT hash FROM audit_events").len(), 1);
    assert_eq!(
        all.iter()
            .filter(|s| s.text.contains("pg_advisory_xact_lock(") && !s.text.contains("lanekeeper.registry"))
            .count(),
        1,
        "the chain's lock is taken once"
    );
    assert_eq!(rig.audit.batch_calls(), 1);
    assert_eq!(rig.audit.single_calls(), 0, "no call of `record` for a path");

    // The rows are all there.
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 50);
    assert_eq!(
        rig.count("SELECT count(*) FROM outbox").await,
        60 + 50,
        "the inventory's 60 events and the delta's 50"
    );
}

#[tokio::test]
async fn the_counter_sees_the_slow_path_when_a_decorator_falls_back_to_it() {
    // The reference writes one event at a time, over the same real log and bus. The same count must tell it from the batch path, or
    // the test above could pass for the wrong reason.
    let Some(rig) = Rig::per_event("sit1", RegistryConfig::default()).await else {
        return;
    };
    let all = statements_of_a_delta(&rig, 60, 50).await;
    assert_eq!(inserts(&all, "INSERT INTO audit_events"), vec![1; 50]);
    assert_eq!(inserts(&all, "INSERT INTO outbox"), vec![1; 50]);
    assert_eq!(inserts(&all, "SELECT hash FROM audit_events").len(), 50);
}

#[tokio::test]
async fn a_chunk_over_the_limits_of_a_batch_is_cut_in_order_and_is_still_one_chain() {
    // `chunk_paths` goes up to 1,000, and a batch is at most 200 events: 450 paths are three batches in one transaction.
    let Some(rig) = Rig::with("sit1", chunks_of(450)).await else {
        return;
    };
    let all = statements_of_a_delta(&rig, 450, 450).await;
    assert_eq!(inserts(&all, "INSERT INTO audit_events"), vec![200, 200, 50]);
    assert_eq!(inserts(&all, "INSERT INTO outbox"), vec![200, 200, 50]);
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 450);
    assert_eq!(verified(&rig.pool).await, (450, true));
    // The order of the events is the order of the paths, whatever the cut.
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM audit_events ORDER BY seq")
        .fetch_all(&rig.pool)
        .await
        .unwrap();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
}

// ------------------------------------------------------------------------------------------------ transactions

#[tokio::test]
async fn a_message_of_one_chunk_is_one_transaction() {
    // The chunk, the root, the agent's acknowledgement and the staged parts' removal commit together: one `COMMIT` (one flush of the
    // write-ahead log) and one transaction's `set_config` for a message of 50 versions, and the rules of the swimlane are read in that
    // transaction and not in one of their own. A last small transaction of its own was a third of the registry's commits.
    let Some(rig) = Rig::new().await else { return };
    let all = statements_of_a_delta(&rig, 60, 50).await;
    assert_eq!(inserts(&all, "COMMIT").len(), 1, "one commit for the message");
    assert_eq!(
        inserts(&all, "SELECT set_config(").len(),
        1,
        "one transaction for the message"
    );
    assert_eq!(
        inserts(&all, "SELECT repo, repo_root FROM").len(),
        1,
        "the rules are read once"
    );
    // And it is the whole of the delta: the root moved, the acknowledgement too.
    assert_eq!(rig.root().await, Some(root(2)));
}

#[tokio::test]
async fn the_last_chunk_carries_the_root_and_each_chunk_before_it_commits_alone() {
    // 50 paths in chunks of 20: three transactions, the last of which also moves the root. The first two cannot, or a crash after them
    // would leave a root that says the delta is applied.
    let Some(rig) = Rig::with("sit1", chunks_of(20)).await else {
        return;
    };
    let all = statements_of_a_delta(&rig, 60, 50).await;
    assert_eq!(inserts(&all, "COMMIT").len(), 3);
    assert_eq!(inserts(&all, "SELECT set_config(").len(), 3);
    assert_eq!(
        inserts(&all, "INSERT INTO swimlane_merkle").len(),
        1,
        "the root moves once, with the last chunk"
    );
    assert_eq!(
        inserts(&all, "SELECT repo, repo_root FROM").len(),
        1,
        "the rules are read once for the three chunks"
    );
    assert_eq!(rig.root().await, Some(root(2)));
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 50);
}

#[tokio::test]
async fn a_delta_with_no_path_still_moves_the_root_in_a_transaction_of_its_own() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1")]).await;
    rig.delta(delta(2, Some(root(1)), root(2), Vec::new()))
        .await
        .unwrap();
    assert_eq!(rig.root().await, Some(root(2)));
}

// ------------------------------------------------------------------------------------------------ the cluster report

#[tokio::test]
async fn the_events_of_a_cluster_report_are_one_batch_in_order() {
    // Two sync windows that open and a Deployment that has a change to pick up: three outbox events in the transaction of the report, in
    // the order the report names them (the windows, then the pickup state that the Deployment's appearance changed), written with one
    // statement (T10: `PickupChanged` replaced `PendingRestartChanged`, A2).
    use domain::{
        ClusterReport, DeploymentInfo, DomainEvent, EnvValue, PodInfo, ServiceRef, SyncWindowEvent,
        SyncWindowKind, Timestamp,
    };
    let Some(rig) = Rig::new().await else { return };
    // A resource of the folder `tx-infinity-api` that changed after the first inventory, and is not yet picked up by the Deployment that
    // appears with a pod from before the change.
    rig.first_scan(&[("tx-infinity-api/layout.xsl", "<a/>")]).await;
    rig.delta(delta(
        2,
        Some(root(1)),
        root(2),
        vec![entry("tx-infinity-api/layout.xsl", b"<b/>", T + 60_000)],
    ))
    .await
    .unwrap();
    let before = rig.bus.count().await;
    let window = |uid: &str, at: i64| SyncWindowEvent {
        kind: SyncWindowKind::Opened,
        job: JobRef::new("sync-sit1", uid).unwrap(),
        at: Timestamp::from_unix_millis(at),
    };
    let report = ClusterReport {
        full: false,
        deployments: vec![DeploymentInfo {
            service: ServiceRef::new("csp", "tx-infinity-api").unwrap(),
            pods: vec![PodInfo {
                name: text("tx-infinity-api-1"),
                started_at: Timestamp::from_unix_millis(T - 3_600_000),
            }],
            env_values: vec![EnvValue {
                name: text("SPRING_APPLICATION_NAME"),
                value: text("tx-infinity-api"),
            }],
            env_names: Vec::new(),
            removed: false,
        }],
        release_hints: Vec::new(),
        config_server_started_at: None,
        sync_windows: vec![
            window("uid-1", hub_registry::ManualClock::T0_MS - 60_000),
            window("uid-2", hub_registry::ManualClock::T0_MS - 50_000),
        ],
    };
    let (capture, _guard) = LogCapture::install();
    rig.cluster(report).await.unwrap();
    let all = statements(&capture);
    assert_eq!(
        inserts(&all, "INSERT INTO outbox"),
        vec![3],
        "one insert of the three events, not one for each"
    );
    let published = rig.bus.published().await;
    assert_eq!(published.len() - before, 3);
    let kinds: Vec<&str> = published[published.len() - 3..]
        .iter()
        .map(|e| match e {
            DomainEvent::SyncWindowOpened { .. } => "window_opened",
            DomainEvent::PickupChanged { .. } => "pickup_changed",
            DomainEvent::PendingRestartChanged { .. } => "the old event",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["window_opened", "window_opened", "pickup_changed"]);
}

// ------------------------------------------------------------------------------------------------ errors

#[tokio::test]
async fn a_port_that_refuses_a_batch_for_size_is_an_internal_error_logged_by_stage() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[("a.yml", "x: 1"), ("b.yml", "y: 1"), ("c.yml", "z: 1")])
        .await;
    let before = (
        rig.count("SELECT count(*) FROM audit_events").await,
        rig.count("SELECT count(*) FROM outbox").await,
        rig.count("SELECT count(*) FROM file_observations").await,
    );
    let d = delta(
        2,
        Some(root(1)),
        root(2),
        vec![
            entry("a.yml", b"x: 2", T + 5_000),
            entry("b.yml", b"y: 2", T + 5_001),
            entry("c.yml", b"z: 2", T + 5_002),
        ],
    );

    let (capture, _guard) = LogCapture::install();
    rig.audit.fail_with(Some(AuditError::BatchTooLarge));
    assert_eq!(
        rig.delta(d.clone()).await,
        Err(SinkError::Unavailable),
        "a bug of the registry is not the agent's: it is told to try again, and nothing else"
    );
    let log = capture.text();
    assert!(log.contains("stage=\"audit_batch\""), "{log}");
    for secret in ["a.yml", "b.yml", "x: 2"] {
        assert!(!log.contains(secret), "the log names nothing of the batch: {log}");
    }
    rig.audit.fail_with(None);

    rig.bus.fail_with(Some(BusError::BatchTooLarge));
    assert_eq!(rig.delta(d.clone()).await, Err(SinkError::Unavailable));
    assert!(
        capture.text().contains("stage=\"event_batch\""),
        "{}",
        capture.text()
    );
    rig.bus.fail_with(None);

    // Neither left anything behind: the chunk rolled back whole.
    assert_eq!(
        (
            rig.count("SELECT count(*) FROM audit_events").await,
            rig.count("SELECT count(*) FROM outbox").await,
            rig.count("SELECT count(*) FROM file_observations").await,
        ),
        before
    );
    assert_eq!(rig.root().await, Some(root(1)), "the root did not move");

    // Both healthy: the same delta, sent again, applies.
    rig.delta(d).await.unwrap();
    assert_eq!(rig.count("SELECT count(*) FROM audit_events").await, 3);
}
