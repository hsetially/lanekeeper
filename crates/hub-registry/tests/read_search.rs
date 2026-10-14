//! Settings search and text search (T6, P7, S11, S12, A27, A28, A31, rule 4, rule 5): what they find, what they never look at, their
//! bounds and their pages, over a real Postgres with the settings index current.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::time::{Duration, Instant};

use domain::{Page, SettingsQuery, ShortText, TextQuery};
use hub_registry::config::RegistryConfig;
use hub_registry::metrics::{CacheName, CacheResult};
use ports::{ReadError, RegistryRead};
use support::reads::{Reads, lane, logical, page};

const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

/// `sit1` (inventory) and `sit2`, `sit3` (the same kind of files), with the index current.
async fn world(sit1: &[(&str, &str)], others: &[(&str, &[(&str, &str)])]) -> Option<Reads> {
    world_with(RegistryConfig::default(), sit1, others).await
}

async fn world_with(
    config: RegistryConfig,
    sit1: &[(&str, &str)],
    others: &[(&str, &[(&str, &str)])],
) -> Option<Reads> {
    let mut r = Reads::with(&["sit1"], config).await?;
    for (id, _) in others {
        r.world.rig.add_swimlane(id, &[id], false).await;
    }
    r.world.inventory(sit1).await;
    for (id, files) in others {
        r.world.second_lane(id, &[id], files).await;
    }
    r.index_all().await;
    Some(r)
}

