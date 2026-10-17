//! Acceptance 2 (T3): after random sequences of events, the incremental state equals a full recompute.
//!
//! A swimlane lives through a random sequence of the things that happen to one: a file written or deleted on NFS (through ingestion),
//! a base file or a tenant file changed in Git (a moved head, through the Git index), an adoption, a baseline or a mark made or removed,
//! a tenant added. The pipeline is told what each of them means the way the job is (the work an event or a signal means), sometimes at
//! once and sometimes several together in one batch. At the end three things must hold:
//!
//! 1. a **fresh** pipeline, that knows nothing and computes the swimlane from scratch, finds nothing to repair: it writes no row, no
//!    finding and no event, and its engine state is equal to the incremental one (`LaneState` is compared whole: every file's state,
//!    every effective hash, the tree, the findings);
//! 2. the **tables** are what the model says: the drift state of each path is the engine's `drift_state` of what the model has on NFS and
//!    in Git and of the baseline and the mark the database holds, so the loader's reading of the world is checked against the test's
//!    own, not against itself;
//! 3. the same after a **takeover**: a second pipeline that starts from the database (`forget_all`) and is told everything has changed
//!    ends in the same tables.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use domain::{ContentHash, FileClass, RepoKind};
use engine::{Baseline, DriftInput, IntentionalMark, drift_hash, drift_state};
use hub_registry::{RegistryConfig, Work};
use proptest::prelude::*;
use support::rig::path;
use support::world::{RunningJob, Tables, World, state_of, wait_until};

/// What can be on NFS or in Git: LF and CRLF spellings of one text are one file for drift.
const TEXTS: [&str; 4] = ["a: 1\n", "a: 2\n", "b:\n  c: 3\n", "a: 1\r\n"];
/// The NFS names. The first four are base files; the last is the sit1 copy of the first.
const FILES: [&str; 5] = [
    "app/core.yml",
    "app/other.yml",
    "app/new.yml",
    "application.yml",
    "app/core-sit1.yml",
];
const TENANT_FILE: usize = 4;

fn hash_of(text: usize) -> ContentHash {
    drift_hash(
        Some(&bytes::Bytes::from(TEXTS[text].to_owned())),
        FileClass::Structured,
    )
    .unwrap()
}

#[derive(Debug, Clone)]
enum Op {
    Nfs { file: usize, text: usize },
    NfsDelete { file: usize },
    Base { file: usize, text: Option<usize> },
    Tenant { text: Option<usize> },
    Adopt,
    Baseline { file: usize, text: Option<usize> },
    Mark { file: usize },
    AddTenant,
}

fn op() -> impl Strategy<Value = (Op, bool)> {
    let file = 0..FILES.len();
    let base_file = 0..FILES.len() - 1;
    let text = 0..TEXTS.len();
    (
        prop_oneof![
            4 => (file.clone(), text.clone()).prop_map(|(file, text)| Op::Nfs { file, text }),
            1 => file.clone().prop_map(|file| Op::NfsDelete { file }),
            3 => (base_file, proptest::option::weighted(0.85, text.clone())).prop_map(|(file, text)| Op::Base { file, text }),
            2 => proptest::option::weighted(0.85, text.clone()).prop_map(|text| Op::Tenant { text }),
            1 => Just(Op::Adopt),
            2 => (file.clone(), proptest::option::weighted(0.8, text)).prop_map(|(file, text)| Op::Baseline { file, text }),
            1 => file.prop_map(|file| Op::Mark { file }),
            1 => Just(Op::AddTenant),
        ],
        // Whether the pipeline is brought up to date after this step, or the work waits for the next one (a batch of several steps).
        proptest::bool::weighted(0.6),
    )
}

/// What the test believes is in the world.
#[derive(Default)]
struct Model {
    nfs: BTreeMap<usize, usize>,
    base: BTreeMap<usize, usize>,
    tenant: Option<usize>,
}

impl Model {
    fn g(&self, file: usize) -> Option<ContentHash> {
        if file == TENANT_FILE {
            self.tenant.map(hash_of)
        } else {
            self.base.get(&file).copied().map(hash_of)
        }
    }

    fn n(&self, file: usize) -> Option<ContentHash> {
        self.nfs.get(&file).copied().map(hash_of)
    }
}

fn parse_hash(text: &str) -> ContentHash {
    ContentHash::parse(text).unwrap()
}

/// The drift state of every path, from the model's NFS and Git and the baselines and marks the database holds.
fn expected(
    model: &Model,
    tables: &Tables,
    marks: &BTreeMap<usize, IntentionalMark>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (i, name) in FILES.iter().enumerate() {
        let (n, g) = (model.n(i), model.g(i));
        if n.is_none() && g.is_none() {
            continue;
        }
        let baseline = tables
            .baselines
            .get(*name)
            .map_or(Baseline::NotRecorded, |h| Baseline::Recorded(Some(parse_hash(h))));
        // A mark is read with the hashes the test made it with; the table only says that there is one, so a mark that is there is
        // the test's own mark of this path, and its hashes are the ones recorded when it was made.
        let intentional = tables.marks.contains(*name).then(|| marks.get(&i).copied());
        out.insert(
            (*name).to_owned(),
            drift_state(&DriftInput {
                baseline,
                nfs: n,
                git: g,
                intentional: intentional.flatten(),
            })
            .state
            .as_str()
            .to_owned(),
        );
    }
    out
}

