//! What a real collection does to the reads (T9, D116): a blob that is gone from the store is a clean `NotFound` for the one version that
//! named it, and never a failure for a read of anything else, a text search, or a shard that was being built.
//!
//! The blobs here are deleted by the retention job itself (the same plan, lock and delete the hub runs), not by a `DELETE` of the test: the
//! job is what decides which blob is gone, so a read that depended on a blob the job takes would fail here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::time::Duration;

use domain::{ContentHash, ContentSource, SettingsQuery, ShortText, TextQuery};
use hub_registry::config::RegistryConfig;
use hub_registry::{RegistryMetrics, RegistryService, Retention};
use ports::{ReadError, RegistryRead, content_hash};
use support::reads::{Reads, lane, nfs, page};
use support::retention::retention_of;

const OLD: &str = "Server:\n  Port: 8080\n  needle: old version\n";
const NEW: &str = "Server:\n  Port: 8081\n  needle: new version\n";

fn config() -> RegistryConfig {
    RegistryConfig {
        search_shards: 4,
        ..RegistryConfig::default()
    }
}

fn text(pattern: &str) -> TextQuery {
    TextQuery {
        pattern: pattern.to_owned(),
        regex: false,
        case_sensitive: false,
        swimlanes: Vec::new(),
        path_glob: None,
        max_total: 500,
        max_per_file: 20,
    }
}

fn settings(q: &str) -> SettingsQuery {
    SettingsQuery {
        path_contains: ShortText::parse(q).unwrap(),
        swimlanes: Vec::new(),
        file: None,
        page: page(100),
    }
}

/// The paths and lines a text search finds, in the order it gives them.
async fn found(service: &RegistryService, pattern: &str) -> Vec<(String, String)> {
    service
        .search_text(&Reads::viewer(), text(pattern))
        .await
        .expect("the search answers")
        .hits
        .iter()
        .map(|h| (h.path.to_string(), h.text.trim().to_owned()))
        .collect()
}

/// `sit1` with three files, `a.yml` at its old content, and the settings index current.
async fn world() -> Option<Reads> {
    let mut r = Reads::with(&["sit1"], config()).await?;
    r.world
        .inventory(&[
            ("a.yml", OLD),
            ("b.yml", "Server:\n  Port: 8082\n  needle: b\n"),
            ("c.yml", "needle: c\n"),
        ])
        .await;
    r.index_all().await;
    Some(r)
}

/// `a.yml` changes to its new content, the settings index follows, and the old version becomes what the job may collect: the rules keep one
/// version and one day, the registry's clock is ten days on, and the old blob was last stored a hundred days ago. Returns its hash. The
/// settings index follows when `reindex` (its worker reads blobs, so it cannot run while the store holds its readers).
async fn the_file_changes_and_its_old_version_becomes_collectable(
    r: &mut Reads,
    reindex: bool,
) -> ContentHash {
    r.world.nfs_change(&[("a.yml", NEW)]).await;
    r.sql("UPDATE swimlane_registry SET version = version + 1").await;
    if reindex {
        r.index_all().await;
    }
    let old = content_hash(OLD.as_bytes());
    r.sql("UPDATE retention_settings SET keep_versions = 1, keep_days = 1")
        .await;
    r.world.rig.clock.advance_secs(10 * 86_400);
    r.sql(&format!(
        "UPDATE blobs SET touched_at = now() - interval '100 days' WHERE hash = '\\x{}'",
        hex::encode(old.as_bytes())
    ))
    .await;
    old
}

fn job_of(r: &Reads) -> Retention {
    retention_of(
        &r.world.rig,
        RegistryConfig::default(),
        RegistryMetrics::default(),
    )
}

async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..1500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}");
}

