//! Effective tree hashes (T2, D65, D67): the engine's directory hashes and effective file hashes, written by diff, and the proof that
//! writing only what changed leaves the tables equal to writing everything. Over a real Postgres 16 as `lanekeeper_app`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{ContentHash, LogicalFile, SwimlaneId};
use engine::LaneState;
use hub_registry::config::RegistryConfig;
use hub_registry::db::Db;
use hub_registry::{ManualClock, StoredTree, TreeHashStore, TreeWrite};
use proptest::prelude::*;
use support::lane::{logical, state};
use support::rig::Rig;

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
        .collect()
}

fn base() -> BTreeMap<String, String> {
    files(&[
        ("a/x.yml", "x: 1\n"),
        ("a/b/y.yml", "y: 1\n"),
        ("c/z.yml", "z: 1\n"),
        ("top.yml", "t: 1\n"),
    ])
}

fn store(rig: &Rig, config: &RegistryConfig) -> (TreeHashStore, Db) {
    let db = Db::new(rig.pool.clone(), config.query_timeout);
    (
        TreeHashStore::new(
            db.clone(),
            Arc::clone(&rig.clock) as _,
            config,
            rig.metrics.clone(),
        ),
        db,
    )
}

/// What the engine says the tables must hold for a state.
fn expected(state: &LaneState) -> StoredTree {
    StoredTree {
        dirs: state
            .tree_hashes()
            .iter()
            .map(|(d, h)| (d.as_str().to_owned(), *h))
            .collect(),
        files: state
            .effective_hashes()
            .map(|(f, h)| (f.as_str().to_owned(), h))
            .collect(),
    }
}

async fn sync(store: &TreeHashStore, db: &Db, s: &LaneState) -> TreeWrite {
    let mut tx = db.begin().await.unwrap();
    let w = store.sync(&mut tx, s).await.unwrap();
    db.commit(tx).await.unwrap();
    w
}

async fn apply(store: &TreeHashStore, db: &Db, s: &LaneState, touched: &[&str]) -> TreeWrite {
    let touched: Vec<LogicalFile> = touched.iter().map(|t| logical(t)).collect();
    let mut tx = db.begin().await.unwrap();
    let w = store.apply(&mut tx, s, &touched).await.unwrap();
    db.commit(tx).await.unwrap();
    w
}

async fn computed_at(rig: &Rig, swimlane: &str, dir: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM computed_at) * 1000)::bigint FROM effective_tree_hashes \
         WHERE swimlane_id = $1 AND dir_path = $2",
    )
    .bind(swimlane)
    .bind(dir)
    .fetch_one(&rig.pool)
    .await
    .unwrap()
}

fn id(s: &str) -> SwimlaneId {
    SwimlaneId::parse(s).unwrap()
}

// ------------------------------------------------------------------------------------------------ sync

#[tokio::test]
async fn sync_writes_the_engines_tree_and_a_second_sync_writes_nothing() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    let s1 = state("sit1", &base());

    let first = sync(&store, &db, &s1).await;
    assert_eq!(
        first,
        TreeWrite {
            dirs_written: 4,
            files_written: 4,
            ..TreeWrite::default()
        },
        "the root, a, a/b and c; four files"
    );
    let stored = store.load(&id("sit1")).await.unwrap();
    assert_eq!(
        stored,
        expected(&s1),
        "the rows are the engine's hashes, nothing else"
    );
    assert!(stored.dirs.contains_key(""), "the root is the directory ''");
    assert_eq!(Some(stored.dirs[""]), s1.tree_root());

    assert!(
        sync(&store, &db, &s1).await.is_empty(),
        "a second sync of the same state writes nothing"
    );
    assert_eq!(rig.metrics.tree_rows_count(), 8);
}

#[tokio::test]
async fn sync_repairs_a_table_that_was_changed_behind_its_back() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    let s1 = state("sit1", &base());
    sync(&store, &db, &s1).await;
    // A wrong hash, a row that should not be there, and a missing row.
    sqlx::query(
        "UPDATE effective_tree_hashes SET hash = $1::bytea WHERE swimlane_id = 'sit1' AND dir_path = 'a'",
    )
    .bind(vec![7u8; 32])
    .execute(&rig.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO effective_files (swimlane_id, logical_file, hash) VALUES ('sit1', 'ghost.yml', $1::bytea)")
        .bind(vec![9u8; 32])
        .execute(&rig.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM effective_files WHERE swimlane_id = 'sit1' AND logical_file = 'top.yml'")
        .execute(&rig.pool)
        .await
        .unwrap();

    let repair = sync(&store, &db, &s1).await;
    assert_eq!(
        repair,
        TreeWrite {
            dirs_written: 1,
            dirs_deleted: 0,
            files_written: 1,
            files_deleted: 1
        },
        "one wrong, one missing, one extra: and nothing else is touched"
    );
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&s1));
}

