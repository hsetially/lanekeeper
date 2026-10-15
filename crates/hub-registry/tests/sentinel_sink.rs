//! T7 (D72, D74, S5, S10, S25, A16, A17, rule 5, rule 10): the registry's side of the sentinel protocol over a real Postgres. `hello` only
//! for a sentinel an admin registered; batches bounded in records and bytes; `seq` replays and gaps; what is dropped and counted and never
//! stored (a chmod, a path outside the swimlane, a sentinel that serves nothing, a time in the future); a record never attributes in a
//! swimlane the sentinel does not serve; and no path, user name, login or executable in a log line, an error or a metric.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use domain::{AuditOperation, SwimlaneId};
use hub_registry::metrics::{Outcome, SentinelKind, SentinelRecord};
use hub_registry::{Clock, ManualClock, RegistryConfig};
use ports::{AckSeq, AgentIdentity, ReportSink, SentinelSink, SinkError};
use support::log_capture::LogCapture;
use support::rig::{Rig, delta, entry, hello, root};
use support::sentinel::{Agent, ROOT, VM, batch, hello_of, identity, op, record, text};

const T0: i64 = ManualClock::T0_MS;
const S: i64 = 1_000;

fn rows(n: usize, at: i64) -> Vec<domain::AuditRecord> {
    (0..n)
        .map(|i| record(&format!("svc/f{i}.yml"), at + i64::try_from(i).unwrap()))
        .collect()
}

// ================================================================================================ hello

#[tokio::test]
async fn hello_is_accepted_only_for_a_sentinel_an_admin_registered_with_the_root_it_registered() {
    let Some(rig) = Rig::new().await else { return };
    // Nobody registered `nfs-vm-1`: refused, and nothing is written.
    assert_eq!(rig.sentinel_hello().await, Err(SinkError::IdentityMismatch));
    rig.sentinel_for_the_swimlane().await;

    // The payload says another VM than the certificate does (S5): refused, whatever is registered.
    assert_eq!(
        rig.sentinel.hello(&identity(), hello_of("nfs-vm-2", ROOT)).await,
        Err(SinkError::IdentityMismatch)
    );
    // The right VM, but it says it watches another root than the admin registered: its paths would be mapped by the wrong root.
    assert_eq!(
        rig.sentinel
            .hello(&identity(), hello_of(VM, "/export/other"))
            .await,
        Err(SinkError::IdentityMismatch)
    );
    let seen: Option<i64> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM last_seen_at) * 1000)::bigint FROM sentinels WHERE vm = $1",
    )
    .bind(VM)
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(seen, None, "a refused hello is not a sighting");

    let config = rig.sentinel_hello().await.unwrap();
    assert_eq!((config.max_batch_records, config.flush_interval_secs), (500, 1));
    let (version, seen): (Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT version, (EXTRACT(EPOCH FROM last_seen_at) * 1000)::bigint FROM sentinels WHERE vm = $1",
    )
    .bind(VM)
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(version.as_deref(), Some("0.1.0"));
    assert_eq!(seen, Some(T0), "the hub's clock, not the sentinel's");
    // A reconnect says hello again.
    rig.clock.advance_secs(5);
    rig.sentinel_hello().await.unwrap();

    assert_eq!(
        rig.metrics
            .sentinel_message_count(SentinelKind::Hello, Outcome::Ok),
        2
    );
    assert_eq!(
        rig.metrics
            .sentinel_message_count(SentinelKind::Hello, Outcome::IdentityMismatch),
        3
    );
}