fn settings(q: &str, lanes: &[&str], file: Option<&str>, limit: u32) -> SettingsQuery {
    SettingsQuery {
        path_contains: ShortText::parse(q).unwrap(),
        swimlanes: lanes.iter().map(|l| lane(l)).collect(),
        file: file.map(logical),
        page: page(limit),
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

// ------------------------------------------------------------------------------------------------ settings search

#[tokio::test]
async fn a_settings_search_finds_a_part_of_a_path_in_every_swimlane_that_holds_it_ignoring_case() {
    let Some(r) = world(
        &[
            ("core.yml", "Server:\n  Port: 8080\n  name: a\n"),
            ("other.yml", "port: 1\n"),
        ],
        &[("sit2", &[("core.yml", "Server:\n  Port: 9090\n")])],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    let hits = r
        .service
        .search_settings(&v, settings("port", &[], None, 100))
        .await
        .unwrap();
    let found: Vec<(String, String, String, String)> = hits
        .items
        .iter()
        .map(|h| {
            (
                h.swimlane.to_string(),
                h.file.to_string(),
                h.path.to_string(),
                h.value.text.clone(),
            )
        })
        .collect();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.contains(&(
        "sit1".into(),
        "core.yml".into(),
        "Server.Port".into(),
        "8080".into()
    )));
    assert!(found.contains(&(
        "sit2".into(),
        "core.yml".into(),
        "Server.Port".into(),
        "9090".into()
    )));
    assert!(found.contains(&("sit1".into(), "other.yml".into(), "port".into(), "1".into())));
    assert!(hits.next.is_none());
    let one = hits.items.iter().find(|h| h.value.text == "8080").unwrap();
    assert_eq!(
        one.location.lines.start(),
        2,
        "the line of the setting in its file"
    );
    // A part from the middle, and nothing that is not in a path.
    assert_eq!(
        r.service
            .search_settings(&v, settings("erver.po", &[], None, 100))
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(
        r.service
            .search_settings(&v, settings("zzz", &[], None, 100))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    // A value is not a path.
    assert!(
        r.service
            .search_settings(&v, settings("8080", &[], None, 100))
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn like_metacharacters_in_the_needle_are_literal() {
    let Some(r) = world(&[("core.yml", "a_b: 1\naxb: 2\npct: 3\nback: 4\n")], &[]).await else {
        return;
    };
    let v = Reads::viewer();
    let paths = |hits: domain::Paged<domain::SettingHit>| -> Vec<String> {
        hits.items.iter().map(|h| h.path.to_string()).collect()
    };
    assert_eq!(
        paths(
            r.service
                .search_settings(&v, settings("a_b", &[], None, 100))
                .await
                .unwrap()
        ),
        ["a_b"],
        "`_` is not a one-character wildcard"
    );
    assert!(
        r.service
            .search_settings(&v, settings("%", &[], None, 100))
            .await
            .unwrap()
            .items
            .is_empty(),
        "`%` is not a wildcard"
    );
    assert!(
        r.service
            .search_settings(&v, settings("\\", &[], None, 100))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        paths(
            r.service
                .search_settings(&v, settings("A_B", &[], None, 100))
                .await
                .unwrap()
        ),
        ["a_b"],
        "and the match ignores case"
    );
}

#[tokio::test]
async fn a_settings_search_can_be_limited_to_swimlanes_and_a_file_and_names_a_swimlane_that_is_not_there() {
    let Some(r) = world(
        &[("core.yml", "port: 1\n"), ("other.yml", "port: 2\n")],
        &[
            ("sit2", &[("core.yml", "port: 3\n")]),
            ("sit3", &[("core.yml", "port: 4\n")]),
        ],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    let lanes = |hits: &domain::Paged<domain::SettingHit>| -> Vec<String> {
        let mut l: Vec<String> = hits.items.iter().map(|h| h.swimlane.to_string()).collect();
        l.sort();
        l
    };
    let two = r
        .service
        .search_settings(&v, settings("port", &["sit2", "sit3"], None, 100))
        .await
        .unwrap();
    assert_eq!(lanes(&two), ["sit2", "sit3"]);
    let dup = r
        .service
        .search_settings(&v, settings("port", &["sit2", "sit2"], None, 100))
        .await
        .unwrap();
    assert_eq!(lanes(&dup), ["sit2"], "a swimlane named twice is searched once");
    let file = r
        .service
        .search_settings(&v, settings("port", &[], Some("other.yml"), 100))
        .await
        .unwrap();
    assert_eq!(lanes(&file), ["sit1"]);
    assert_eq!(
        r.service
            .search_settings(&v, settings("port", &["sit2", "sit9"], None, 100))
            .await
            .unwrap_err(),
        ReadError::NotFound
    );
    let mut empty = settings("port", &[], None, 100);
    empty.path_contains = ShortText::parse("").unwrap();
    assert_eq!(
        r.service.search_settings(&v, empty).await.unwrap_err(),
        ReadError::BadRequest
    );
}

#[tokio::test]
async fn settings_pages_hold_every_hit_once_even_where_a_page_ends_between_two_swimlanes_of_one_setting() {
    // The same file in three swimlanes: one blob, one setting, three hits that a page boundary can fall between.
    let same: &[(&str, &str)] = &[("core.yml", "port: 1\nname: a\nnode: z\n")];
    let Some(r) = world(same, &[("sit2", same), ("sit3", same)]).await else {
        return;
    };
    let v = Reads::viewer();
    let all = r
        .service
        .search_settings(&v, settings("o", &[], None, 100))
        .await
        .unwrap();
    let key = |h: &domain::SettingHit| (h.path.to_string(), h.swimlane.to_string());
    let whole: Vec<_> = all.items.iter().map(key).collect();
    assert_eq!(
        whole.len(),
        6,
        "two settings with an `o` in them, three swimlanes: {whole:?}"
    );
    for size in [1, 2, 4] {
        let mut seen = Vec::new();
        let mut cursor = None;
        for _ in 0..30 {
            let mut q = settings("o", &[], None, size);
            q.page = Page::new(size, cursor.clone()).unwrap();
            let page = r.service.search_settings(&v, q).await.unwrap();
            assert!(page.items.len() <= size as usize);
            seen.extend(page.items.iter().map(key));
            match page.next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(seen, whole, "page size {size}: each hit once, in the same order");
    }
    // A cursor belongs to its search: another needle, other swimlanes or another file refuse it.
    let first = r
        .service
        .search_settings(&v, settings("o", &[], None, 2))
        .await
        .unwrap();
    let cursor = first.next.unwrap();
    let with = |q: &str, lanes: &[&str], file: Option<&str>| {
        let mut s = settings(q, lanes, file, 2);
        s.page = Page::new(2, Some(cursor.clone())).unwrap();
        s
    };
    for q in [
        with("n", &[], None),
        with("o", &["sit1"], None),
        with("o", &[], Some("core.yml")),
    ] {
        assert_eq!(
            r.service.search_settings(&v, q).await.unwrap_err(),
            ReadError::BadRequest
        );
    }
    assert!(
        r.service.search_settings(&v, with("O", &[], None)).await.is_ok(),
        "case is not part of the search"
    );
}

#[tokio::test]
async fn a_flagged_value_is_masked_in_a_settings_hit() {
    let Some(r) = world(
        &[("core.yml", &format!("api:\n  token: {SECRET}\n  url: http://x\n"))],
        &[],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    let hits = r
        .service
        .search_settings(&v, settings("token", &[], None, 10))
        .await
        .unwrap();
    assert_eq!(hits.items.len(), 1);
    let value = &hits.items[0].value;
    assert_eq!(value.text, "[REDACTED: 40 chars, github_token]");
    assert!(!serde_json::to_string(&hits.items).unwrap().contains(SECRET));
    let url = r
        .service
        .search_settings(&v, settings("url", &[], None, 10))
        .await
        .unwrap();
    assert_eq!(
        url.items[0].value.text, "http://x",
        "only the flagged value is masked"
    );
}

#[tokio::test]
async fn a_search_sees_a_file_as_it_is_now_and_not_as_it_was_or_as_a_denied_file_has_it() {
    let Some(mut r) = world(
        &[
            ("core.yml", "old_key: 1\n"),
            ("keys/app.pem", "-----BEGIN PRIVATE KEY-----\nAAAA\n"),
        ],
        &[],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    assert_eq!(
        r.service
            .search_settings(&v, settings("old_key", &[], None, 10))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    r.world.nfs_change(&[("core.yml", "new_key: 1\n")]).await;
    r.index_all().await;
    assert!(
        r.service
            .search_settings(&v, settings("old_key", &[], None, 10))
            .await
            .unwrap()
            .items
            .is_empty(),
        "the previous version is history, not a hit"
    );
    assert_eq!(
        r.service
            .search_settings(&v, settings("new_key", &[], None, 10))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    r.world.nfs_delete(&["core.yml"]).await;
    assert!(
        r.service
            .search_settings(&v, settings("new_key", &[], None, 10))
            .await
            .unwrap()
            .items
            .is_empty(),
        "a deleted file"
    );
}

// ------------------------------------------------------------------------------------------------ text search

#[tokio::test]
async fn a_text_search_finds_lines_with_the_ranges_of_the_match_and_never_looks_at_binary_or_denied_files() {
    let Some(r) = world(
        &[
            ("core.yml", "a: 1\nTimeout: 30\nb: 2\n"),
            ("notes.txt", "wait for the timeout, then Timeout again\n"),
            ("blob.bin", "\u{0}\u{1}timeout"),
            ("keys/app.pem", "-----BEGIN PRIVATE KEY-----\ntimeout\n"),
        ],
        &[("sit2", &[("core.yml", "timeout: 5\n")])],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    let found = r.service.search_text(&v, text("timeout")).await.unwrap();
    assert!(!found.truncated);
    let mut got: Vec<(String, String, u32, String)> = found
        .hits
        .iter()
        .map(|h| (h.swimlane.to_string(), h.path.to_string(), h.line, h.text.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            ("sit1".into(), "core.yml".into(), 2, "Timeout: 30".into()),
            (
                "sit1".into(),
                "notes.txt".into(),
                1,
                "wait for the timeout, then Timeout again".into()
            ),
            ("sit2".into(), "core.yml".into(), 1, "timeout: 5".into()),
        ],
        "case-insensitive by default; the binary file and the denied file are not searched"
    );
    let notes = found
        .hits
        .iter()
        .find(|h| h.path.as_str() == "notes.txt")
        .unwrap();
    assert_eq!(
        notes.ranges,
        [(13, 20), (27, 34)],
        "byte ranges of both matches within the line"
    );
    // Case sensitive.
    let mut sensitive = text("Timeout");
    sensitive.case_sensitive = true;
    assert_eq!(r.service.search_text(&v, sensitive).await.unwrap().hits.len(), 2);
    // A literal by default, a regex when asked for.
    let literal = r.service.search_text(&v, text("t.meout")).await.unwrap();
    assert!(literal.hits.is_empty(), "the dot is a dot");
    let mut re = text("t.meout");
    re.regex = true;
    assert_eq!(r.service.search_text(&v, re).await.unwrap().hits.len(), 3);
    // Swimlanes and a glob narrow it.
    let mut narrow = text("timeout");
    narrow.swimlanes = vec![lane("sit2")];
    assert_eq!(r.service.search_text(&v, narrow).await.unwrap().hits.len(), 1);
    let mut glob = text("timeout");
    glob.path_glob = Some(ShortText::parse("**/*.txt").unwrap());
    let g = r.service.search_text(&v, glob).await.unwrap();
    assert_eq!(g.hits.len(), 1);
    assert_eq!(g.hits[0].path.as_str(), "notes.txt");
    let mut missing = text("timeout");
    missing.swimlanes = vec![lane("sit9")];
    assert_eq!(
        r.service.search_text(&v, missing).await.unwrap_err(),
        ReadError::NotFound
    );
}

#[tokio::test]
async fn the_caps_are_the_servers_ceilings_whatever_the_request_asks_and_truncation_is_reported() {
    let many: String = (0..30).map(|i| format!("hit {i}\n")).collect::<Vec<_>>().concat();
    let files: Vec<(String, String)> = (0..6).map(|i| (format!("f{i}.txt"), many.clone())).collect();
    let config = RegistryConfig {
        search_max_total: 7,
        search_max_per_file: 3,
        ..RegistryConfig::default()
    };
    // Six files with the same bytes are one blob in six places; make them differ so that each is searched on its own.
    let distinct: Vec<(String, String)> = files
        .iter()
        .enumerate()
        .map(|(i, (p, c))| (p.clone(), format!("{c}unique {i}\n")))
        .collect();
    let drefs: Vec<(&str, &str)> = distinct.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let Some(r) = world_with(config, &drefs, &[]).await else {
        return;
    };
    let v = Reads::viewer();
    // The request asks for the REST maximum; the ceilings win.
    let greedy = TextQuery {
        max_total: 500,
        max_per_file: 100,
        ..text("hit")
    };
    let out = r.service.search_text(&v, greedy).await.unwrap();
    assert_eq!(out.hits.len(), 7, "the total ceiling");
    assert!(out.truncated);
    let mut per_file = std::collections::BTreeMap::new();
    for h in &out.hits {
        *per_file.entry(h.path.to_string()).or_insert(0) += 1;
    }
    assert!(
        per_file.values().all(|n| *n <= 3),
        "the per-file ceiling: {per_file:?}"
    );
    assert_eq!(r.world.rig.metrics.read_search_truncated_count(), 1);
    // A request below the ceilings is respected.
    let modest = TextQuery {
        max_total: 2,
        max_per_file: 1,
        ..text("hit")
    };
    let out = r.service.search_text(&v, modest).await.unwrap();
    assert_eq!(out.hits.len(), 2);
    assert!(out.truncated);
    // Fewer matches than any cap: nothing is cut.
    let few = TextQuery {
        max_total: 500,
        ..text("unique 5")
    };
    let out = r.service.search_text(&v, few).await.unwrap();
    assert_eq!((out.hits.len(), out.truncated), (1, false));
    // A cap of nothing is not a search.
    for bad in [
        TextQuery {
            max_total: 0,
            ..text("hit")
        },
        TextQuery {
            max_per_file: 0,
            ..text("hit")
        },
    ] {
        assert_eq!(
            r.service.search_text(&v, bad).await.unwrap_err(),
            ReadError::BadRequest
        );
    }
}

#[tokio::test]
async fn a_pattern_is_bounded_and_a_regex_runs_in_linear_time_under_limits() {
    let Some(r) = world(&[("core.yml", "aaaa\n"), ("notes.txt", &"a".repeat(2000))], &[]).await else {
        return;
    };
    let v = Reads::viewer();
    for bad in [String::new(), "x".repeat(513)] {
        assert_eq!(
            r.service.search_text(&v, text(&bad)).await.unwrap_err(),
            ReadError::BadRequest
        );
    }
    let regex = |p: &str| TextQuery {
        regex: true,
        ..text(p)
    };
    for invalid in ["(unclosed", "[z-a]", "(?<n>a)\\k<m>", "a{2,1}"] {
        assert_eq!(
            r.service.search_text(&v, regex(invalid)).await.unwrap_err(),
            ReadError::BadRequest,
            "{invalid}"
        );
    }
    // A pattern whose compiled size is over the engine's limit is refused, not run.
    assert_eq!(
        r.service
            .search_text(&v, regex("((a{100}){100}){100}"))
            .await
            .unwrap_err(),
        ReadError::BadRequest
    );
    // The classic catastrophic pattern finishes at once: there is no backtracking.
    let started = Instant::now();
    let out = r.service.search_text(&v, regex("(a+)+$")).await.unwrap();
    assert!(!out.hits.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let literal = r.service.search_text(&v, text("(a+)+$")).await.unwrap();
    assert!(literal.hits.is_empty(), "as a literal it is just characters");
}

#[tokio::test]
async fn the_corpus_is_built_once_for_a_version_and_a_change_makes_a_new_one() {
    let Some(mut r) = world(
        &[("a.txt", "needle one\n"), ("b.txt", "other\n")],
        &[("sit2", &[("a.txt", "needle one\n")])],
    )
    .await
    else {
        return;
    };
    let v = Reads::viewer();
    let m = r.world.rig.metrics.clone();
    let first = r.service.search_text(&v, text("needle")).await.unwrap();
    assert_eq!(first.hits.len(), 2, "one blob, two places, two hits");
    let (hits, misses) = (
        m.read_cache_count(CacheName::Corpus, CacheResult::Hit),
        m.read_cache_count(CacheName::Corpus, CacheResult::Miss),
    );
    assert_eq!(misses, 1);
    r.world.rig.blobs.forget_requests();
    r.service.search_text(&v, text("other")).await.unwrap();
    assert_eq!(
        m.read_cache_count(CacheName::Corpus, CacheResult::Hit),
        hits + 1,
        "the same swimlane versions: the same corpus"
    );
    assert_eq!(r.world.rig.blobs.many_calls(), 0, "no blob was read again");
    // A change to a swimlane is a new version, so a new corpus with the new bytes.
    r.world.nfs_change(&[("b.txt", "needle two\n")]).await;
    let after = r.service.search_text(&v, text("needle")).await.unwrap();
    assert_eq!(after.hits.len(), 3);
    assert_eq!(
        m.read_cache_count(CacheName::Corpus, CacheResult::Miss),
        misses + 1
    );
    // A search of one swimlane has a corpus of its own.
    let mut one = text("needle");
    one.swimlanes = vec![lane("sit2")];
    assert_eq!(r.service.search_text(&v, one).await.unwrap().hits.len(), 1);
}

#[tokio::test]
async fn concurrent_cold_searches_share_one_corpus_build() {
    let files: Vec<(String, String)> = (0..20)
        .map(|i| (format!("f{i}.txt"), format!("line {i} marker\n")))
        .collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let Some(r) = world(&refs, &[]).await else { return };
    r.world.rig.blobs.forget_requests();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (service, v) = (r.service.clone(), Reads::viewer());
        tasks.push(tokio::spawn(async move {
            service.search_text(&v, text("marker")).await.unwrap()
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().hits.len(), 20);
    }
    let requested = r.world.rig.blobs.requested();
    assert_eq!(
        requested.len(),
        20,
        "each blob was read once for eight searches: {}",
        requested.len()
    );
}

#[tokio::test]
async fn a_search_that_runs_past_its_deadline_is_answered_503_and_the_pool_is_not_held() {
    let config = RegistryConfig {
        read_pool_threads: 1,
        search_timeout: Duration::from_millis(200),
        ..RegistryConfig::default()
    };
    let Some(r) = world_with(config, &[("a.txt", "needle\n")], &[]).await else {
        return;
    };
    let v = Reads::viewer();
    // Warm the corpus so that only the search itself is waiting.
    assert_eq!(
        r.service
            .search_text(&v, text("needle"))
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
    // Occupy the only worker of the pool, as a long search would.
    let pool = r.service.read_pool();
    let (release, hold) = std::sync::mpsc::channel::<()>();
    pool.spawn(move || {
        let _ = hold.recv();
    });
    let started = Instant::now();
    let err = r.service.search_text(&v, text("needle")).await.unwrap_err();
    assert_eq!(err, ReadError::Unavailable);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "answered at the deadline, not when the pool came free: {:?}",
        started.elapsed()
    );
    // Let go: the cancelled job drains and the service works again.
    release.send(()).unwrap();
    let mut ok = None;
    for _ in 0..50 {
        match r.service.search_text(&v, text("needle")).await {
            Ok(found) => {
                ok = Some(found);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    assert_eq!(ok.expect("the pool is free again").hits.len(), 1);
}