// ------------------------------------------------------------------------------------------------ apply

#[tokio::test]
async fn apply_writes_only_the_touched_files_and_the_directories_above_them() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    sync(&store, &db, &state("sit1", &base())).await;
    let before_c = computed_at(&rig, "sit1", "c").await;
    rig.clock.advance_secs(3600);

    let mut changed = base();
    changed.insert("a/b/y.yml".to_owned(), "y: 2\n".to_owned());
    let s2 = state("sit1", &changed);
    let w = apply(&store, &db, &s2, &["a/b/y.yml"]).await;
    assert_eq!(
        w,
        TreeWrite {
            dirs_written: 3,
            files_written: 1,
            ..TreeWrite::default()
        },
        "a/b, a and the root, and the file; not c, and not the siblings"
    );
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&s2));
    assert_eq!(
        computed_at(&rig, "sit1", "c").await,
        before_c,
        "an untouched directory is not rewritten"
    );
    assert_eq!(
        computed_at(&rig, "sit1", "a/b").await,
        ManualClock::T0_MS + 3_600_000,
        "a rewritten one is stamped by the clock"
    );

    // Naming a file whose hash did not change writes nothing: the diff is the hash.
    let again = apply(&store, &db, &s2, &["a/x.yml", "a/b/y.yml", "top.yml"]).await;
    assert!(again.is_empty(), "{again:?}");
}

#[tokio::test]
async fn a_removed_file_and_an_emptied_directory_are_deleted() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    sync(&store, &db, &state("sit1", &base())).await;

    let mut less = base();
    less.remove("c/z.yml");
    let s = state("sit1", &less);
    let w = apply(&store, &db, &s, &["c/z.yml"]).await;
    assert_eq!(
        w,
        TreeWrite {
            dirs_written: 1,
            dirs_deleted: 1,
            files_deleted: 1,
            files_written: 0
        },
        "the file, its directory c, and the root's new hash"
    );
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&s));

    // Everything gone: nothing is served, and no row remains.
    let none = state("sit1", &BTreeMap::new());
    let w = apply(&store, &db, &none, &["a/x.yml", "a/b/y.yml", "top.yml"]).await;
    assert_eq!(w.dirs_deleted, 3, "the root, a, a/b");
    assert_eq!(w.files_deleted, 3);
    assert_eq!(store.load(&id("sit1")).await.unwrap(), StoredTree::default());
    assert_eq!(rig.count("SELECT count(*) FROM effective_tree_hashes").await, 0);
    assert_eq!(rig.count("SELECT count(*) FROM effective_files").await, 0);
}

#[tokio::test]
async fn a_new_deep_directory_is_created_with_its_parents() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    sync(&store, &db, &state("sit1", &base())).await;
    let mut more = base();
    more.insert("new/deep/er/f.yml".to_owned(), "f: 1\n".to_owned());
    let s = state("sit1", &more);
    let w = apply(&store, &db, &s, &["new/deep/er/f.yml"]).await;
    assert_eq!(w.dirs_written, 4, "new/deep/er, new/deep, new and the root");
    assert_eq!(w.files_written, 1);
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&s));
}

#[tokio::test]
async fn swimlanes_do_not_touch_each_other() {
    let Some(rig) = Rig::new().await else { return };
    rig.add_swimlane("sit2", &[], false).await;
    let (store, db) = store(&rig, &RegistryConfig::default());
    let (a, b) = (state("sit1", &base()), state("sit2", &base()));
    sync(&store, &db, &a).await;
    sync(&store, &db, &b).await;
    let mut changed = base();
    changed.insert("c/z.yml".to_owned(), "z: 2\n".to_owned());
    let a2 = state("sit1", &changed);
    apply(&store, &db, &a2, &["c/z.yml"]).await;
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&a2));
    assert_eq!(
        store.load(&id("sit2")).await.unwrap(),
        expected(&b),
        "the other swimlane is as it was"
    );
    assert_eq!(
        store.load(&id("sit2")).await.unwrap().dirs[""],
        expected(&state("sit2", &base())).dirs[""]
    );
}

