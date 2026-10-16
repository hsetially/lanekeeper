//! P8: an edit is applied on NFS and acknowledged within 1.5 seconds, also when the request reaches a replica that is not the
//! one holding the agent's stream (an extra hop). The hop is modelled by a delay in the fake agent; the budget is judged over
//! a run of edits and not one, and `benches/edit_applied.rs` measures the same path as the registered budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::time::{Duration, Instant};

use domain::Role;
use ports::WriteService;
use support::rig::{Rig, applied, at, ctx, edit, h};

#[tokio::test]
async fn edit_through_a_second_replica_meets_p8() {
    let rig = Rig::new(db!()).await;
    let s = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "n: 0\n")]);
    let editor = rig.user(1, Role::Editor).await;
    // The request is served by a second replica; the agent's stream is on the first, which costs a hop of tens of milliseconds
    // on a cluster network. A pessimistic 100 ms each way.
    rig.agent.set_delay(Duration::from_millis(200));
    let replica = rig.replica();
    let mut worst = Duration::ZERO;
    let mut current = "n: 0\n".to_owned();
    for i in 1..=20 {
        let next = format!("n: {i}\n");
        let started = Instant::now();
        let out = replica
            .edit(
                &ctx(&editor, &format!("k-{i}")),
                edit(&s, "a.yml", at(&current), &next),
            )
            .await
            .unwrap();
        worst = worst.max(started.elapsed());
        assert_eq!(applied(&out).0, Some(h(&next)));
        current = next;
    }
    assert!(worst < Duration::from_millis(1500), "slowest edit took {worst:?}");
}
