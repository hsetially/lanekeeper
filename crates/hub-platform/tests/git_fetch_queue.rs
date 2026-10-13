//! T7 (rule 5, A19): the fetch queue is bounded and coalesced, so a flood of webhooks cannot cause fetches without bound.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use domain::{GitRef, RepoKind};
use hub_platform::git::fetch_queue::{Enqueue, FetchQueue};
use hub_platform::git::payload::PushedRef;

fn branch(name: &str) -> PushedRef {
    PushedRef::Branch(GitRef::parse(name).unwrap())
}

#[tokio::test]
async fn pushes_to_one_repo_coalesce_into_one_fetch_job() {
    let queue = FetchQueue::new(16);
    assert_eq!(queue.push(RepoKind::Tenant, branch("sit1")), Enqueue::Queued);
    assert_eq!(queue.push(RepoKind::Tenant, branch("sit2")), Enqueue::Queued);
    assert_eq!(queue.push(RepoKind::Tenant, branch("sit1")), Enqueue::Coalesced);
    assert_eq!(queue.push(RepoKind::Base, branch("main")), Enqueue::Queued);
    assert_eq!(queue.len(), 3);

    let mut batch = queue.next_batch().await;
    batch.sort_by_key(|j| j.repo);
    assert_eq!(batch.len(), 2, "one job per repository");
    assert_eq!(batch[0].repo, RepoKind::Base);
    assert_eq!(batch[1].repo, RepoKind::Tenant);
    assert_eq!(batch[1].targets.len(), 2, "sit1 once and sit2");
    assert_eq!(queue.len(), 0);
}

#[tokio::test]
async fn a_full_queue_drops_new_targets_and_counts_them() {
    let queue = FetchQueue::new(2);
    assert_eq!(queue.push(RepoKind::Tenant, branch("a")), Enqueue::Queued);
    assert_eq!(queue.push(RepoKind::Tenant, branch("b")), Enqueue::Queued);
    assert_eq!(queue.push(RepoKind::Tenant, branch("c")), Enqueue::Dropped);
    assert_eq!(
        queue.push(RepoKind::Tenant, branch("a")),
        Enqueue::Coalesced,
        "a known target still coalesces"
    );
    assert_eq!(queue.dropped_total(), 1);
    assert_eq!(queue.len(), 2, "the bound holds");
    // After the batch is taken there is room again.
    let _ = queue.next_batch().await;
    assert_eq!(queue.push(RepoKind::Tenant, branch("c")), Enqueue::Queued);
}

#[tokio::test(start_paused = true)]
async fn next_batch_waits_until_something_is_queued() {
    let queue = FetchQueue::new(4);
    let waiter = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.next_batch().await })
    };
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!waiter.is_finished(), "nothing queued, so it waits");
    queue.push(RepoKind::Base, PushedRef::Tag(GitRef::parse("v1").unwrap()));
    let batch = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(batch.len(), 1);
}

#[tokio::test]
async fn a_push_that_arrives_while_a_batch_is_being_worked_is_kept_for_the_next_one() {
    let queue = FetchQueue::new(4);
    queue.push(RepoKind::Base, branch("main"));
    let first = queue.next_batch().await;
    assert_eq!(first.len(), 1);
    queue.push(RepoKind::Base, branch("main"));
    let second = tokio::time::timeout(Duration::from_secs(1), queue.next_batch())
        .await
        .unwrap();
    assert_eq!(second.len(), 1, "taking a batch does not swallow later pushes");
}
