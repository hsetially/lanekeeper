//! T5 (S9, S10, P12): the hash-chained audit log over a real Postgres, as `lanekeeper_app`, the role the hub runs as.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::time::{Duration, Instant};

use domain::{Attribution, AttributionSource, Confidence, ProposalId, ShortText, Timestamp};
use hub_identity::audit::{CHAIN_LOCK_KEY, PgAuditLog};
use ports::conformance::sample::{hash, nfs, request_id, swimlane, user_id};
use ports::{
    AuditAction, AuditActor, AuditError, AuditEvent, AuditLog, AuditVia, Tx, genesis_hash, verify_chain,
};
use sqlx::{PgPool, Row};
use support::log_capture;
use support::pg::{TestDb, sqlstate};
use support::pg_tx::{PgAuditProbe, PgTxFactory, committed};

fn event(n: i64) -> AuditEvent {
    AuditEvent::new(
        Timestamp::from_unix_millis(1_791_676_800_000 + n),
        AuditActor::User { user: user_id(1) },
        AuditAction::SettingsChanged,
        AuditVia::Ui,
    )
}

/// Record `e` in a transaction of its own and commit it.
async fn record_committed(pool: &PgPool, e: AuditEvent) -> domain::AuditId {
    let mut tx = pool.begin().await.unwrap();
    let id = PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

#[tokio::test]
async fn chain_is_linear_for_sequential_records() {
    let db = db!();
    let pool = db.app_pool(4).await;
    for n in 0..20 {
        record_committed(&pool, event(n)).await;
    }
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 20);
    assert_eq!(chain[0].prev_hash, genesis_hash());
    assert_eq!(verify_chain(&chain), Ok(()));
    for pair in chain.windows(2) {
        assert_eq!(pair[1].prev_hash, pair[0].hash);
        assert!(pair[1].id > pair[0].id);
    }
}

#[tokio::test]
async fn concurrent_writers_across_two_instances_give_one_linear_chain() {
    let db = db!();
    // Two hub replicas: two pools on one database.
    let instances = [db.app_pool(10).await, db.app_pool(10).await];
    let mut tasks = Vec::new();
    for writer in 0..16_i64 {
        let pool = instances[usize::try_from(writer % 2).unwrap()].clone();
        tasks.push(tokio::spawn(async move {
            for round in 0..5_i64 {
                let mut tx = pool.begin().await.unwrap();
                let log = PgAuditLog::new();
                log.record(&mut Tx::from_pg(&mut tx), event(writer * 100 + round))
                    .await
                    .unwrap();
                // Some of the work after `record` takes a moment, as a real caller's commit does.
                tokio::task::yield_now().await;
                if round == 3 && writer % 4 == 0 {
                    tx.rollback().await.unwrap();
                } else {
                    tx.commit().await.unwrap();
                }
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let chain = committed(&instances[0]).await;
    // 16 writers x 5 rounds, minus the 4 rolled back.
    assert_eq!(chain.len(), 76);
    assert_eq!(verify_chain(&chain), Ok(()), "one linear chain, no fork");
    let prevs: std::collections::BTreeSet<_> = chain.iter().map(|e| e.prev_hash).collect();
    assert_eq!(prevs.len(), chain.len(), "no two events share a predecessor");
}

#[tokio::test]
async fn rolled_back_record_leaves_no_row_and_next_record_chains_correctly() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let first = record_committed(&pool, event(1)).await;
    let mut tx = pool.begin().await.unwrap();
    PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), event(2))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let third = record_committed(&pool, event(3)).await;
    let chain = committed(&pool).await;
    assert_eq!(chain.len(), 2);
    assert_eq!(
        chain[1].prev_hash, chain[0].hash,
        "the rolled-back event is not in the chain"
    );
    assert_eq!(verify_chain(&chain), Ok(()));
    // The sequence is not transactional: the number of the rolled-back event is gone, and nothing relies on contiguity.
    assert!(third.get() >= first.get() + 2);
}

#[tokio::test]
async fn app_role_cannot_update_or_delete_audit_rows() {
    let db = db!();
    let pool = db.app_pool(2).await;
    record_committed(&pool, event(1)).await;
    for stmt in [
        "UPDATE audit_events SET path = 'x'",
        "DELETE FROM audit_events",
        "TRUNCATE audit_events",
        "UPDATE audit_checkpoints SET signature = '\\x00'",
        "DELETE FROM audit_checkpoints",
        "TRUNCATE audit_checkpoints",
    ] {
        let err = sqlx::query(stmt).execute(&pool).await.unwrap_err();
        assert_eq!(sqlstate(&err).as_deref(), Some("42501"), "{stmt}: {err}");
    }
    // The owner has the privilege and is stopped by the trigger, which also fires for superusers.
    let mut owner = db.migrator().await;
    let err = sqlx::query("UPDATE audit_events SET path = 'x'")
        .execute(&mut owner)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err).as_deref(), Some("23001"), "{err}");
    let mut admin = db.admin().await;
    let err = sqlx::query("DELETE FROM audit_events")
        .execute(&mut admin)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err).as_deref(), Some("23001"), "{err}");
}

