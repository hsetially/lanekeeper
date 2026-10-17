//! The settings index (T2, D65, S13, S22): one set of rows for every unique structured blob, written once, off the acknowledgement
//! path, with every flagged value stored redacted. Over a real Postgres 16 as `lanekeeper_app`, with the engine as the oracle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use domain::{FileClass, NfsPath, ScanDelta};
use engine::{DenyMatcher, Limits, Syntax, classify, index_settings};
use hub_platform::jobs::{Job, JobContext, JobError};
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::IndexResult;
use hub_registry::{IndexJob, Offer, RegistryMetrics, SettingsSweeper};
use ports::{BlobStore, content_hash};
use sqlx::Row;
use support::fixtures;
use support::rig::{Rig, T, delta, entry, path, root};

// A GitHub token: `ghp_` and 36 letters and digits.
const TOKEN: &str = "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bD1fH3jL5";

fn job(p: &str, content: &str) -> IndexJob {
    IndexJob {
        hash: content_hash(content.as_bytes()),
        syntax: Syntax::of_path(&path(p)).unwrap(),
    }
}

/// Every row of the settings tables, in a fixed order, as text: what "identical tables" is compared on.
async fn dump(rig: &Rig) -> Vec<String> {
    let mut out = Vec::new();
    for (label, sql) in [
        (
            "settings_index",
            "SELECT concat_ws('|', encode(blob_hash::bytea, 'hex'), setting_path, value_text, value_type, start_line, end_line, \
                coalesce(secret_rule, '-')) FROM settings_index ORDER BY blob_hash, setting_path",
        ),
        (
            "markers",
            "SELECT concat_ws('|', encode(blob_hash::bytea, 'hex'), setting_count, parse_ok, \
                (EXTRACT(EPOCH FROM indexed_at) * 1000)::bigint) FROM settings_index_blobs ORDER BY blob_hash",
        ),
        (
            "file_state",
            "SELECT concat_ws('|', path, encode(nfs_hash::bytea, 'hex'), class, state, denied, size, \
                (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint) FROM file_state ORDER BY path",
        ),
        (
            "observations",
            "SELECT concat_ws('|', path, encode(hash::bytea, 'hex'), source, initial, \
                (EXTRACT(EPOCH FROM observed_at) * 1000)::bigint) FROM file_observations ORDER BY path, observed_at, hash",
        ),
    ] {
        let rows = sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&rig.pool)
            .await
            .unwrap();
        out.push(format!("-- {label}"));
        out.extend(rows);
    }
    out
}

