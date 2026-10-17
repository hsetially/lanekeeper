//! The adoption ranking at design scale (T5, D22, P6): 40 swimlanes of about 2,000 files and 60 tenant branches, loaded through the
//! registry's own ingestion path into a real Postgres, the fixtures' repositories in the in-memory Git. The branch each swimlane was
//! deployed from ranks first, and ranking takes 5 seconds or less per swimlane (the prompt's bound; the budget `P6.adoption_rank` is its
//! benchmark, `adoption_rank_per_swimlane`, whose p95 `bench-check` holds to the same 5,000 ms).
//!
//! These tests are marked `#[ignore]` and run by `registry-slow-test` (release mode, one at a time): loading 80,000 files is the slow part
//! and the time bound is the point, so it is checked without the load of the rest of the suite. The swimlanes are ranked once and both tests
//! read the same result.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

#[path = "../benches/common/mod.rs"]
mod common;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use hub_registry::{AdoptionDeps, AdoptionReason, AdoptionService, AdoptionView, RegistryConfig};

use common::Bed;
use common::support::adoption::{admin, assert_ranked_right, insert_user};

/// The bound of the prompt (T5): ranking takes 5 seconds or less per swimlane.
const BOUND: Duration = Duration::from_secs(5);

struct Ranked {
    tenants: Vec<String>,
    view: AdoptionView,
    took: Duration,
}

async fn rank_every_swimlane() -> Option<Vec<Ranked>> {
    let bed = Bed::new(40, RegistryConfig::default()).await?;
    let admin = admin();
    insert_user(&bed.rig.pool, &admin).await;
    let svc = AdoptionService::new(AdoptionDeps {
        pool: bed.rig.pool.clone(),
        git: Arc::new(bed.git.clone()),
        blobs: Arc::clone(&bed.rig.blobs) as _,
        bus: Arc::clone(&bed.rig.bus) as _,
        audit: Arc::clone(&bed.rig.audit) as _,
        clock: Arc::clone(&bed.rig.clock) as _,
        config: RegistryConfig::default(),
        metrics: bed.rig.metrics.clone(),
        pipeline: None,
    })
    .unwrap();
    let mut out = Vec::new();
    for lane in &bed.lanes {
        let started = Instant::now();
        let view = svc
            .start(&admin, &lane.id, AdoptionReason::Initial)
            .await
            .unwrap();
        out.push(Ranked {
            tenants: lane.tenants.clone(),
            view,
            took: started.elapsed(),
        });
    }
    Some(out)
}

fn results() -> Option<&'static Vec<Ranked>> {
    static CELL: OnceLock<Option<Vec<Ranked>>> = OnceLock::new();
    CELL.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(rank_every_swimlane())
    })
    .as_ref()
}

#[test]
#[ignore = "target scale: run by `just registry-slow-test`"]
fn the_correct_branch_ranks_first_for_every_fixture_swimlane() {
    let Some(ranked) = results() else {
        eprintln!("SKIPPED: no database");
        return;
    };
    assert_eq!(ranked.len(), 40);
    for r in ranked {
        assert_ranked_right(&r.view, &r.tenants);
    }
}

#[test]
#[ignore = "target scale: run by `just registry-slow-test`"]
fn ranking_takes_5_seconds_or_less_per_swimlane() {
    let Some(ranked) = results() else {
        eprintln!("SKIPPED: no database");
        return;
    };
    let mut ms: Vec<f64> = ranked.iter().map(|r| r.took.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    eprintln!(
        "adoption ranking of {} swimlanes: first (cold) {:.1} ms, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms.len(),
        ranked[0].took.as_secs_f64() * 1000.0,
        common::percentile(&ms, 0.50),
        common::percentile(&ms, 0.95),
        ms[ms.len() - 1],
    );
    for r in ranked {
        assert!(
            r.took <= BOUND,
            "{} took {:?}, the bound is {BOUND:?}",
            r.view.swimlane,
            r.took
        );
    }
}