#[tokio::test]
async fn fork_is_refused_by_unique_prev_hash() {
    let db = db!();
    let pool = db.app_pool(4).await;
    record_committed(&pool, event(1)).await;
    let head = committed(&pool).await.pop().unwrap().hash;
    // Two writers that skip the lock and both build on the same head.
    let insert = "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, action, via) \
                  VALUES ($1, sha256($1 || convert_to($2, 'UTF8')), $2, now(), 'system', 'settings_changed', 'system')";
    let mut a = pool.begin().await.unwrap();
    sqlx::query(insert)
        .bind(head.as_bytes().as_slice())
        .bind("{\"fork\":\"a\"}")
        .execute(&mut *a)
        .await
        .unwrap();
    let pool_b = pool.clone();
    let b = tokio::spawn(async move {
        let mut b = pool_b.begin().await.unwrap();
        // Waits for `a` to commit or roll back, then fails.
        sqlx::query(insert)
            .bind(head.as_bytes().as_slice())
            .bind("{\"fork\":\"b\"}")
            .execute(&mut *b)
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.commit().await.unwrap();
    let err = b.await.unwrap().unwrap_err();
    assert_eq!(sqlstate(&err).as_deref(), Some("23505"), "{err}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 2, "the first writer's row and the original, not the fork");
}

#[tokio::test]
async fn the_table_refuses_a_row_whose_hash_is_not_the_chain_hash() {
    let db = db!();
    let pool = db.app_pool(2).await;
    let err = sqlx::query(
        "INSERT INTO audit_events (prev_hash, hash, event_json, at, actor_kind, action, via) \
         VALUES ($1, $2, '{}', now(), 'system', 'settings_changed', 'system')",
    )
    .bind([0u8; 32].as_slice())
    .bind([1u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&err).as_deref(), Some("23514"), "{err}");
}

#[tokio::test]
async fn record_hash_matches_ports_chain_hash_golden() {
    let db = db!();
    let pool = db.app_pool(2).await;
    let mut e = event(0);
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(nfs("tx-infinity-api/tx-infinity-core.yml"));
    e.hash_before = Some(hash(b"before"));
    e.hash_after = Some(hash(b"after"));
    e.diff = Some("-a: 1\n+a: 2\n".to_owned());
    record_committed(&pool, e).await;
    let row = sqlx::query("SELECT event_json, prev_hash, hash FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let json: String = row.get("event_json");
    insta::assert_snapshot!(json, @r#"{"at":1791676800000,"actor":{"kind":"user","user":"72f988bf-86f1-41af-91ab-2d7cd011db47:00000000-0000-4000-8000-000000000001"},"action":"settings_changed","via":"ui","swimlane":"sit1","path":"tx-infinity-api/tx-infinity-core.yml","hash_before":"6db7d803e74f1ffa7d8f5adc0bf95b3e15bf4c8373fffadf546227cc6c6742cb","hash_after":"f39592393ef0859cb196a52693d2cea00fb2df784b3c04ae54aa7cadb8e562f8","diff":"-a: 1\n+a: 2\n","github_login":null,"approval":null,"request_id":null,"attribution":null}"#);
    let prev: Vec<u8> = row.get("prev_hash");
    let stored: Vec<u8> = row.get("hash");
    assert_eq!(prev, genesis_hash().as_bytes().to_vec());
    let expected = ports::chain_hash(&genesis_hash(), json.as_bytes());
    assert_eq!(stored, expected.as_bytes().to_vec());
    insta::assert_snapshot!(expected.to_string(), @"5bc9c7994dc2cbf09274dba6441e4764229a7b6ec5320838eb6fe2e8d0149a53");
}

/// The canonical JSON of an event is pinned: the stored chain, the table's `CHECK` and the nightly verifier all hash this text, so
/// a change to the field order or a wire name breaks every chain already written. No database.
#[test]
fn canonical_json_golden() {
    let mut e = event(0);
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(nfs("tx-infinity-api/tx-infinity-core.yml"));
    e.hash_before = Some(hash(b"before"));
    e.hash_after = Some(hash(b"after"));
    e.diff = Some("-a: 1\n+a: 2\n".to_owned());
    let prepared = hub_identity::audit::log::prepare(&e).unwrap();
    insta::assert_snapshot!(prepared.json(), @r#"{"at":1791676800000,"actor":{"kind":"user","user":"72f988bf-86f1-41af-91ab-2d7cd011db47:00000000-0000-4000-8000-000000000001"},"action":"settings_changed","via":"ui","swimlane":"sit1","path":"tx-infinity-api/tx-infinity-core.yml","hash_before":"6db7d803e74f1ffa7d8f5adc0bf95b3e15bf4c8373fffadf546227cc6c6742cb","hash_after":"f39592393ef0859cb196a52693d2cea00fb2df784b3c04ae54aa7cadb8e562f8","diff":"-a: 1\n+a: 2\n","github_login":null,"approval":null,"request_id":null,"attribution":null}"#);
    assert_eq!(
        prepared.json().as_bytes(),
        ports::canonical_json(&e).unwrap().as_slice()
    );
}

#[tokio::test]
async fn every_column_is_filled_from_the_event() {
    let db = db!();
    let pool = db.app_pool(2).await;
    let mut e = AuditEvent::new(
        Timestamp::from_unix_millis(1_791_676_800_123),
        AuditActor::Sentinel {
            name: ShortText::parse("nfs-vm-1").unwrap(),
        },
        AuditAction::FileChangeDetected,
        AuditVia::AgentDetected,
    );
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(nfs("a/b.yml"));
    e.hash_before = Some(hash(b"x"));
    e.hash_after = Some(hash(b"y"));
    e.github_login = Some(ShortText::parse("octocat").unwrap());
    e.approval = Some(ProposalId::from(77));
    e.request_id = Some(request_id("req-1"));
    e.attribution = Some(Attribution {
        source: AttributionSource::SentinelLogin,
        confidence: Confidence::High,
        actor: Some(domain::Actor::OsLogin {
            login: ShortText::parse("jdoe").unwrap(),
        }),
        evidence: domain::Evidence {
            executable: Some(ShortText::parse("/usr/bin/vim").unwrap()),
            login_identity: None,
            observed_at: Some(Timestamp::from_unix_millis(5)),
        },
    });
    record_committed(&pool, e.clone()).await;
    let row = sqlx::query("SELECT * FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("actor_kind"), "sentinel");
    assert_eq!(
        row.get::<Option<String>, _>("actor_ref").as_deref(),
        Some("nfs-vm-1")
    );
    assert_eq!(row.get::<String, _>("action"), "file_change_detected");
    assert_eq!(row.get::<String, _>("via"), "agent_detected");
    assert_eq!(
        row.get::<Option<String>, _>("swimlane_id").as_deref(),
        Some("sit1")
    );
    assert_eq!(row.get::<Option<String>, _>("path").as_deref(), Some("a/b.yml"));
    assert_eq!(
        row.get::<Option<Vec<u8>>, _>("hash_before"),
        Some(hash(b"x").as_bytes().to_vec())
    );
    assert_eq!(
        row.get::<Option<Vec<u8>>, _>("hash_after"),
        Some(hash(b"y").as_bytes().to_vec())
    );
    assert_eq!(
        row.get::<Option<String>, _>("github_login").as_deref(),
        Some("octocat")
    );
    assert_eq!(row.get::<Option<i64>, _>("approval_id"), Some(77));
    assert_eq!(
        row.get::<Option<String>, _>("request_id").as_deref(),
        Some("req-1")
    );
    assert_eq!(
        row.get::<Option<String>, _>("attribution_source").as_deref(),
        Some("sentinel_login")
    );
    assert_eq!(
        row.get::<Option<String>, _>("attribution_confidence").as_deref(),
        Some("high")
    );
    let actor: serde_json::Value = row.get("attribution_actor");
    assert_eq!(actor["kind"], "os_login");
    let evidence: serde_json::Value = row.get("attribution_evidence");
    assert_eq!(evidence["executable"], "/usr/bin/vim");
    // `at` keeps the millisecond.
    let at_ms: i64 = sqlx::query_scalar("SELECT (extract(epoch FROM at) * 1000)::bigint FROM audit_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(at_ms, 1_791_676_800_123);
}

#[tokio::test]
async fn ports_conformance_suite_passes() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let probe = PgAuditProbe {
        log: PgAuditLog::new(),
        pool: pool.clone(),
    };
    ports::conformance::audit_log(&probe, &PgTxFactory(pool)).await;
}

#[tokio::test]
async fn oversize_diff_is_rejected_before_taking_the_lock() {
    let db = db!();
    let pool = db.app_pool(4).await;
    // Someone else holds the chain lock for the whole test.
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK_KEY)
        .execute(&mut *holder)
        .await
        .unwrap();
    let mut big = event(1);
    big.diff = Some("x".repeat(AuditEvent::MAX_DIFF_BYTES + 1));
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    // The default lock timeout is 5 s; an event that cannot be stored must not wait for it.
    let result = PgAuditLog::new().record(&mut Tx::from_pg(&mut tx), big).await;
    assert_eq!(result, Err(AuditError::TooLarge));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited for the lock: {:?}",
        started.elapsed()
    );
    holder.rollback().await.unwrap();
}

#[tokio::test]
async fn a_wait_for_the_lock_ends_with_unavailable_and_an_aborted_transaction() {
    let db = db!();
    let pool = db.app_pool(4).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK_KEY)
        .execute(&mut *holder)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let started = Instant::now();
    let log = PgAuditLog::new().with_lock_timeout(Duration::from_millis(300));
    let result = log.record(&mut Tx::from_pg(&mut tx), event(1)).await;
    assert_eq!(result, Err(AuditError::Unavailable));
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_secs(3),
        "{waited:?}"
    );
    // The action must not go ahead without its event: the transaction cannot commit.
    assert!(sqlx::query("SELECT 1").execute(&mut *tx).await.is_err());
    drop(tx);
    holder.rollback().await.unwrap();
    assert_eq!(committed(&pool).await.len(), 0);
}

#[tokio::test]
async fn a_transaction_of_another_store_is_refused() {
    let mut state = ports::fakes::FakeTxState::default();
    let result = PgAuditLog::new()
        .record(&mut Tx::in_memory(&mut state), event(1))
        .await;
    assert_eq!(result, Err(AuditError::WrongTx));
}

#[tokio::test]
async fn log_and_errors_never_carry_event_content() {
    let (capture, _guard) = log_capture::install();
    let db = db!();
    let pool = db.app_pool(4).await;
    let secret = "SECRET-DIFF-CONTENT-4711";
    let mut e = event(1);
    e.diff = Some(format!("+token: {secret}\n"));
    record_committed(&pool, e.clone()).await;
    // A failing record: the table refuses the event after the lock (a fork through a stale head is not reachable here, so the
    // transaction is poisoned first).
    let mut tx = pool.begin().await.unwrap();
    let _ = sqlx::query("SELECT nonexistent_column FROM audit_events")
        .execute(&mut *tx)
        .await;
    let err = PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), e.clone())
        .await
        .unwrap_err();
    let mut big = e;
    big.diff = Some(format!("{secret}{}", "x".repeat(AuditEvent::MAX_DIFF_BYTES)));
    let too_large = PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), big)
        .await
        .unwrap_err();
    let text = format!("{err} {err:?} {too_large} {too_large:?} {}", capture.text());
    assert!(!text.contains(secret), "event content leaked: {text}");
    assert!(
        capture.text().contains("audit record failed"),
        "the failure is logged by stage: {}",
        capture.text()
    );
}