#[tokio::test]
async fn a_refused_hello_does_not_take_one_of_the_places() {
    let config = RegistryConfig {
        sentinel_max_streams: 1,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.sentinel_for_the_swimlane().await;
    rig.register_sentinel("nfs-vm-2", None, ROOT).await;
    // Names nobody registered are refused and give their place back: the one place is still free.
    for name in ["ghost-1", "ghost-2", "ghost-3"] {
        let id = ports::SentinelIdentity::new(text(name));
        assert_eq!(
            rig.sentinel.hello(&id, hello_of(name, ROOT)).await,
            Err(SinkError::IdentityMismatch)
        );
    }
    rig.sentinel_hello().await.unwrap();
    // The place is taken now: a second sentinel is refused for want of room, and the first can come back whenever it likes.
    let second = ports::SentinelIdentity::new(text("nfs-vm-2"));
    assert_eq!(
        rig.sentinel.hello(&second, hello_of("nfs-vm-2", ROOT)).await,
        Err(SinkError::Unavailable),
        "rule 5: the table of live sentinels is bounded"
    );
    rig.sentinel_hello().await.unwrap();
}

// ================================================================================================ records: identity

#[tokio::test]
async fn records_before_hello_or_from_a_sentinel_that_never_said_hello_are_refused() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.register_sentinel("nfs-vm-2", Some("sit1"), ROOT).await;
    assert_eq!(
        rig.records(batch(1, rows(1, T0))).await,
        Err(SinkError::HelloRequired)
    );
    rig.sentinel_hello().await.unwrap();
    // Another sentinel has not said hello, whatever the first one did.
    let other = ports::SentinelIdentity::new(text("nfs-vm-2"));
    assert_eq!(
        rig.sentinel.records(&other, batch(1, rows(1, T0))).await,
        Err(SinkError::HelloRequired)
    );
    assert_eq!(rig.stored_records().await.len(), 0);
    assert_eq!(
        rig.metrics
            .sentinel_message_count(SentinelKind::Records, Outcome::HelloRequired),
        2
    );
}

#[tokio::test]
async fn a_sentinel_that_was_removed_after_its_hello_is_refused() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    sqlx::query("DELETE FROM sentinels WHERE vm = $1")
        .bind(VM)
        .execute(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        rig.records(batch(1, rows(1, T0))).await,
        Err(SinkError::IdentityMismatch)
    );
}

// ================================================================================================ records: sequence numbers

#[tokio::test]
async fn a_batch_is_applied_once_and_a_resend_stores_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    assert_eq!(rig.records(batch(1, rows(3, T0))).await.unwrap(), AckSeq::new(1));
    let first = rig.stored_records().await;
    assert_eq!(
        first.iter().map(|r| (r.batch_seq, r.index)).collect::<Vec<_>>(),
        vec![(1, 0), (1, 1), (1, 2)]
    );
    assert_eq!(rig.last_acked().await, 1);
    // At-least-once: the same batch again, and an older one after a newer one.
    assert_eq!(rig.records(batch(1, rows(3, T0))).await.unwrap(), AckSeq::new(1));
    assert_eq!(
        rig.records(batch(2, rows(1, T0 + 10))).await.unwrap(),
        AckSeq::new(2)
    );
    assert_eq!(
        rig.records(batch(1, rows(3, T0))).await.unwrap(),
        AckSeq::new(2),
        "the highest so far"
    );
    assert_eq!(rig.stored_records().await.len(), 4);
    assert_eq!(rig.last_acked().await, 2);
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Resent), 6);
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Stored), 4);
}

#[tokio::test]
async fn the_primary_key_refuses_a_duplicate_even_if_the_counter_was_reset() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    let mut agent = Agent::start(&rig, &[("svc/a.yml", "v: 0")]).await;
    rig.clock.advance_secs(30);
    agent.change("svc/a.yml", "v: 1", T0 + 20 * S).await;
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    assert_eq!(rig.upgrade_audits().await.len(), 1);
    // An operator puts the counter back (a restore, a re-enrolment): the sentinel replays what it still holds.
    sqlx::query("UPDATE sentinels SET last_acked_seq = 0")
        .execute(&rig.pool)
        .await
        .unwrap();
    assert_eq!(
        rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
            .await
            .unwrap(),
        AckSeq::new(1)
    );
    assert_eq!(
        rig.stored_records().await.len(),
        1,
        "the key refused the duplicate"
    );
    assert_eq!(
        rig.upgrade_audits().await.len(),
        1,
        "and the attribution is not made twice"
    );
    assert_eq!(rig.last_acked().await, 1);
}