/// A search stands between "these are the blobs I need" and "read them" (the store holds its readers at a gate) when the job deletes one
/// of them: the old version of `a.yml`, which the search's file list names because the file had not changed yet when the list was read.
/// The store then answers without it. The search still answers, with what it could read; and the next search, at the swimlane's new
/// version, finds the file's new content.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_search_that_is_fetching_a_blob_that_a_real_collection_deletes_still_answers() {
    let Some(mut r) = world().await else {
        return;
    };
    let job = job_of(&r);
    let blobs = std::sync::Arc::clone(&r.world.rig.blobs);

    blobs.pause_reads(true);
    let searching = {
        let service = r.service.clone();
        tokio::spawn(async move { service.search_text(&Reads::viewer(), text("needle")).await })
    };
    until("the search is waiting for its blobs", || {
        blobs.paused_readers() > 0
    })
    .await;
    assert!(!searching.is_finished());

    let old = the_file_changes_and_its_old_version_becomes_collectable(&mut r, false).await;
    let report = job.run_once().await.unwrap();
    assert_eq!(report.deleted_blobs, 1, "the job collected the old version");
    assert!(!blobs.has(&old).await, "and it is gone from the store");

    blobs.pause_reads(false);
    let during = searching
        .await
        .unwrap()
        .expect("a blob that the store no longer has does not make the search fail");
    let mut paths: Vec<String> = during.hits.iter().map(|h| h.path.to_string()).collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["b.yml", "c.yml"],
        "the search left out the blob that was gone, with its places, and found the rest"
    );

    // The swimlane has a new version, so the next search builds from it: the file's new content is found.
    let after = found(&r.service, "needle").await;
    let mut lines: Vec<String> = after.iter().map(|(p, t)| format!("{p}: {t}")).collect();
    lines.sort();
    assert_eq!(
        lines,
        vec![
            "a.yml: needle: new version",
            "b.yml: needle: b",
            "c.yml: needle: c"
        ]
    );
}

/// With no race at all: after a real collection a cold service (nothing cached) reads everything as it did before, and the version that was
/// collected is a clean `NotFound` while the history still lists it.
#[tokio::test]
async fn after_a_real_collection_everything_reads_as_before_and_the_collected_version_is_not_found() {
    let Some(mut r) = world().await else {
        return;
    };
    let job = job_of(&r);
    let old = the_file_changes_and_its_old_version_becomes_collectable(&mut r, true).await;
    let v = Reads::viewer();
    let sit1 = lane("sit1");
    let a = nfs("a.yml");

    // Before the collection: the old version reads, and the settings index holds its settings.
    let before_content = r
        .service
        .content(&v, &sit1, &a, ContentSource::Blob { hash: old })
        .await
        .unwrap();
    assert_eq!(before_content.bytes.as_deref(), Some(OLD.as_bytes()));
    let old_rows = || async {
        r.world
            .rig
            .count(&format!(
                "SELECT count(*) FROM settings_index WHERE blob_hash = '\\x{}'",
                hex::encode(old.as_bytes())
            ))
            .await
    };
    assert!(old_rows().await > 0, "the old version's settings are indexed");
    let cold = r.service_with(config());
    let text_before = found(&cold, "needle").await;
    let settings_before = cold.search_settings(&v, settings("port")).await.unwrap();
    let history_before = r.service.history(&v, &sit1, &a, page(50)).await.unwrap();

    let report = job.run_once().await.unwrap();
    assert_eq!(report.deleted_blobs, 1);

    assert_eq!(
        r.service
            .content(&v, &sit1, &a, ContentSource::Blob { hash: old })
            .await
            .unwrap_err(),
        ReadError::NotFound,
        "the version whose blob is gone is not found, and not a failure"
    );
    let current = r
        .service
        .content(&v, &sit1, &a, ContentSource::Nfs)
        .await
        .unwrap();
    assert_eq!(current.bytes.as_deref(), Some(NEW.as_bytes()));
    assert_eq!(old_rows().await, 0, "its settings went with it (cascade)");

    let cold = r.service_with(config());
    assert_eq!(
        found(&cold, "needle").await,
        text_before,
        "a cold text search finds what it found"
    );
    assert_eq!(
        cold.search_settings(&v, settings("port")).await.unwrap().items,
        settings_before.items,
        "and so does the settings search, which only ever answers from the current versions"
    );
    let history_after = r.service.history(&v, &sit1, &a, page(50)).await.unwrap();
    assert_eq!(
        history_after.items, history_before.items,
        "the history still lists both versions: it is made of rows, and the audit chain keeps its own record"
    );
    assert_eq!(history_after.items.len(), 2);
}