// ---------------------------------------------------------------------------------------------------------------
// P12: the hot queries, by plan

/// Every `(Node Type, Relation Name or Index Name)` of an `EXPLAIN (FORMAT JSON)` plan.
fn plan_nodes(explain: &serde_json::Value) -> Vec<(String, String)> {
    fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
        let ty = node["Node Type"].as_str().unwrap_or_default().to_owned();
        let what = node["Index Name"]
            .as_str()
            .or_else(|| node["Relation Name"].as_str())
            .unwrap_or_default()
            .to_owned();
        out.push((ty, what));
        if let Some(children) = node["Plans"].as_array() {
            for c in children {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&explain[0]["Plan"], &mut out);
    out
}

async fn explain(db: &TestDb, sql: &str) -> serde_json::Value {
    let mut conn = db.admin().await;
    sqlx::query_scalar(&format!("EXPLAIN (FORMAT JSON) {sql}"))
        .fetch_one(&mut conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn explain_audit_head_is_backward_pk_scan() {
    let db = db!();
    let pool = db.app_pool(4).await;
    for n in 0..300 {
        record_committed(&pool, event(n)).await;
    }
    let mut admin = db.admin().await;
    sqlx::query("ANALYZE audit_events")
        .execute(&mut admin)
        .await
        .unwrap();
    let plan = explain(&db, "SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1").await;
    let nodes = plan_nodes(&plan);
    assert!(
        nodes
            .iter()
            .any(|(t, i)| t == "Index Scan" && i == "audit_events_pkey")
            && plan.to_string().contains("\"Scan Direction\":\"Backward\""),
        "expected a backward scan of the primary key: {plan}"
    );
    assert!(
        !nodes.iter().any(|(t, _)| t == "Seq Scan" || t == "Sort"),
        "{nodes:?}"
    );
}

/// The verifier compares `at` to the microsecond, so the log must write it exactly, whatever the date. A floating-point product of
/// the milliseconds and an interval is off by microseconds from the year 2255 on.
#[tokio::test]
async fn at_is_stored_exactly_for_any_date_the_column_can_hold() {
    let db = db!();
    let pool = db.app_pool(2).await;
    let dates = [
        -210_866_803_200_000, // 4713 BC
        -1,
        0,
        1,
        1_791_676_800_123,
        253_402_300_799_999, // 9999-12-31
        8_900_000_000_000_123,
    ];
    for ms in dates {
        let mut e = event(0);
        e.at = Timestamp::from_unix_millis(ms);
        record_committed(&pool, e).await;
    }
    let stored: Vec<i64> = sqlx::query_scalar(
        "SELECT (extract(epoch FROM at) * 1000000)::bigint FROM audit_events ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stored, dates.map(|ms| ms * 1000));
}