#[tokio::test]
async fn a_gap_is_applied_and_counted_and_a_late_batch_below_it_is_a_resend() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, rows(1, T0))).await.unwrap();
    assert_eq!(rig.metrics.sentinel_gap_count(), 0);
    // The sentinel's spool dropped batches 2 to 4: nothing the hub was given is lost, and the gap is said.
    assert_eq!(
        rig.records(batch(5, rows(2, T0 + 100))).await.unwrap(),
        AckSeq::new(5)
    );
    assert_eq!(rig.metrics.sentinel_gap_count(), 1);
    assert_eq!(rig.stored_records().await.len(), 3);
    // A batch below the counter is taken for what it is by the rule, a batch applied already (the sentinel sends in order).
    assert_eq!(
        rig.records(batch(3, rows(1, T0 + 50))).await.unwrap(),
        AckSeq::new(5)
    );
    assert_eq!(rig.stored_records().await.len(), 3);
}

#[tokio::test]
async fn a_sequence_number_of_zero_or_beyond_the_column_is_invalid_and_stores_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    // The schema's 0 is "none acknowledged": a batch numbered 0 would be taken for a resend and lost without a word.
    assert_eq!(rig.records(batch(0, rows(1, T0))).await, Err(SinkError::Invalid));
    assert_eq!(
        rig.records(batch(u64::MAX, rows(1, T0))).await,
        Err(SinkError::Invalid)
    );
    assert_eq!(
        rig.records(batch(u64::try_from(i64::MAX).unwrap() + 1, rows(1, T0)))
            .await,
        Err(SinkError::Invalid)
    );
    assert_eq!((rig.stored_records().await.len(), rig.last_acked().await), (0, 0));
    assert_eq!(
        rig.records(batch(u64::try_from(i64::MAX).unwrap(), rows(1, T0)))
            .await
            .unwrap()
            .get(),
        u64::try_from(i64::MAX).unwrap()
    );
}

#[tokio::test]
async fn two_streams_of_one_sentinel_applying_the_same_batch_at_once_store_it_once() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    let (a, b) = tokio::join!(
        rig.records(batch(1, rows(20, T0))),
        rig.records(batch(1, rows(20, T0)))
    );
    assert_eq!((a.unwrap(), b.unwrap()), (AckSeq::new(1), AckSeq::new(1)));
    assert_eq!(rig.stored_records().await.len(), 20);
    assert_eq!(rig.last_acked().await, 1);
}

// ================================================================================================ records: bounds

#[tokio::test]
async fn a_batch_over_the_record_bound_or_the_byte_bound_is_too_large_and_applies_nothing() {
    let config = RegistryConfig {
        sentinel_max_batch_bytes: 64 * 1024,
        ..RegistryConfig::default()
    };
    let Some(rig) = Rig::with("sit1", config).await else {
        return;
    };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    assert_eq!(
        rig.records(batch(1, rows(501, T0))).await,
        Err(SinkError::TooLarge)
    );
    // 80 records of the longest path a record can have: about 90 KB of text, over the 64 KiB bound.
    let long = |i: usize| {
        let mut r = record("svc/a.yml", T0);
        r.path = domain::NfsPath::parse(&format!(
            "{}/f{i}.yml",
            std::iter::repeat_n("d".repeat(200), 4)
                .collect::<Vec<_>>()
                .join("/")
        ))
        .unwrap();
        r.exe = text(&"x".repeat(200));
        r
    };
    assert_eq!(
        rig.records(batch(1, (0..80).map(long).collect())).await,
        Err(SinkError::TooLarge)
    );
    assert_eq!(
        (rig.stored_records().await.len(), rig.last_acked().await),
        (0, 0),
        "nothing of a refused batch is kept"
    );
    // The bound is on the sum, not on a record: a batch of the same records that fits is applied.
    assert_eq!(
        rig.records(batch(1, (0..20).map(long).collect())).await.unwrap(),
        AckSeq::new(1)
    );
    assert_eq!(rig.stored_records().await.len(), 20);
    // 500 records is the bound and is accepted (and an empty batch is a batch).
    assert_eq!(
        rig.records(batch(2, rows(500, T0))).await.unwrap(),
        AckSeq::new(2)
    );
    assert_eq!(rig.records(batch(3, Vec::new())).await.unwrap(), AckSeq::new(3));
    assert_eq!(
        rig.metrics
            .sentinel_message_count(SentinelKind::Records, Outcome::TooLarge),
        2
    );
}