async fn run(ops: Vec<(Op, bool)>) -> Result<(), TestCaseError> {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return Ok(());
    };
    let mut marks: BTreeMap<usize, IntentionalMark> = BTreeMap::new();
    let mut model = Model::default();
    let lane = w.lane();

    // The starting point: a base file and a tenant file in Git, and the same on NFS, computed whole.
    w.base_commit(&[
        ("config/app/core.yml", Some(TEXTS[0])),
        ("config/application.yml", Some(TEXTS[0])),
    ]);
    model.base.insert(0, 0);
    model.base.insert(3, 0);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some(TEXTS[1]))]);
    model.tenant = Some(1);
    w.inventory(&[
        ("app/core.yml", TEXTS[0]),
        ("app/core-sit1.yml", TEXTS[1]),
        ("application.yml", TEXTS[0]),
    ])
    .await;
    model.nfs.insert(0, 0);
    model.nfs.insert(TENANT_FILE, 1);
    model.nfs.insert(3, 0);
    w.whole().await;

    let mut pending: Vec<(domain::SwimlaneId, Work)> = Vec::new();
    let one = |name: &str| (lane.clone(), Work::Paths([path(name)].into()));
    let mut sit2 = false;
    for (op, now) in &ops {
        match op {
            Op::Nfs { file, text } => {
                w.nfs_change(&[(FILES[*file], TEXTS[*text])]).await;
                model.nfs.insert(*file, *text);
                pending.push(one(FILES[*file]));
            }
            Op::NfsDelete { file } => {
                if model.nfs.remove(file).is_some() {
                    w.nfs_delete(&[FILES[*file]]).await;
                    pending.push(one(FILES[*file]));
                }
            }
            Op::Base { file, text } => {
                let from = w.head(RepoKind::Base, "master");
                let name = format!("config/{}", FILES[*file]);
                let to = w.base_commit(&[(name.as_str(), text.map(|t| TEXTS[t]))]);
                match text {
                    Some(t) => model.base.insert(*file, *t),
                    None => model.base.remove(file),
                };
                pending.extend(w.moved(RepoKind::Base, "master", Some(&from), &to).await);
            }
            Op::Tenant { text } => {
                let from = w.head(RepoKind::Tenant, "sit1");
                let to = w.tenant_commit("sit1", &[("data/config/app/core.yml", text.map(|t| TEXTS[t]))]);
                model.tenant = *text;
                pending.extend(w.moved(RepoKind::Tenant, "sit1", Some(&from), &to).await);
            }
            Op::Adopt => {
                w.adopt().await;
                // Confirming the adoption tells the pipeline to recompute the swimlane.
                pending.push((lane.clone(), Work::Whole));
            }
            Op::Baseline { file, text } => {
                match text {
                    Some(t) => w.baseline(FILES[*file], hash_of(*t)).await,
                    None => w.remove_baseline(FILES[*file]).await,
                }
                pending.push(one(FILES[*file]));
            }
            Op::Mark { file } => {
                // A mark hangs off the file's row, so the row has to be there: bring the swimlane up to date first.
                w.pipeline.process(std::mem::take(&mut pending)).await.unwrap();
                let (n, g) = (model.n(*file), model.g(*file));
                if (n.is_some() || g.is_some()) && w.rig.state(FILES[*file]).await.is_some() {
                    w.mark(FILES[*file], n, g).await;
                    marks.insert(*file, IntentionalMark { nfs: n, git: g });
                    pending.push(one(FILES[*file]));
                }
            }
            Op::AddTenant => {
                if !sit2 {
                    w.add_tenant("sit2").await;
                    sit2 = true;
                    pending.push(one(FILES[0]));
                }
            }
        }
        if *now {
            w.pipeline.process(std::mem::take(&mut pending)).await.unwrap();
        }
    }
    w.pipeline.process(std::mem::take(&mut pending)).await.unwrap();

    let incremental_tables = w.tables().await;
    let incremental_state = w.pipeline.lane_state(&lane).await;

    // 2. The tables say what the model says.
    let want = expected(&model, &incremental_tables, &marks);
    let got: BTreeMap<String, String> = incremental_tables
        .rows
        .iter()
        .map(|(p, r)| (p.clone(), r.0.clone()))
        .collect();
    prop_assert_eq!(&got, &want, "the drift state of each path");

    // 1. A fresh pipeline finds nothing to repair, and computes the same state.
    let (fresh, _) = w.rig.pipeline(
        std::sync::Arc::new(w.git.clone()),
        hub_registry::RegistryConfig::default(),
    );
    let outbox = w.rig.count("SELECT count(*) FROM outbox").await;
    let report = fresh.refresh_lane(&lane).await.unwrap();
    prop_assert_eq!(report.lanes.len(), 1);
    prop_assert_eq!(
        report.lanes[0].rows_written + report.lanes[0].rows_deleted,
        0,
        "rows to repair: {:?}",
        report
    );
    prop_assert_eq!(
        report.lanes[0].findings_changed,
        0,
        "findings to repair: {:?}",
        report
    );
    prop_assert_eq!(
        report.lanes[0].baselines_set,
        0,
        "baselines the incremental run did not set"
    );
    prop_assert_eq!(
        w.rig.count("SELECT count(*) FROM outbox").await,
        outbox,
        "no event for a repair that is not needed"
    );
    prop_assert_eq!(w.tables().await, incremental_tables.clone(), "the tables");
    prop_assert_eq!(
        fresh.lane_state(&lane).await,
        incremental_state.clone(),
        "the engine state, whole"
    );

    // 3. A takeover: the same pipeline forgets everything, and is told that every path changed.
    w.pipeline.forget_all().await;
    let all: BTreeSet<_> = FILES.iter().map(|f| path(f)).collect();
    w.pipeline
        .process(vec![(lane.clone(), Work::Paths(all))])
        .await
        .unwrap();
    prop_assert_eq!(
        w.tables().await,
        incremental_tables,
        "the tables after a takeover"
    );
    prop_assert_eq!(
        w.pipeline.lane_state(&lane).await,
        incremental_state,
        "the state after a takeover"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        // 12 cases run in the gate; `PROPTEST_CASES=200` runs more (each case is a swimlane over its own database).
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(12),
        max_shrink_iters: 40,
        ..ProptestConfig::default()
    })]

    #[test]
    fn incremental_state_equals_full_recompute_after_random_events(ops in proptest::collection::vec(op(), 1..14)) {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(run(ops))?;
    }
}

