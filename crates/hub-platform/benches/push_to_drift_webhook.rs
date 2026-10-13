//! P2: a Git push reaches `GitHeadMoved` on another replica within 10 s (budget `P2`, `hub-platform/push_to_drift_webhook`, p95).
//!
//! The part of "push to drift" that the hub platform owns (plan A3): a signed `push` webhook arrives at replica 0, which fetches
//! the push from the remote (the target-scale tenant fixture repository, copied, over the file transport; the production leg is
//! HTTPS and is not measured in CI), announces `GitHeadMoved` through the outbox, and the sample ends when a subscriber of
//! replica 1 has the event. 05 adds the drift step, and 16 measures the whole path. Three replicas on one real Postgres, each
//! with its own mirror. 20 pushes after 3 warm-up pushes. Writes `target/perf-results/hub-platform_push_to_drift_webhook.json`.
//!
//! Needs Docker (testcontainers) and the fixtures (`cargo xtask gen-fixtures --scale target`); without either this exits with an
//! error rather than writing a result.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::cast_precision_loss
)]

#[path = "../tests/support/mod.rs"]
mod support;

mod common;

use std::time::{Duration, Instant};

use domain::RepoKind;
use support::fake_github::GitRemote;
use support::git_cluster::{GitCluster, eventually, next_head_moved};
use support::perf::write_result;

const REPLICAS: usize = 3;
const WARM_UP: usize = 3;
const MEASURED: usize = 20;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    if !runtime.block_on(run()) {
        std::process::exit(1);
    }
}

async fn run() -> bool {
    let (base, tenant) = common::fixture_repos();
    let Some(mut cluster) =
        GitCluster::start_over(REPLICAS, GitRemote::copy_of(&base), GitRemote::copy_of(&tenant)).await
    else {
        eprintln!("push_to_drift_webhook: no Docker, so no Postgres to measure against");
        return false;
    };
    let mut samples = Vec::with_capacity(MEASURED);
    for n in 0..WARM_UP + MEASURED {
        // A push to one of twenty branches, changing one file, as a developer's push would.
        let branch = format!("sit{}", n % 20 + 1);
        let before = cluster.tenant.head(&branch).expect("the fixture has the branch");
        let file = format!("data/config/bench-{n}-{branch}.yml");
        let content = format!("n: {n}\n");
        let after = cluster
            .tenant
            .commit(&branch, &[(file.as_str(), Some(content.as_bytes()))], "push");

        let started = Instant::now();
        let status = cluster
            .deliver_push(
                0,
                RepoKind::Tenant,
                &branch,
                &before,
                &after,
                &format!("bench-{n}"),
            )
            .await;
        assert_eq!(status, http::StatusCode::ACCEPTED);
        // The subscriber of another replica: the event, not just the fetch.
        let heard = loop {
            let Some((_, moved_branch, _, to)) =
                next_head_moved(&mut cluster.replicas[1].events, Duration::from_secs(30)).await
            else {
                eprintln!("push_to_drift_webhook: push {n} was not announced to replica 1 within 30 s");
                return false;
            };
            if moved_branch.as_str() == branch && to.as_str() == after {
                break started.elapsed();
            }
        };
        if n >= WARM_UP {
            samples.push(heard.as_secs_f64() * 1000.0);
        }
        // Settle: drain the other subscribers and wait until every replica has the commit, so the next push starts clean.
        for index in [0, 2] {
            let _ = next_head_moved(&mut cluster.replicas[index].events, Duration::from_secs(30)).await;
        }
        for replica in &cluster.replicas {
            let mirror = std::sync::Arc::clone(&replica.mirror);
            let target = domain::CommitId::parse(&after).unwrap();
            eventually("every mirror to have the push", Duration::from_secs(30), || {
                mirror.has_commit(RepoKind::Tenant, &target)
            })
            .await;
        }
    }
    let path = write_result("hub-platform/push_to_drift_webhook", "ms", &samples);
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    eprintln!(
        "push_to_drift_webhook: {} pushes, p50 {:.0} ms, p95 {:.0} ms, max {:.0} ms -> {}",
        samples.len(),
        support::perf::percentile(&sorted, 0.5),
        support::perf::percentile(&sorted, 0.95),
        support::perf::percentile(&sorted, 1.0),
        path.display()
    );
    true
}