// ================================================================================================ records: what is dropped

#[tokio::test]
async fn a_chmod_or_chown_is_acknowledged_counted_and_not_stored() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    let records = vec![
        op(record("svc/a.yml", T0), AuditOperation::Attribute),
        record("svc/b.yml", T0),
        op(record("svc/c.yml", T0), AuditOperation::Attribute),
    ];
    assert_eq!(rig.records(batch(1, records)).await.unwrap(), AckSeq::new(1));
    let stored = rig.stored_records().await;
    assert_eq!(
        stored.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
        vec!["svc/b.yml"]
    );
    assert_eq!(stored[0].index, 1, "the index is the record's place in the batch");
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Attribute), 2);
}

#[tokio::test]
async fn a_record_outside_the_swimlanes_prefix_is_dropped_counted_and_never_stored() {
    let Some(rig) = Rig::new().await else { return };
    // The VM exports /export; this swimlane's config lives at /export/sit1/csp, so a record's path starts with sit1/csp.
    rig.set_nfs_export(Some("/export/sit1/csp")).await;
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    let records = vec![
        record("sit1/csp/svc/a.yml", T0),
        record("sit2/csp/svc/a.yml", T0),
        record("sit1/other/svc/a.yml", T0),
        record("sit1/csp", T0),
        record("sit1/cspx/svc/a.yml", T0),
        record("svc/a.yml", T0),
        record("sit1/csp/deep/er/b.yml", T0),
    ];
    assert_eq!(rig.records(batch(1, records)).await.unwrap(), AckSeq::new(1));
    let stored = rig.stored_records().await;
    assert_eq!(
        stored
            .iter()
            .map(|r| (r.index, r.path.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "svc/a.yml"), (6, "deep/er/b.yml")],
        "only the swimlane's own, under its own paths"
    );
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Outside), 5);
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Stored), 2);
}

#[tokio::test]
async fn when_the_export_is_not_under_the_sentinels_root_its_paths_are_the_config_roots_own() {
    let Some(rig) = Rig::new().await else { return };
    rig.set_nfs_export(Some("/somewhere/else")).await;
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    rig.records(batch(
        1,
        vec![record("svc/a.yml", T0), record("sit1/csp/svc/a.yml", T0)],
    ))
    .await
    .unwrap();
    assert_eq!(
        rig.stored_records()
            .await
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>(),
        vec!["svc/a.yml".to_owned(), "sit1/csp/svc/a.yml".to_owned()],
        "A17: otherwise a path is already relative to the config root"
    );
}

#[tokio::test]
async fn a_sentinel_that_serves_no_swimlane_stores_nothing_and_is_still_acknowledged() {
    let Some(rig) = Rig::new().await else { return };
    rig.register_sentinel(VM, None, ROOT).await;
    rig.sentinel_hello().await.unwrap();
    assert_eq!(rig.records(batch(1, rows(4, T0))).await.unwrap(), AckSeq::new(1));
    assert_eq!(rig.stored_records().await.len(), 0);
    assert_eq!(
        rig.last_acked().await,
        1,
        "its spool is not kept for ever by a hub that cannot use it"
    );
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Unserved), 4);
}

#[tokio::test]
async fn a_swimlane_export_that_is_not_a_clean_path_maps_nothing() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    // The agent fills in an empty export: what it says is untrusted, and `..` is never resolved into another tree.
    for (n, export) in ["/export/../etc", "relative/path", "/export/a\\b", ""]
        .into_iter()
        .enumerate()
    {
        rig.set_nfs_export(Some(export)).await;
        let seq = u64::try_from(n).unwrap() + 1;
        rig.records(batch(seq, rows(2, T0))).await.unwrap();
        assert_eq!(rig.stored_records().await.len(), 0, "{export:?}");
    }
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Unserved), 8);
    assert_eq!(
        rig.last_acked().await,
        4,
        "they are acknowledged: a hub that cannot use them does not keep the sentinel's spool"
    );
}

