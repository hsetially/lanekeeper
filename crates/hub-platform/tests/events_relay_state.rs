//! T5: the relay's cursor and its overlap window (D80, P13), without a database.
//!
//! Ids come from a sequence, but transactions commit in any order: a transaction with a lower id can commit after one with a
//! higher id. A cursor that only moves forward would skip it. The state keeps the ids below the cursor that have not been seen
//! yet (the gaps) for a bounded time and asks for exactly those again. These tests drive the state with every commit order the
//! property generator finds and check that no committed id is skipped and none is delivered twice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use hub_platform::events::relay::RelayState;
use proptest::prelude::*;

const TTL: Duration = Duration::from_secs(10);

fn state(newest: i64) -> RelayState {
    RelayState::new(newest, TTL, 16)
}

#[test]
fn consecutive_ids_move_the_cursor_and_leave_no_gaps() {
    let now = Instant::now();
    let mut s = state(10);
    assert_eq!(s.after(), 10);
    assert_eq!(s.observe(&[11, 12, 13], &[], now), vec![11, 12, 13]);
    assert_eq!(s.after(), 13);
    assert!(s.gaps().is_empty());
}

#[test]
fn a_jump_leaves_the_skipped_ids_as_gaps_and_a_late_commit_fills_one() {
    let now = Instant::now();
    let mut s = state(0);
    // 1 is in flight, 2 and 3 committed.
    assert_eq!(s.observe(&[2, 3], &[], now), vec![2, 3]);
    assert_eq!(s.gaps(), vec![1]);
    // Nothing new, 1 still not committed.
    assert_eq!(s.observe(&[], &[], now), Vec::<i64>::new());
    assert_eq!(s.gaps(), vec![1]);
    // 1 commits.
    assert_eq!(s.observe(&[], &[1], now), vec![1]);
    assert!(s.gaps().is_empty());
    assert_eq!(s.after(), 3, "the cursor never moves back");
}

#[test]
fn a_gap_that_never_commits_is_dropped_after_the_ttl() {
    let t0 = Instant::now();
    let mut s = state(0);
    s.observe(&[3], &[], t0);
    assert_eq!(s.gaps(), vec![1, 2]);
    s.expire(t0 + Duration::from_millis(9_999));
    assert_eq!(s.gaps(), vec![1, 2], "still inside the window");
    s.expire(t0 + TTL + Duration::from_millis(1));
    assert!(s.gaps().is_empty(), "rolled back, as far as anyone can tell");
}

#[test]
fn the_number_of_gaps_is_bounded_and_the_newest_are_kept() {
    let now = Instant::now();
    let mut s = RelayState::new(0, TTL, 4);
    // A jump of a million ids (a sequence that skipped) must not allocate a million entries.
    assert_eq!(s.observe(&[1_000_000], &[], now), vec![1_000_000]);
    assert_eq!(s.gaps().len(), 4);
    assert_eq!(s.gaps(), vec![999_996, 999_997, 999_998, 999_999]);
}

#[test]
fn a_late_id_that_is_not_a_gap_is_not_delivered() {
    // A row the database returned for an id the state is not waiting for (already delivered, or expired) is a duplicate.
    let now = Instant::now();
    let mut s = state(5);
    assert_eq!(s.observe(&[], &[3], now), Vec::<i64>::new());
    assert_eq!(s.observe(&[6], &[], now), vec![6]);
    assert_eq!(s.observe(&[], &[6], now), Vec::<i64>::new(), "6 was delivered");
}

#[test]
fn a_new_id_at_or_below_the_cursor_is_not_delivered_twice() {
    let now = Instant::now();
    let mut s = state(0);
    assert_eq!(s.observe(&[1, 2], &[], now), vec![1, 2]);
    assert_eq!(s.observe(&[2], &[], now), Vec::<i64>::new());
}

#[test]
fn reset_forgets_the_gaps_and_moves_the_cursor() {
    let now = Instant::now();
    let mut s = state(0);
    s.observe(&[5], &[], now);
    assert!(!s.gaps().is_empty());
    s.reset(100);
    assert_eq!(s.after(), 100);
    assert!(s.gaps().is_empty());
}

#[test]
fn delivery_is_in_ascending_id_order_even_when_a_late_id_arrives_with_new_ones() {
    let now = Instant::now();
    let mut s = state(0);
    s.observe(&[3], &[], now);
    assert_eq!(s.observe(&[4, 5], &[1, 2], now), vec![1, 2, 4, 5]);
}

/// One step of the model: some transactions commit, then the relay polls.
#[derive(Debug, Clone)]
enum Step {
    /// The next in-flight transaction (by position in the pending list) commits.
    Commit(usize),
    /// The next in-flight transaction (by position) rolls back.
    Rollback(usize),
    /// The relay polls once, reading at most `batch` new rows.
    Poll { batch: usize },
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(
        prop_oneof![
            4 => (0usize..8).prop_map(Step::Commit),
            1 => (0usize..8).prop_map(Step::Rollback),
            3 => (1usize..6).prop_map(|batch| Step::Poll { batch }),
        ],
        1..120,
    )
}

proptest! {
    /// 40 transactions take ids 1..=40 in order and then finish in the generated order. The relay polls in between and at the
    /// end. Time stands still, so the gap window never expires: the only thing under test is the bookkeeping.
    #[test]
    fn no_committed_id_is_skipped_and_none_is_delivered_twice(script in steps()) {
        let now = Instant::now();
        let mut relay = RelayState::new(0, TTL, 4096);
        let mut pending: Vec<i64> = (1..=40).collect();
        let mut committed: BTreeSet<i64> = BTreeSet::new();
        let mut delivered: Vec<i64> = Vec::new();

        let poll = |relay: &mut RelayState, committed: &BTreeSet<i64>, delivered: &mut Vec<i64>, batch: usize| {
            let after = relay.after();
            let new: Vec<i64> = committed.range(after + 1..).copied().take(batch).collect();
            let wanted: BTreeSet<i64> = relay.gaps().into_iter().collect();
            let late: Vec<i64> = committed.iter().copied().filter(|id| wanted.contains(id)).collect();
            delivered.extend(relay.observe(&new, &late, now));
        };

        for step in script {
            match step {
                Step::Commit(i) if !pending.is_empty() => {
                    let id = pending.remove(i % pending.len());
                    committed.insert(id);
                }
                Step::Rollback(i) if !pending.is_empty() => {
                    pending.remove(i % pending.len());
                }
                Step::Poll { batch } => poll(&mut relay, &committed, &mut delivered, batch),
                _ => {}
            }
        }
        // Everything still in flight commits, and the relay polls until it is idle.
        committed.extend(pending.drain(..));
        for _ in 0..100 {
            poll(&mut relay, &committed, &mut delivered, 6);
        }

        let unique: BTreeSet<i64> = delivered.iter().copied().collect();
        prop_assert_eq!(unique.len(), delivered.len(), "delivered twice: {:?}", delivered);
        prop_assert_eq!(unique, committed, "skipped an id");
    }
}
