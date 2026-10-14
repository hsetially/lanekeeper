//! P2, the registry's share: a push to Git shows up as updated drift within 10 seconds through the webhook, of which hub-platform
//! spends the webhook, the fetch and the tree index, and the registry has 1 s from `GitHeadMoved` to `DriftChanged` in the outbox
//! (`P2.hub_head_moved_to_drift`, `hub-registry/head_moved_to_drift`, a harness, p95).
//!
//! All 40 swimlanes of the target-scale fixtures are in the pipeline's memory (the job's takeover has loaded them), and a base file that
//! every swimlane holds changes in the base repository: the worst case of a push, since the base repository's default branch concerns
//! every swimlane with an inventory. A sample is the time from the publication of `GitHeadMoved` on the real event bus until a
//! `DriftChanged` that names the file has been published for each of the 40 swimlanes: the event is routed through the Git index (the
//! tree diff, the mapping to NFS names, the swimlanes the branch concerns), the windows are waited out, the 40 swimlanes are recomputed in
//! one batch and committed. Twenty samples, each a new version of the file in Git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use domain::{DomainEvent, GitRef, RepoKind};
use hub_registry::RegistryConfig;
use hub_registry::metrics::LaneResult;
use ports::{EventBus, GitReader};

use common::Bed;
use common::support::perf::write_result;
use common::support::world::{RunningJob, wait_until};

const LANES: usize = 40;
const SAMPLES: usize = 20;

#[tokio::main]
#[allow(clippy::too_many_lines)] // a script: bed, job, one sample loop
async fn main() {
    let Some(bed) = Bed::new(LANES, RegistryConfig::default()).await else {
        eprintln!("hub-registry/head_moved_to_drift: SKIPPED (no database)");
        return;
    };
    let cold = bed.pipeline.refresh_all().await.unwrap();
    assert_eq!(cold.lanes, LANES);
    eprintln!(
        "hub-registry/head_moved_to_drift: computed whole in {:.1} s",
        cold.took.as_secs_f64()
    );
    // The job takes over (it forgets what the nightly run left in memory) and loads every swimlane. The wait is for loads that happened
    // after the job started: the memory the nightly run left makes "every swimlane in memory" true before the takeover has cleared it,
    // and a first sample would then be the cost of the takeover, not of a push.
    let wholes = bed.rig.metrics.pipeline_lane_count(LaneResult::Whole);
    let job = RunningJob::start(&bed.pipeline);
    wait_until(
        "every swimlane loaded by the job",
        Duration::from_secs(900),
        || async {
            bed.rig.metrics.pipeline_lane_count(LaneResult::Whole) >= wholes + LANES as u64
                && bed.pipeline.lanes_in_memory().await.len() == LANES
        },
    )
    .await;

    // A base file that every swimlane holds, YAML, of median size among those.
    let mut common: Option<BTreeSet<String>> = None;
    for lane in &bed.lanes {
        let names: BTreeSet<String> = lane
            .files
            .iter()
            .filter(|(p, b)| common::is_yaml(p) && b.len() < 200_000)
            .map(|(p, _)| p.as_str().to_owned())
            .collect();
        common = Some(match common {
            None => names,
            Some(c) => c.intersection(&names).cloned().collect(),
        });
    }
    let git = bed.git.clone();
    let base_head = git.head(RepoKind::Base, "master").unwrap();
    let tree = git.tree_index(RepoKind::Base, &base_head).unwrap();
    let mut candidates: Vec<(u64, String)> = tree
        .entries()
        .iter()
        .filter_map(|e| {
            let nfs = e.path.as_str().strip_prefix("config/")?;
            common.as_ref()?.contains(nfs).then(|| (e.size, nfs.to_owned()))
        })
        .collect();
    candidates.sort();
    assert!(!candidates.is_empty(), "no base file is held by every swimlane");
    let rel = candidates[candidates.len() / 2].1.clone();
    let repo_path = format!("config/{rel}");
    eprintln!("hub-registry/head_moved_to_drift: the file is {rel}");

    let events = bed.rig.events.clone().unwrap();
    let mut samples = Vec::new();
    for sample in 0..SAMPLES {
        let from = git.head(RepoKind::Base, "master").unwrap();
        let text = format!("moved_{sample}: {sample}\n");
        let to = git
            .commit(
                RepoKind::Base,
                "master",
                &[(repo_path.as_str(), Some(text.as_bytes()))],
                "push",
            )
            .unwrap();
        let after: i64 = sqlx::query_scalar("SELECT COALESCE(max(id), 0) FROM outbox")
            .fetch_one(&bed.rig.pool)
            .await
            .unwrap();
        let started = Instant::now();
        events
            .publish(DomainEvent::GitHeadMoved {
                repo: RepoKind::Base,
                branch: GitRef::parse("master").unwrap(),
                from: Some(from),
                to,
            })
            .await
            .unwrap();
        loop {
            let seen: i64 = sqlx::query_scalar(
                "SELECT count(DISTINCT event #>> '{event,swimlane}') FROM outbox WHERE id > $1 \
                 AND event #>> '{event,type}' = 'drift_changed' AND (event #> '{event,paths}') @> to_jsonb($2::text)",
            )
            .bind(after)
            .bind(&rel)
            .fetch_one(&bed.rig.pool)
            .await
            .unwrap();
            if seen == i64::try_from(LANES).unwrap() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "sample {sample}: DriftChanged for {seen} of {LANES} swimlanes"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if sample % 4 == 0 {
            eprintln!(
                "hub-registry/head_moved_to_drift: sample {}: {ms:.0} ms",
                sample + 1
            );
        }
        samples.push(ms);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = job.lose_lease().await;
    let path = write_result("hub-registry/head_moved_to_drift", "ms", &samples);
    eprintln!("hub-registry/head_moved_to_drift: {}", path.display());
}
