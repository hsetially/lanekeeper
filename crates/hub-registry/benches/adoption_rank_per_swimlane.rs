//! P6, ranking the deployable branches of one swimlane for the adoption pass (`P6.adoption_rank`, `hub-registry/adoption_rank_per_swimlane`,
//! a harness, p95, 5,000 ms): 40 swimlanes of about 2,000 files and 60 tenant branches, a real Postgres, the in-memory Git loaded from the
//! fixtures' repositories, and `AdoptionService::start`, which is the whole first step of the wizard for one swimlane: the swimlane's
//! files and the path mapping from the database, every deployable branch of the mirror, its tree index (path and SHA-256 of every file,
//! no blob read), `engine::adoption_rank` on rayon, the hints, the row of the pass and its audit event.
//!
//! A sample is one `start` of one swimlane. Three rounds over the 40 swimlanes, 120 samples; between rounds each pass is cancelled, so
//! that every sample opens a new pass (a started one is resumed, which costs nothing). The first call of the run builds the tree indexes
//! of the 60 branches in the in-memory Git (the platform's `git_tree_index` and its cache do that once per commit in the hub), so the
//! largest sample is that cold one and the rest are warm; the budget takes the 95th percentile of all 120, and the run prints the cold
//! sample and the median apart.
//!
//! Every sample is checked: the swimlane's own tenants fill the first places of the ranking, with files in common, and no other branch
//! shares a file with it, so that what is timed is a ranking that is right.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::cast_precision_loss
)]

mod common;

use std::sync::Arc;
use std::time::Instant;

use hub_registry::{AdoptionDeps, AdoptionReason, AdoptionService, RegistryConfig};

use common::Bed;
use common::support::adoption::{admin, assert_ranked_right, insert_user};
use common::support::perf::{percentile, write_result};

const ROUNDS: usize = 3;

#[tokio::main]
async fn main() {
    let Some(bed) = Bed::new(40, RegistryConfig::default()).await else {
        eprintln!("hub-registry/adoption_rank_per_swimlane: SKIPPED (no database)");
        return;
    };
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

    let mut samples: Vec<f64> = Vec::new();
    let mut cold = None;
    for round in 0..ROUNDS {
        for lane in &bed.lanes {
            let started = Instant::now();
            let view = svc
                .start(&admin, &lane.id, AdoptionReason::Initial)
                .await
                .unwrap();
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_ranked_right(&view, &lane.tenants);
            if cold.is_none() {
                cold = Some(ms);
            }
            samples.push(ms);
            // The next round opens a new pass for this swimlane.
            svc.cancel(&admin, view.id).await.unwrap();
        }
        eprintln!(
            "hub-registry/adoption_rank_per_swimlane: round {} of {ROUNDS}: {} swimlanes ranked",
            round + 1,
            bed.lanes.len()
        );
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!(
        "hub-registry/adoption_rank_per_swimlane: {} samples, cold first call {:.1} ms, p50 {:.1} ms, p95 {:.1} ms, p99 {:.1} ms, max {:.1} ms",
        samples.len(),
        cold.unwrap_or_default(),
        percentile(&sorted, 0.50),
        percentile(&sorted, 0.95),
        percentile(&sorted, 0.99),
        percentile(&sorted, 1.0),
    );
    let path = write_result("hub-registry/adoption_rank_per_swimlane", "ms", &samples);
    eprintln!("hub-registry/adoption_rank_per_swimlane: {}", path.display());
}
