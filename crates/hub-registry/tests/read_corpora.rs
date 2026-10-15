//! The text corpora and their warm-up (T6, P7, `read::corpora`, `read::warmup`): a change to one swimlane makes again only the shards
//! it touches and reads only the blobs it brings; a search never sees an older version than the swimlane's current one, with or without
//! the warm-up having run; the warm-up builds without a search, follows changes, survives a failing store and stops when asked; a
//! snapshot over its bound is made for the request and not kept. Over a real Postgres.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::time::{Duration, Instant};

use domain::{TextQuery, TextResults};
use hub_registry::RegistryService;
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::{CacheName, CacheResult};
use ports::{BlobStore, ReadError, RegistryRead};
use support::reads::{Reads, lane};

const SHARDS: usize = 4;

fn config() -> RegistryConfig {
    RegistryConfig {
        search_shards: SHARDS,
        warmup_poll: Duration::from_millis(100),
        warmup_debounce: Duration::ZERO,
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

/// `sit1` with twelve files and two of its own, `sit2` (eight of the same contents and four of its own) and `sit3` (four and four):
/// 34 files, each with one line that has the word `needle`.
async fn world(config: RegistryConfig) -> Option<Reads> {
    let mut r = Reads::with(&["sit1"], config).await?;
    let own = |lane: &str, n: usize| -> Vec<(String, String)> {
        (0..n)
            .map(|i| (format!("{lane}-own{i}.yml"), format!("needle own {lane} {i}\n")))
            .collect()
    };
    let shared = |n: usize| -> Vec<(String, String)> {
        (0..n)
            .map(|i| (format!("f{i}.yml"), format!("needle shared {i}\n")))
            .collect()
    };
    let mut sit1 = shared(12);
    sit1.extend(own("sit1", 2));
    let mut sit2 = shared(8);
    sit2.extend(own("sit2", 4));
    let mut sit3 = shared(4);
    sit3.extend(own("sit3", 4));
    r.world.rig.add_swimlane("sit2", &["sit2"], false).await;
    r.world.rig.add_swimlane("sit3", &["sit3"], false).await;
    let sit1_refs: Vec<(&str, &str)> = sit1.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let sit2_refs: Vec<(&str, &str)> = sit2.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let sit3_refs: Vec<(&str, &str)> = sit3.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    r.world.inventory(&sit1_refs).await;
    r.world.second_lane("sit2", &["sit2"], &sit2_refs).await;
    r.world.second_lane("sit3", &["sit3"], &sit3_refs).await;
    Some(r)
}

fn counts(r: &Reads, name: CacheName) -> (u64, u64) {
    let m = &r.world.rig.metrics;
    (
        m.read_cache_count(name, CacheResult::Hit),
        m.read_cache_count(name, CacheResult::Miss),
    )
}

async fn search(service: &RegistryService, pattern: &str) -> TextResults {
    service
        .search_text(&Reads::viewer(), text(pattern))
        .await
        .unwrap()
}

fn found(results: &TextResults) -> Vec<(String, String, String)> {
    results
        .hits
        .iter()
        .map(|h| (h.swimlane.to_string(), h.path.to_string(), h.text.clone()))
        .collect()
}

async fn until_current(service: &RegistryService) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !service.text_corpora_are_current().await.unwrap() {
        assert!(Instant::now() < deadline, "the corpora did not catch up in 20 s");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_change_to_one_swimlane_makes_again_only_the_shards_it_touches_and_reads_only_the_blobs_it_brings()
{
    let Some(mut r) = world(config()).await else {
        return;
    };
    let first = search(&r.service, "needle").await;
    assert_eq!(first.hits.len(), 34, "every file of every swimlane has one hit");
    let (_, made) = counts(&r, CacheName::CorpusShard);
    assert_eq!(made, SHARDS as u64, "the first build makes every shard");
    let (hits_before, _) = counts(&r, CacheName::CorpusShard);

    r.world.rig.blobs.forget_requests();
    r.world
        .nfs_change(&[("f3.yml", "needle changed in sit1 only\n")])
        .await;
    let after = search(&r.service, "needle").await;
    let (hits_after, made_after) = counts(&r, CacheName::CorpusShard);
    let remade = made_after - made;
    assert!(
        (1..=2).contains(&remade),
        "the shard the old content left and the one the new came to, no more: {remade} of {SHARDS}"
    );
    assert_eq!(
        hits_after - hits_before,
        SHARDS as u64 - remade,
        "the others were kept"
    );
    let requested = r.world.rig.blobs.requested();
    assert_eq!(
        requested,
        vec![ports::content_hash(b"needle changed in sit1 only\n")],
        "only the new blob was read from the store: the shards had the others"
    );
    // The answer is the one a service that builds everything from nothing gives.
    let fresh = r.service_with(config());
    assert_eq!(after, search(&fresh, "needle").await);
    let sit1_f3: Vec<_> = found(&after)
        .into_iter()
        .filter(|(l, p, _)| l == "sit1" && p == "f3.yml")
        .collect();
    assert_eq!(
        sit1_f3,
        [(
            "sit1".into(),
            "f3.yml".into(),
            "needle changed in sit1 only".into()
        )]
    );
    // The other swimlanes still have f3's old content.
    assert!(found(&after).contains(&("sit2".into(), "f3.yml".into(), "needle shared 3".into())));
    // A swimlane whose version moved and whose files did not makes no shard again.
    let (_, made_now) = counts(&r, CacheName::CorpusShard);
    r.sql("UPDATE swimlane_registry SET version = version + 1").await;
    assert_eq!(search(&r.service, "needle").await, after);
    assert_eq!(
        counts(&r, CacheName::CorpusShard).1,
        made_now,
        "same files, no shard made"
    );
}

#[tokio::test]
async fn a_search_sees_the_new_version_and_not_the_old_one_whether_or_not_the_warm_up_has_run() {
    for warmed in [false, true] {
        let Some(mut r) = world(config()).await else {
            return;
        };
        let task = warmed.then(|| r.service.spawn_warmup());
        if warmed {
            until_current(&r.service).await;
        } else {
            search(&r.service, "needle").await;
        }
        for round in 0..4 {
            let content = format!("zebra {round} stripe\n");
            let old = if round == 0 {
                "needle shared 5".to_owned()
            } else {
                format!("zebra {} stripe", round - 1)
            };
            r.world.nfs_change(&[("f5.yml", &content)]).await;
            // At once: the warm-up may not have looked yet, may be in the middle of its build, or may have finished.
            let found_now = search(&r.service, &format!("zebra {round}")).await;
            let in_sit1: Vec<_> = found(&found_now)
                .into_iter()
                .filter(|(l, p, _)| l == "sit1" && p == "f5.yml")
                .collect();
            assert_eq!(
                in_sit1.len(),
                1,
                "warmed {warmed}, round {round}: the new content is found: {:?}",
                found(&found_now)
            );
            let gone = search(&r.service, &old).await;
            assert!(
                !found(&gone).iter().any(|(l, p, _)| l == "sit1" && p == "f5.yml"),
                "warmed {warmed}, round {round}: the old content is not: {:?}",
                found(&gone)
            );
            // The other swimlanes' f5 keep their content (round 0: the shared one).
            if round == 0 {
                assert!(found(&gone).contains(&("sit2".into(), "f5.yml".into(), "needle shared 5".into())));
            }
        }
        if let Some(task) = task {
            until_current(&r.service).await;
            let (_, misses) = counts(&r, CacheName::Corpus);
            search(&r.service, "needle").await;
            assert_eq!(
                counts(&r, CacheName::Corpus).1,
                misses,
                "a settled warm-up: the search needs no build"
            );
            task.shutdown().await;
        }
    }
}

#[tokio::test]
async fn the_warm_up_builds_without_a_search_follows_changes_and_stops_when_asked() {
    let Some(mut r) = world(config()).await else {
        return;
    };
    assert!(
        !r.service.text_corpora_are_current().await.unwrap(),
        "nothing is built before the warm-up or a search"
    );
    let task = r.service.spawn_warmup();
    until_current(&r.service).await;
    r.world.rig.blobs.forget_requests();
    let (hits, misses) = counts(&r, CacheName::Corpus);
    assert_eq!(misses, 1, "the warm-up made the snapshot");
    search(&r.service, "needle").await;
    assert_eq!(
        counts(&r, CacheName::Corpus),
        (hits + 1, misses),
        "the first search is a warm one"
    );
    assert_eq!(r.world.rig.blobs.many_calls(), 0, "and reads no blob");

    // A change: the warm-up takes it in, the shards it touches only, with no search asking.
    let (_, made) = counts(&r, CacheName::CorpusShard);
    r.world.nfs_change(&[("f1.yml", "needle changed once\n")]).await;
    assert!(
        !r.service.text_corpora_are_current().await.unwrap(),
        "the change is not in the snapshot yet"
    );
    until_current(&r.service).await;
    let remade = counts(&r, CacheName::CorpusShard).1 - made;
    assert!((1..=2).contains(&remade), "{remade} shards made again");
    assert_eq!(
        r.world.rig.blobs.requested(),
        vec![ports::content_hash(b"needle changed once\n")]
    );
    let (_, misses) = counts(&r, CacheName::Corpus);
    let hits_now = found(&search(&r.service, "changed once").await);
    assert_eq!(
        hits_now,
        [(
            "sit1".to_owned(),
            "f1.yml".to_owned(),
            "needle changed once".to_owned()
        )]
    );
    assert_eq!(counts(&r, CacheName::Corpus).1, misses, "a warm search");

    // Stopped, it does not follow any more.
    task.shutdown().await;
    r.world.nfs_change(&[("f1.yml", "needle changed twice\n")]).await;
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(
        !r.service.text_corpora_are_current().await.unwrap(),
        "nobody is following"
    );
    let found_now = found(&search(&r.service, "changed twice").await);
    assert_eq!(
        found_now.len(),
        1,
        "a search builds for itself when there is no warm-up"
    );
}

#[tokio::test]
async fn a_warm_up_that_cannot_read_the_blobs_keeps_trying_and_searches_build_for_themselves() {
    let Some(r) = world(config()).await else {
        return;
    };
    r.world.rig.blobs.set_failing_reads(true);
    let task = r.service.spawn_warmup();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(!task.is_finished(), "a failed build does not end the task");
    assert!(!r.service.text_corpora_are_current().await.unwrap());
    let err = r
        .service
        .search_text(&Reads::viewer(), text("needle"))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ReadError::Unavailable,
        "the search builds for itself, fails the same way, and says so"
    );
    r.world.rig.blobs.set_failing_reads(false);
    until_current(&r.service).await;
    assert_eq!(search(&r.service, "needle").await.hits.len(), 34);
    task.shutdown().await;
}

#[tokio::test]
async fn a_snapshot_over_its_bound_is_made_for_the_request_and_not_kept() {
    let config = RegistryConfig {
        corpus_cache_bytes: 1024 * 1024,
        ..config()
    };
    let Some(mut r) = Reads::with(&["sit1"], config).await else {
        return;
    };
    let big = |i: usize| {
        format!(
            "{}needle {i}\n",
            format!("line of padding number {i} with some text in it\n").repeat(10_000)
        )
    };
    let files: Vec<(String, String)> = (0..3).map(|i| (format!("big{i}.txt"), big(i))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    r.world.inventory(&refs).await;
    let (_, m0) = counts(&r, CacheName::Corpus);
    let first = search(&r.service, "needle").await;
    let second = search(&r.service, "needle").await;
    assert_eq!(first.hits.len(), 3);
    assert_eq!(first, second);
    assert_eq!(
        counts(&r, CacheName::Corpus).1 - m0,
        2,
        "each search made its own: over the bound nothing is kept"
    );
    assert!(!r.service.text_corpora_are_current().await.unwrap());
}

#[tokio::test]
async fn a_search_of_some_swimlanes_uses_the_snapshot_of_all_and_finds_only_theirs() {
    let Some(r) = world(config()).await else {
        return;
    };
    let all = search(&r.service, "needle").await;
    let (hits, misses) = counts(&r, CacheName::Corpus);
    let mut one = text("needle");
    one.swimlanes = vec![lane("sit2")];
    let only = r.service.search_text(&Reads::viewer(), one).await.unwrap();
    assert_eq!(
        counts(&r, CacheName::Corpus),
        (hits + 1, misses),
        "no build for a subset of what is built"
    );
    let want: Vec<_> = found(&all).into_iter().filter(|(l, _, _)| l == "sit2").collect();
    assert_eq!(found(&only), want);
    let mut two = text("needle");
    two.swimlanes = vec![lane("sit3"), lane("sit1")];
    let both = r.service.search_text(&Reads::viewer(), two).await.unwrap();
    let want: Vec<_> = found(&all).into_iter().filter(|(l, _, _)| l != "sit2").collect();
    assert_eq!(found(&both), want, "sit1 then sit3, in swimlane order");
}

#[tokio::test]
async fn a_blob_the_store_lacked_is_searched_once_it_turns_up_though_no_swimlane_changed() {
    let Some(r) = world(config()).await else {
        return;
    };
    let content = bytes::Bytes::from_static(b"needle shared 2\n");
    let hash = ports::content_hash(&content);
    sqlx::query("DELETE FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().as_slice())
        .execute(&r.world.rig.pool)
        .await
        .unwrap();
    let without = search(&r.service, "shared 2").await;
    assert!(
        without.hits.is_empty(),
        "a blob that is not in the store is left out, with its places: {:?}",
        found(&without)
    );
    // The store has it again (nothing in any swimlane moved): the shard that lacked it is made again by the next build.
    r.world.rig.blobs.put(content).await.unwrap();
    r.sql("UPDATE swimlane_registry SET version = version + 1").await;
    let with = search(&r.service, "shared 2").await;
    assert_eq!(
        with.hits.len(),
        3,
        "sit1, sit2 and sit3 each have it: {:?}",
        found(&with)
    );
}