#[tokio::test]
async fn a_transaction_that_does_not_commit_leaves_the_tables_alone() {
    let Some(rig) = Rig::new().await else { return };
    let (store, db) = store(&rig, &RegistryConfig::default());
    let s = state("sit1", &base());
    let mut tx = db.begin().await.unwrap();
    store.sync(&mut tx, &s).await.unwrap();
    drop(tx);
    assert_eq!(rig.count("SELECT count(*) FROM effective_tree_hashes").await, 0);
    assert_eq!(rig.count("SELECT count(*) FROM effective_files").await, 0);
}

#[tokio::test]
async fn a_large_swimlane_is_written_in_statements_of_bounded_size() {
    let Some(rig) = Rig::new().await else { return };
    let config = RegistryConfig {
        rows_per_statement: 100,
        ..RegistryConfig::default()
    };
    let (store, db) = store(&rig, &config);
    let many: BTreeMap<String, String> = (0..700)
        .map(|i| (format!("d{}/f{i}.yml", i % 17), format!("n: {i}\n")))
        .collect();
    let s = state("sit1", &many);
    let w = sync(&store, &db, &s).await;
    assert_eq!(
        (w.files_written, w.dirs_written),
        (700, 18),
        "17 directories and the root, in chunks of 100 rows"
    );
    assert_eq!(store.load(&id("sit1")).await.unwrap(), expected(&s));
}

// ------------------------------------------------------------------------------------------------ apply equals sync

const UNIVERSE: [&str; 12] = [
    "a.yml",
    "a/b.yml",
    "a/b/c.yml",
    "a/b/d.yml",
    "e/f.yml",
    "e/g/h.yml",
    "e/g/i.yml",
    "j/k.yml",
    "j/l/m.yml",
    "n.yml",
    "o/p.yml",
    "o/q/r.yml",
];

/// The logical files whose effective hash is different, or missing on one side.
fn changed(old: &LaneState, new: &LaneState) -> Vec<String> {
    let (a, b): (BTreeMap<_, ContentHash>, BTreeMap<_, ContentHash>) = (
        old.effective_hashes()
            .map(|(f, h)| (f.as_str().to_owned(), h))
            .collect(),
        new.effective_hashes()
            .map(|(f, h)| (f.as_str().to_owned(), h))
            .collect(),
    );
    a.keys()
        .chain(b.keys())
        .filter(|f| a.get(*f) != b.get(*f))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// One case of the property: a sequence of edits, applied one at a time.
async fn case(rig: &Rig, steps: Vec<(usize, u8)>) {
    let (store, db) = store(rig, &RegistryConfig::default());
    sqlx::query("DELETE FROM effective_tree_hashes WHERE swimlane_id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM effective_files WHERE swimlane_id = 'sit1'")
        .execute(&rig.pool)
        .await
        .unwrap();
    let mut current: BTreeMap<String, String> = BTreeMap::new();
    let mut previous = state("sit1", &current);
    for (which, content) in steps {
        let path = UNIVERSE[which].to_owned();
        if content == 0 {
            current.remove(&path);
        } else {
            current.insert(path, format!("v: {content}\n"));
        }
        let next = state("sit1", &current);
        let touched = changed(&previous, &next);
        let touched: Vec<&str> = touched.iter().map(String::as_str).collect();
        apply(&store, &db, &next, &touched).await;
        assert_eq!(
            store.load(&id("sit1")).await.unwrap(),
            expected(&next),
            "after {touched:?}"
        );
        previous = next;
    }
    // A full sync from here changes nothing: the incremental tables were already right.
    let w = sync(&store, &db, &previous).await;
    assert!(w.is_empty(), "{w:?}");
}

/// After any sequence of edits, the tables written by `apply` (touched files only) equal the engine's tree, which is what `sync`
/// writes from scratch: the diff-based writes lose nothing and keep nothing stale. One database serves every case (a database per
/// case would spend the test on migrations) and is dropped at the end.
#[test]
fn incremental_writes_equal_a_full_write_after_random_edits() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let Some(rig) = rt.block_on(Rig::new()) else {
        return;
    };
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(32));
    let strategy = prop::collection::vec((0usize..UNIVERSE.len(), 0u8..4), 1..14);
    let result = runner.run(&strategy, |steps| {
        rt.block_on(case(&rig, steps));
        Ok(())
    });
    drop(rig);
    result.unwrap();
}