async fn markers(rig: &Rig) -> BTreeSet<String> {
    sqlx::query_scalar::<_, String>("SELECT encode(blob_hash::bytea, 'hex') FROM settings_index_blobs")
        .fetch_all(&rig.pool)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

fn hex(content: &str) -> String {
    content_hash(content.as_bytes()).to_string()
}

// ------------------------------------------------------------------------------------------------ T2 verify

#[tokio::test]
async fn settings_index_row_count_matches_the_engine() {
    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    let fixtures = fixtures::small();
    let lane = &fixtures.swimlanes()[0];
    let files = fixtures.files(lane);
    rig.hello().await;
    let entries = files.iter().map(|(p, b)| entry(p.as_str(), b, T)).collect();
    rig.delta(delta(1, None, root(1), entries)).await.unwrap();
    rig.index().idle().await;

    // The oracle: the engine over the same files, one parse per distinct structured blob.
    let deny = DenyMatcher::default();
    let limits = Limits::default();
    let mut expected_rows = BTreeSet::new();
    let mut blobs: BTreeMap<String, bool> = BTreeMap::new();
    for (p, bytes) in &files {
        if classify(p, Some(bytes.as_ref()), &deny) != FileClass::Structured {
            continue;
        }
        let hash = content_hash(bytes).to_string();
        if blobs.contains_key(&hash) {
            continue;
        }
        match index_settings(bytes, Syntax::of_path(p).unwrap(), &limits) {
            Ok(index) => {
                blobs.insert(hash.clone(), true);
                for row in index.rows {
                    expected_rows.insert((
                        hash.clone(),
                        row.path.to_string(),
                        i32::try_from(row.lines.start()).unwrap(),
                        i32::try_from(row.lines.end()).unwrap(),
                    ));
                }
            }
            Err(_) => {
                blobs.insert(hash, false);
            }
        }
    }
    assert!(
        expected_rows.len() > 200 && blobs.len() > 20,
        "the fixtures must make the comparison worth something: {} rows in {} blobs",
        expected_rows.len(),
        blobs.len()
    );

    let rows: BTreeSet<(String, String, i32, i32)> = sqlx::query(
        "SELECT encode(blob_hash::bytea, 'hex'), setting_path, start_line, end_line FROM settings_index",
    )
    .fetch_all(&rig.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
    .collect();
    assert_eq!(
        rows.len(),
        expected_rows.len(),
        "T2 verify: the settings-index row count is the engine's"
    );
    assert_eq!(rows, expected_rows, "and so is every row, at its lines");

    let stored: BTreeMap<String, bool> =
        sqlx::query("SELECT encode(blob_hash::bytea, 'hex'), parse_ok FROM settings_index_blobs")
            .fetch_all(&rig.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
    assert_eq!(
        stored, blobs,
        "one marker per distinct structured blob, parse_ok as the engine says"
    );
    let counted: i64 =
        sqlx::query_scalar("SELECT coalesce(sum(setting_count), 0)::bigint FROM settings_index_blobs")
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!(usize::try_from(counted).unwrap(), expected_rows.len());
    assert_eq!(
        rig.metrics.index_rows_count(),
        expected_rows.len() as u64,
        "the counter says the same"
    );
    let unparseable = blobs.values().filter(|ok| !**ok).count() as u64;
    assert_eq!(rig.metrics.index_count(IndexResult::Unparseable), unparseable);
    assert_eq!(
        rig.metrics.index_count(IndexResult::Indexed),
        blobs.len() as u64 - unparseable
    );
}

fn deltas() -> Vec<ScanDelta> {
    let d1 = delta(
        1,
        None,
        root(1),
        vec![
            entry("a.yml", b"x: 1\ny: 2\n", T),
            entry("svc/b.yml", b"name: b\nport: 80\n", T),
            entry("c.properties", b"k=v\nz=1\n", T),
            entry("notes.txt", b"not structured", T),
            entry("broken.yml", b"a: [unclosed\n", T),
        ],
    );
    let d2 = delta(
        2,
        Some(root(1)),
        root(2),
        vec![
            entry("a.yml", b"x: 1\ny: 3\nw: 4\n", T + 10_000),
            entry("d.json", b"{\"k\": {\"n\": 1}}", T + 10_000),
        ],
    );
    let d3 = ScanDelta {
        removed: vec![path("svc/b.yml")],
        ..delta(
            3,
            Some(root(2)),
            root(3),
            vec![entry("c.properties", b"k=w\nz=1\nq=9\n", T + 20_000)],
        )
    };
    vec![d1, d2, d3]
}

#[tokio::test]
async fn replaying_the_same_deltas_twice_gives_identical_tables() {
    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    rig.hello().await;
    for d in deltas() {
        rig.delta(d).await.unwrap();
    }
    rig.index().idle().await;
    let first = dump(&rig).await;
    let indexed = rig.metrics.index_count(IndexResult::Indexed);
    let rows = rig.metrics.index_rows_count();
    assert!(
        indexed >= 5 && rows >= 8,
        "the first run indexed something: {indexed} blobs, {rows} rows"
    );

    // The agent reconnects and sends everything again (a spool replay).
    rig.hello().await;
    for d in deltas() {
        rig.delta(d).await.unwrap();
    }
    rig.index().idle().await;
    assert_eq!(
        dump(&rig).await,
        first,
        "T2 verify: the same deltas twice leave the same tables"
    );
    assert_eq!(
        rig.metrics.index_count(IndexResult::Indexed),
        indexed,
        "nothing was parsed again"
    );
    assert_eq!(rig.metrics.index_rows_count(), rows);

    // And so does a sweep over a database that is already right.
    let (sweeping, _worker) = rig.another_indexer(RegistryConfig::default(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(sweeping).sweep(|| Ok(())).await.unwrap();
    assert_eq!(swept.found, 0, "every current structured blob has its marker");
    assert_eq!(dump(&rig).await, first);
}

#[tokio::test]
async fn two_runs_of_the_same_deltas_give_the_same_index() {
    let Some(one) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    let Some(two) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    for (n, rig) in [&one, &two].into_iter().enumerate() {
        rig.hello().await;
        for d in deltas() {
            rig.delta(d).await.unwrap();
            // Different batching and timing must not change what is written: the second run waits for the index after each delta.
            if n == 1 {
                rig.index().idle().await;
            }
        }
        rig.index().idle().await;
    }
    assert_eq!(dump(&one).await, dump(&two).await);
}

#[tokio::test]
async fn a_blob_is_indexed_once_by_two_replicas() {
    let Some(rig) = Rig::new().await else { return };
    let contents: Vec<String> = (0..24)
        .map(|i| format!("a: {i}\nb:\n  c: {i}\n  d: x{i}\n"))
        .collect();
    for c in &contents {
        rig.blobs.put(Bytes::from(c.clone())).await.unwrap();
    }
    let jobs: Vec<IndexJob> = contents.iter().map(|c| job("x.yml", c)).collect();
    let (ma, mb) = (RegistryMetrics::default(), RegistryMetrics::default());
    let (a, _wa) = rig.another_indexer(RegistryConfig::default(), ma.clone());
    let (b, _wb) = rig.another_indexer(RegistryConfig::default(), mb.clone());
    let (ra, rb) = tokio::join!(a.index_now(jobs.clone()), b.index_now(jobs.clone()));
    let (ra, rb) = (ra.unwrap(), rb.unwrap());

    assert_eq!(
        ra.indexed + rb.indexed,
        24,
        "each blob has exactly one winner: {ra:?} {rb:?}"
    );
    assert_eq!(ra.indexed + ra.already_indexed, 24);
    assert_eq!(rb.indexed + rb.already_indexed, 24);
    assert_eq!(
        rig.count("SELECT count(*) FROM settings_index_blobs").await,
        24,
        "one marker per blob"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM settings_index").await,
        24 * 3,
        "one set of rows per blob, three settings each, whoever wrote them"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM (SELECT blob_hash, setting_path FROM settings_index GROUP BY 1, 2 HAVING count(*) > 1) d")
            .await,
        0
    );
    assert_eq!(
        ma.index_count(IndexResult::Indexed) + mb.index_count(IndexResult::Indexed),
        24,
        "and the counters of the two replicas add up to once"
    );
}

// ------------------------------------------------------------------------------------------------ S13

#[tokio::test]
async fn flagged_values_are_stored_redacted() {
    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    let key = "key: |\n  -----BEGIN RSA PRIVATE KEY-----\n  MIIEowIBAAKCAQEAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n  -----END RSA PRIVATE KEY-----\n";
    let secrets = format!("api:\n  url: http://x\n  token: {TOKEN}\n{key}other: 1\n");
    rig.first_scan(&[("secrets.yml", secrets.as_str()), ("plain.yml", "a: 1\n")])
        .await;
    rig.index().idle().await;

    let row = |p: &str| {
        let pool = rig.pool.clone();
        let p = p.to_owned();
        async move {
            sqlx::query("SELECT value_text, secret_rule FROM settings_index WHERE setting_path = $1")
                .bind(p)
                .fetch_one(&pool)
                .await
                .map(|r| (r.get::<String, _>(0), r.get::<Option<String>, _>(1)))
                .unwrap()
        }
    };
    assert_eq!(
        row("api.token").await,
        (
            "[REDACTED: 40 chars, github_token]".to_owned(),
            Some("github_token".to_owned())
        )
    );
    let (key_value, key_rule) = row("key").await;
    assert_eq!(key_rule.as_deref(), Some("private_key"));
    assert!(key_value.starts_with("[REDACTED: "), "{key_value}");
    assert_eq!(
        row("api.url").await,
        ("http://x".to_owned(), None),
        "a neighbour is not touched"
    );
    assert_eq!(row("other").await, ("1".to_owned(), None));

    // The secret is nowhere in either settings table, in any column.
    for table in ["settings_index", "settings_index_blobs"] {
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} t WHERE t::text LIKE '%' || $1 || '%' OR t::text LIKE '%MIIEow%'"
        ))
        .bind(TOKEN)
        .fetch_one(&rig.pool)
        .await
        .unwrap();
        assert_eq!(n, 0, "{table} holds a secret");
    }
    assert_eq!(rig.metrics.index_redacted_count(), 2);
    // The blob itself is the file, as it was written: revealing reads it, never the index (D79).
    assert!(rig.blobs.has(&content_hash(secrets.as_bytes())).await);
}

// ------------------------------------------------------------------------------------------------ what is marked

#[tokio::test]
async fn an_unparseable_blob_is_marked_and_parsed_once() {
    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    rig.first_scan(&[("bad.yml", "a: [unclosed\n"), ("good.yml", "a: 1\n")])
        .await;
    rig.index().idle().await;
    let marker =
        sqlx::query("SELECT setting_count, parse_ok FROM settings_index_blobs WHERE blob_hash = $1::bytea")
            .bind(content_hash(b"a: [unclosed\n").as_bytes().as_slice())
            .fetch_one(&rig.pool)
            .await
            .unwrap();
    assert_eq!((marker.get::<i32, _>(0), marker.get::<bool, _>(1)), (0, false));
    assert_eq!(rig.metrics.index_count(IndexResult::Unparseable), 1);

    // A sweep does not look at it again: it has its marker.
    let (indexer, _w) = rig.another_indexer(RegistryConfig::default(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(indexer).sweep(|| Ok(())).await.unwrap();
    assert_eq!(swept.found, 0);
    assert_eq!(
        rig.metrics.index_count(IndexResult::Unparseable),
        1,
        "parsed once and not again"
    );
    assert_eq!(rig.count("SELECT count(*) FROM settings_index").await, 1);
}

#[tokio::test]
async fn an_empty_structured_file_is_marked_with_no_rows() {
    let Some(rig) = Rig::with_index("sit1", RegistryConfig::default()).await else {
        return;
    };
    rig.first_scan(&[("empty.yml", ""), ("comment.yml", "# nothing here\n")])
        .await;
    rig.index().idle().await;
    assert_eq!(
        rig.count("SELECT count(*) FROM settings_index_blobs WHERE parse_ok AND setting_count = 0")
            .await,
        2
    );
    assert_eq!(rig.count("SELECT count(*) FROM settings_index").await, 0);
}

// ------------------------------------------------------------------------------------------------ off the ack path

#[tokio::test]
async fn the_acknowledgement_does_not_wait_for_the_index_and_the_sweeper_catches_what_was_dropped() {
    // The queue holds two hashes, and nothing drains it: ingestion must still acknowledge every delta.
    let Some(rig) = Rig::new().await else { return };
    let config = RegistryConfig {
        index_queue: 2,
        ..RegistryConfig::default()
    };
    let metrics = RegistryMetrics::default();
    let (indexer, worker) = rig.another_indexer(config, metrics.clone());
    assert!(rig.ingest.attach_settings_index(indexer.clone()).is_ok());
    assert!(
        rig.ingest.attach_settings_index(indexer.clone()).is_err(),
        "an indexer is attached once"
    );

    let files: Vec<(String, String)> = (0..8)
        .map(|i| (format!("f{i}.yml"), format!("n: {i}\n")))
        .collect();
    rig.hello().await;
    let entries = files.iter().map(|(p, c)| entry(p, c.as_bytes(), T)).collect();
    rig.delta(delta(1, None, root(1), entries)).await.unwrap();

    assert_eq!(indexer.pending(), 2, "the queue holds what it can");
    assert_eq!(
        metrics.index_count(IndexResult::Dropped),
        6,
        "and counts what it cannot"
    );
    assert_eq!(
        rig.count("SELECT count(*) FROM settings_index_blobs").await,
        0,
        "nothing is indexed yet"
    );
    assert_eq!(
        rig.state("f0.yml").await.unwrap().class,
        "structured",
        "the delta itself was applied"
    );

    // The sweeper finds all eight, whatever the queue dropped.
    let swept = SettingsSweeper::new(indexer.clone())
        .sweep(|| Ok(()))
        .await
        .unwrap();
    assert_eq!((swept.found, swept.indexed), (8, 8));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 8);

    // When the worker finally runs, the two it had are already indexed: no second parse.
    tokio::spawn(worker.run());
    indexer.idle().await;
    assert_eq!(metrics.index_count(IndexResult::Indexed), 8);
    assert_eq!(metrics.index_count(IndexResult::AlreadyIndexed), 2);
    assert_eq!(rig.count("SELECT count(*) FROM settings_index").await, 8);
}

#[tokio::test]
async fn a_hash_that_is_already_waiting_is_not_queued_twice() {
    let Some(rig) = Rig::new().await else { return };
    let metrics = RegistryMetrics::default();
    let (indexer, worker) = rig.another_indexer(RegistryConfig::default(), metrics.clone());
    let j = job("a.yml", "a: 1\n");
    rig.blobs.put(Bytes::from_static(b"a: 1\n")).await.unwrap();
    assert_eq!(indexer.offer(j), Offer::Queued);
    assert_eq!(indexer.offer(j), Offer::Duplicate);
    assert_eq!(indexer.pending(), 1);
    tokio::spawn(worker.run());
    indexer.idle().await;
    assert_eq!(indexer.pending(), 0);
    assert_eq!(metrics.index_count(IndexResult::Indexed), 1);
    assert_eq!(metrics.index_count(IndexResult::Duplicate), 1);
    // Once it is done it can be offered again, and costs nothing.
    assert_eq!(indexer.offer(j), Offer::Queued);
    indexer.idle().await;
    assert_eq!(metrics.index_count(IndexResult::AlreadyIndexed), 1);
}

#[tokio::test]
async fn a_blob_the_store_does_not_have_gets_no_marker() {
    let Some(rig) = Rig::new().await else { return };
    let metrics = RegistryMetrics::default();
    let (indexer, _w) = rig.another_indexer(RegistryConfig::default(), metrics.clone());
    let report = indexer
        .index_now(vec![job("a.yml", "never stored: 1\n")])
        .await
        .unwrap();
    assert_eq!((report.missing, report.indexed), (1, 0));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 0);

    // A store that fails fails the batch, and again leaves no marker: the sweeper tries again.
    rig.blobs.put(Bytes::from_static(b"a: 1\n")).await.unwrap();
    rig.blobs.set_failing_reads(true);
    assert!(indexer.index_now(vec![job("a.yml", "a: 1\n")]).await.is_err());
    rig.blobs.set_failing_reads(false);
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 0);
    let retried = indexer.index_now(vec![job("a.yml", "a: 1\n")]).await.unwrap();
    assert_eq!(retried.indexed, 1, "and the retry works");
}

// ------------------------------------------------------------------------------------------------ the sweeper

#[tokio::test]
async fn the_sweeper_indexes_only_current_structured_blobs_of_readable_files() {
    let Some(rig) = Rig::new().await else { return };
    rig.first_scan(&[
        ("a.yml", "kept: 1\n"),
        ("img.png", "\u{1}PNG-not-structured"),
        ("tls/server.key", "key material"),
        ("gone.yml", "removed: 1\n"),
        ("moves.yml", "old: 1\n"),
        ("notes.txt", "text"),
    ])
    .await;
    // gone.yml disappears, moves.yml changes: its old blob is a history version, no longer current.
    let d2 = ScanDelta {
        removed: vec![path("gone.yml")],
        ..delta(
            2,
            Some(root(1)),
            root(2),
            vec![entry("moves.yml", b"new: 2\n", T + 10_000)],
        )
    };
    rig.delta(d2).await.unwrap();

    let (indexer, _w) = rig.another_indexer(RegistryConfig::default(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(indexer).sweep(|| Ok(())).await.unwrap();
    assert_eq!((swept.found, swept.indexed, swept.truncated), (2, 2, false));
    assert_eq!(
        markers(&rig).await,
        BTreeSet::from([hex("kept: 1\n"), hex("new: 2\n")]),
        "a.yml and the current moves.yml; not the png, the denied key, the removed file, the text file or the old version"
    );
    assert_eq!(rig.count("SELECT count(*) FROM settings_index").await, 2);
}

#[tokio::test]
async fn the_sweeper_is_bounded_and_goes_on_where_it_stopped() {
    let Some(rig) = Rig::new().await else { return };
    let files: Vec<(String, String)> = (0..7)
        .map(|i| (format!("f{i}.yml"), format!("n: {i}\n")))
        .collect();
    rig.hello().await;
    let entries = files.iter().map(|(p, c)| entry(p, c.as_bytes(), T)).collect();
    rig.delta(delta(1, None, root(1), entries)).await.unwrap();

    let config = RegistryConfig {
        index_batch: 2,
        index_sweep_max_blobs: 3,
        ..RegistryConfig::default()
    };
    let (indexer, _w) = rig.another_indexer(config, RegistryMetrics::default());
    let sweeper = SettingsSweeper::new(indexer);
    let first = sweeper.sweep(|| Ok(())).await.unwrap();
    assert!(first.truncated, "{first:?}");
    assert_eq!(first.indexed, 4, "whole batches, up to the bound: 2 then 2");
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 4);

    let mut runs = 1;
    loop {
        runs += 1;
        let next = sweeper.sweep(|| Ok(())).await.unwrap();
        if !next.truncated {
            break;
        }
        assert!(runs < 10, "the sweep must finish");
    }
    assert_eq!(
        rig.count("SELECT count(*) FROM settings_index_blobs").await,
        7,
        "all of them, after a few runs"
    );
}

#[tokio::test]
async fn the_sweeper_stops_when_the_lease_is_lost() {
    let Some(rig) = Rig::new().await else { return };
    let files: Vec<(String, String)> = (0..5)
        .map(|i| (format!("f{i}.yml"), format!("n: {i}\n")))
        .collect();
    rig.hello().await;
    let entries = files.iter().map(|(p, c)| entry(p, c.as_bytes(), T)).collect();
    rig.delta(delta(1, None, root(1), entries)).await.unwrap();
    let config = RegistryConfig {
        index_batch: 2,
        ..RegistryConfig::default()
    };
    let (indexer, _w) = rig.another_indexer(config, RegistryMetrics::default());
    let sweeper = SettingsSweeper::new(indexer);

    // Lost before the first unit of work: nothing is written.
    let lost = sweeper.sweep(|| Err(JobError::LeaseLost)).await;
    assert_eq!(lost, Err(JobError::LeaseLost));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 0);

    // Lost after one batch: that batch is written, the rest is not.
    let asked = AtomicUsize::new(0);
    let lost = sweeper
        .sweep(|| {
            // 1: before the swimlane list, 2: before the first page, 3: before the second page.
            if asked.fetch_add(1, Ordering::SeqCst) >= 2 {
                Err(JobError::LeaseLost)
            } else {
                Ok(())
            }
        })
        .await;
    assert_eq!(lost, Err(JobError::LeaseLost));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 2);

    // As a job with a context, whose lease ends: the same.
    let (ctx, expiry) = JobContext::held_until_told_otherwise("registry-index-sweeper");
    expiry.send(tokio::time::Instant::now()).unwrap();
    assert_eq!(sweeper.run(&ctx).await, Err(JobError::LeaseLost));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 2);

    // And as a job that holds its lease, it finishes the work.
    let (ctx, _expiry) = JobContext::held_until_told_otherwise("registry-index-sweeper");
    assert_eq!(sweeper.run(&ctx).await, Ok(()));
    assert_eq!(rig.count("SELECT count(*) FROM settings_index_blobs").await, 5);
}