#[tokio::test]
async fn a_time_in_the_future_is_dropped_and_an_old_one_is_stored() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    let skew = i64::try_from(RegistryConfig::default().max_clock_skew.as_millis()).unwrap();
    let records = vec![
        record("svc/a.yml", T0 + skew),
        record("svc/b.yml", T0 + skew + 1),
        record("svc/c.yml", 10),
        record("svc/d.yml", 253_402_300_799_999),
    ];
    rig.records(batch(1, records)).await.unwrap();
    let stored: Vec<String> = rig.stored_records().await.into_iter().map(|r| r.path).collect();
    assert_eq!(stored, vec!["svc/a.yml".to_owned(), "svc/c.yml".to_owned()]);
    assert_eq!(rig.metrics.sentinel_record_count(SentinelRecord::Future), 2);
}

// ================================================================================================ a sentinel serves one swimlane

#[tokio::test]
async fn a_record_attributes_in_the_swimlane_the_sentinel_serves_and_in_no_other() {
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada@example.com", "active").await;
    let mut agent = Agent::start(&rig, &[("svc/a.yml", "v: 0")]).await;
    // Another swimlane in the same database, with the same file changed at the same moment.
    rig.add_swimlane("sit2", &[], true).await;
    let other = AgentIdentity::new(SwimlaneId::parse("sit2").unwrap());
    rig.ingest.hello(&other, hello("sit2")).await.unwrap();
    rig.clock.advance_secs(30);
    agent.change("svc/a.yml", "v: 1", T0 + 20 * S).await;
    rig.ingest
        .delta(
            &other,
            delta(
                1,
                Some(root(0)),
                root(1),
                vec![entry("svc/a.yml", b"v: 1", T0 + 20 * S)],
            ),
        )
        .await
        .unwrap();
    let unknown_in_sit2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM file_observations WHERE swimlane_id = 'sit2' AND attribution_source = 'unknown'",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        unknown_in_sit2, 1,
        "the setup: the other swimlane has a change of the same path, unattributed"
    );

    rig.sentinel_hello().await.unwrap();
    rig.records(batch(1, vec![record("svc/a.yml", T0 + 10 * S)]))
        .await
        .unwrap();
    rig.clock.advance_secs(60);
    rig.sentinel.grace().run_once().await.unwrap();

    assert!(rig.attribution("svc/a.yml", 1).await.is("sentinel_login", "high"));
    let other_source: String = sqlx::query_scalar(
        "SELECT attribution_source FROM file_observations WHERE swimlane_id = 'sit2' AND path = 'svc/a.yml' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&rig.pool)
    .await
    .unwrap();
    assert_eq!(
        other_source, "unknown",
        "a sentinel names nobody in a swimlane it does not serve"
    );
    // And the upgrade it made is audited in its own swimlane only.
    let swimlanes: Vec<Option<String>> =
        sqlx::query_scalar("SELECT swimlane_id FROM audit_events WHERE action = 'attribution_upgraded'")
            .fetch_all(&rig.pool)
            .await
            .unwrap();
    assert_eq!(swimlanes, vec![Some("sit1".to_owned())]);
}

// ================================================================================================ S10