// ------------------------------------------------------------------------------------------------ a takeover, with the job

fn drift(text: &str) -> ContentHash {
    drift_hash(Some(&bytes::Bytes::from(text.to_owned())), FileClass::Structured).unwrap()
}

/// A takeover through the job itself: a pipeline has been running; then nobody holds the lease while NFS, both Git repositories and the
/// baselines move on. The next holder starts empty, loads the swimlane from the database and the world, and must end where a computation
/// from scratch ends, whatever was announced in between (nothing was).
#[tokio::test]
async fn incremental_equals_full_after_a_takeover() {
    let Some(mut w) = World::new(&["sit1"]).await else {
        return;
    };
    w.base_commit(&[
        ("config/app/core.yml", Some("a: 1\n")),
        ("config/app/drifted.yml", Some("d: 0\n")),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("a: 2\n"))]);
    w.inventory(&[
        ("app/core.yml", "a: 1\n"),
        ("app/core-sit1.yml", "a: 2\n"),
        ("app/drifted.yml", "d: 1\n"),
    ])
    .await;
    w.whole().await;

    w.nfs_change(&[("app/drifted.yml", "d: 3\n")]).await;
    w.paths(&["app/drifted.yml"]).await;
    w.pipeline.forget_all().await;
    w.nfs_change(&[("app/core.yml", "a: 6\n"), ("app/new.yml", "n: 1\n")])
        .await;
    w.nfs_delete(&["app/core-sit1.yml"]).await;
    w.base_commit(&[
        ("config/app/drifted.yml", Some("d: 3\n")),
        ("config/app/gone-soon.yml", Some("g: 1\n")),
    ]);
    w.tenant_commit("sit1", &[("data/config/app/core.yml", Some("a: 8\n"))]);
    w.baseline("app/core.yml", drift("a: 1\n")).await;

    let (b, _) = w
        .rig
        .pipeline(std::sync::Arc::new(w.git.clone()), RegistryConfig::default());
    let job = RunningJob::start(&b);
    wait_until(
        "the swimlane verified",
        std::time::Duration::from_secs(20),
        || async { b.lanes_in_memory().await.contains(&w.lane()) },
    )
    .await;
    let tables = w.tables().await;
    // A third, fresh pipeline computes the same thing from scratch and finds nothing to change.
    let (c, _) = w
        .rig
        .pipeline(std::sync::Arc::new(w.git.clone()), RegistryConfig::default());
    let report = c.refresh_lane(&w.lane()).await.unwrap();
    assert_eq!(
        report.lanes[0].rows_written + report.lanes[0].rows_deleted,
        0,
        "{report:?}"
    );
    assert_eq!(w.tables().await, tables);
    assert_eq!(b.lane_state(&w.lane()).await, c.lane_state(&w.lane()).await);
    assert_eq!(
        state_of(&w, "app/core-sit1.yml").await.as_deref(),
        Some("unknown"),
        "Git has the file, NFS lost it"
    );
    assert!(
        w.tables().await.rows.contains_key("app/gone-soon.yml"),
        "a file only Git has"
    );
    let _ = job.lose_lease().await;
}