#[tokio::test]
async fn the_sweeper_covers_every_swimlane() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_swimlane("sit2", &[], true).await;
    rig.first_scan(&[("a.yml", "one: 1\n")]).await;
    // A second swimlane's file, written straight into file_state as ingestion would leave it.
    rig.blobs.put(Bytes::from_static(b"two: 2\n")).await.unwrap();
    sqlx::query(
        "INSERT INTO file_state (swimlane_id, path, logical_file, kind, class, nfs_hash, state) \
         VALUES ('sit2', 'b.yml', 'b.yml', 'base', 'structured', $1::bytea, 'unknown')",
    )
    .bind(content_hash(b"two: 2\n").as_bytes().as_slice())
    .execute(&rig.pool)
    .await
    .unwrap();
    let (indexer, _w) = rig.another_indexer(RegistryConfig::default(), RegistryMetrics::default());
    let swept = SettingsSweeper::new(indexer).sweep(|| Ok(())).await.unwrap();
    assert_eq!((swept.found, swept.indexed), (2, 2));
}

#[test]
fn the_fixture_paths_used_here_are_valid() {
    // A guard for the helpers: a typo in a path would make a test pass for the wrong reason.
    for p in ["a.yml", "svc/b.yml", "c.properties", "d.json", "tls/server.key"] {
        assert!(NfsPath::parse(p).is_ok(), "{p}");
    }
}