#[tokio::test]
async fn no_path_user_login_or_executable_is_in_a_log_line_an_error_or_a_metric() {
    let Some(rig) = Rig::new().await else { return };
    // The VM exports /export and this swimlane is /export/svc: a record's path starts with `svc/`.
    rig.set_nfs_export(Some("/export/svc")).await;
    rig.sentinel_for_the_swimlane().await;
    rig.add_user(1, "ada-secret@example.com", "active").await;
    let mut agent = Agent::start(
        &rig,
        &[
            ("secret-path-5521.yml", "v: 0"),
            ("secret-other-2288.yml", "v: 0"),
        ],
    )
    .await;
    let (capture, _guard) = LogCapture::install();

    let secret = |mut r: domain::AuditRecord| {
        r.login_user = text("ada_secret_example_com");
        r.effective_user = text("root-secret-7731");
        r.exe = text("/opt/secret-tool-9913/bin/vim");
        r.comm = text("secret-comm-4410");
        r
    };
    rig.clock.advance_secs(30);
    agent
        .change_all(
            &[
                ("secret-path-5521.yml", "v: 1"),
                ("secret-other-2288.yml", "v: 1"),
            ],
            T0 + 20 * S,
        )
        .await;
    rig.sentinel_hello().await.unwrap();
    // A stored and correlated record, a dropped one, a chmod, a future one, a resend, a refused batch, a refused hello and a failed write.
    let records = vec![
        secret(record("svc/secret-path-5521.yml", T0 + 10 * S)),
        secret(record("elsewhere/secret-path-5521.yml", T0 + 10 * S)),
        secret(op(
            record("svc/secret-path-5521.yml", T0 + 10 * S),
            AuditOperation::Attribute,
        )),
        secret(record("svc/secret-path-5521.yml", T0 + 3_600_000)),
    ];
    let mut errors = vec![format!("{:?}", rig.records(batch(1, records.clone())).await)];
    errors.push(format!("{:?}", rig.records(batch(1, records)).await));
    let too_many = (0..600)
        .map(|_| secret(record("svc/secret-path-5521.yml", T0)))
        .collect();
    errors.push(format!("{:?}", rig.records(batch(2, too_many)).await));
    errors.push(format!(
        "{:?}",
        rig.sentinel
            .hello(&identity(), hello_of("ada_secret_example_com", ROOT))
            .await
    ));
    rig.audit.fail_with(Some(ports::AuditError::Unavailable));
    let failing = vec![secret(record("svc/secret-other-2288.yml", T0 + 10 * S))];
    errors.push(format!("{:?}", rig.records(batch(2, failing)).await));
    rig.audit.fail_with(None);
    // The same record once the audit log is back: the batch was refused whole, so the sentinel sends it again.
    rig.clock.advance_secs(60);
    rig.sentinel.grace().run_once().await.unwrap();
    assert!(
        rig.attribution("secret-path-5521.yml", 1)
            .await
            .is("sentinel_login", "high"),
        "the setup reaches the code it tests"
    );

    let mut registry = prometheus_client::registry::Registry::default();
    rig.metrics.register(&mut registry);
    let mut metrics = String::new();
    prometheus_client::encoding::text::encode(&mut metrics, &registry).unwrap();
    let logs = capture.text();
    assert!(!logs.is_empty(), "the capture works: counts are logged");
    assert!(logs.contains("sentinel batch applied"), "{logs}");
    for planted in [
        "secret-path-5521",
        "secret-other-2288",
        "ada_secret_example_com",
        "ada-secret",
        "root-secret-7731",
        "secret-tool-9913",
        "secret-comm-4410",
        "/export/svc",
        "elsewhere",
    ] {
        assert!(!logs.contains(planted), "{planted} is in a log line: {logs}");
        assert!(!metrics.contains(planted), "{planted} is in a metric");
        for e in &errors {
            assert!(!e.contains(planted), "{planted} is in an error: {e}");
        }
    }
}

#[tokio::test]
async fn the_sink_and_the_job_never_read_a_clock_of_their_own() {
    // The clock is the registry's: moving it moves the stamps, and a record is stored with the time the hub received it.
    let Some(rig) = Rig::new().await else { return };
    rig.sentinel_for_the_swimlane().await;
    rig.sentinel_hello().await.unwrap();
    rig.clock.advance_secs(1234);
    rig.records(batch(1, rows(1, T0))).await.unwrap();
    let at: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM received_at) * 1000)::bigint FROM sentinel_records")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(at, rig.clock.now().unix_millis());
}
