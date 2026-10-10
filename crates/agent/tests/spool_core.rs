//! The durable spool (T9, D74, P15): what it keeps, what it drops, what it replays, and what survives a crash.
//!
//! The spool is exercised without a connection: a test appends deltas as the scanner would, then attaches a pump to a
//! fresh outbox as a new connection would, and looks at what comes out. File operations run inline, so time is virtual.
// The sequence numbers and times in these tests are small, so the casts between `u64` and `i64` cannot wrap.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent::clock::Clock;
use agent::spool::{Appended, SpoolHooks, SpoolLimits};
use support::spool_rig::{
    Rig, delta, denied_file, file, hash_of, part, paths_of, removal, root, segment_files,
};

fn rig_with_limits(max_bytes: u64, max_entries: u64) -> Rig {
    Rig::with_limits(SpoolLimits::new(max_bytes, max_entries).with_segment_bytes(4 * 1024))
}

// ------------------------------------------------------------------------------------------------ keeping and acking

#[tokio::test(start_paused = true)]
async fn spool_ack_deletes_entries() {
    let rig = Rig::new();
    for seq in 1..=3 {
        rig.append(delta(
            seq,
            vec![file(&format!("svc/f{seq}.yml"), b"x: 1\n", 1_000 * seq as i64)],
        ))
        .await;
    }
    assert_eq!(rig.spool.stats().entries, 3);
    assert!(!rig.segments().is_empty());

    let sent = rig.replay_with(3, |d| rig.spool.ack(d.seq)).await;
    assert_eq!(sent.iter().map(|d| d.seq).collect::<Vec<_>>(), [1, 2, 3]);

    rig.spool.maintain().await;
    let stats = rig.spool.stats();
    assert_eq!((stats.records, stats.entries), (0, 0), "{stats:?}");
    assert!(
        rig.segments().is_empty(),
        "every segment is fully acknowledged and removed"
    );
    assert_eq!(stats.disk_bytes, 0);
}

#[tokio::test(start_paused = true)]
async fn an_ack_for_a_seq_the_spool_does_not_hold_is_ignored() {
    let rig = Rig::new();
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    rig.spool.ack(99);
    rig.spool.ack(0);
    assert_eq!(rig.spool.stats().entries, 1);
    rig.spool.ack(1);
    rig.spool.ack(1);
    assert_eq!(rig.spool.stats().entries, 0);
}

#[tokio::test(start_paused = true)]
async fn spool_dedups_adjacent_duplicates_only() {
    let rig = Rig::new();
    rig.append(delta(1, vec![file("svc/a.yml", b"one", 1_000)])).await;
    // The same content again, with a new file: only the new file is news.
    rig.append(delta(
        2,
        vec![file("svc/a.yml", b"one", 2_000), file("svc/b.yml", b"two", 2_000)],
    ))
    .await;
    // And a removal of something that was not there is not collapsed with anything.
    let got = rig.replay(2).await;
    assert_eq!(
        paths_of(&got),
        [
            (1, vec!["svc/a.yml".to_owned()]),
            (2, vec!["svc/b.yml".to_owned()])
        ]
    );
    assert_eq!(
        rig.spool.stats().entries,
        2,
        "three versions came in, two are kept"
    );
}

#[tokio::test(start_paused = true)]
async fn spool_a_b_a_keeps_final_state() {
    // A global dedup by (path, hash) would turn A, B, A into A, B and the hub would end on B. Only the version that
    // directly follows an identical one is collapsed.
    let rig = Rig::new();
    rig.append(delta(1, vec![file("svc/a.yml", b"A", 1_000)])).await;
    rig.append(delta(2, vec![file("svc/a.yml", b"B", 2_000)])).await;
    rig.append(delta(3, vec![file("svc/a.yml", b"A", 3_000)])).await;
    let got = rig.replay(3).await;
    let hashes: Vec<_> = got.iter().map(|d| d.entries[0].hash).collect();
    assert_eq!(hashes, [hash_of(b"A"), hash_of(b"B"), hash_of(b"A")]);
}

#[tokio::test(start_paused = true)]
async fn a_removal_directly_after_a_removal_is_collapsed_and_a_return_is_not() {
    let rig = Rig::new();
    rig.append(delta(1, vec![file("a.yml", b"A", 1)])).await;
    rig.append(removal(2, "a.yml")).await;
    rig.append(removal(3, "a.yml")).await;
    rig.append(delta(4, vec![file("a.yml", b"A", 4)])).await;
    let got = rig.replay(4).await;
    assert_eq!(got[1].removed.len(), 1);
    assert!(got[2].removed.is_empty(), "the second removal repeats the first");
    assert_eq!(got[3].entries.len(), 1, "the file coming back is news");
}

#[tokio::test(start_paused = true)]
async fn dedup_only_looks_at_versions_that_are_still_pending() {
    // An acknowledged version is the hub's business now; the same content later (after a write the agent did not
    // spool, say) must still go out.
    let rig = Rig::new();
    rig.append(delta(1, vec![file("a.yml", b"A", 1)])).await;
    rig.replay_with(1, |d| rig.spool.ack(d.seq)).await;
    rig.append(delta(2, vec![file("a.yml", b"A", 2)])).await;
    let got = rig.replay(1).await;
    assert_eq!(got[0].seq, 2);
    assert_eq!(got[0].entries.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_denied_entry_without_content_survives_the_spool() {
    // T11 decides what is denied. The record format must carry the flag and the absence of bytes.
    let rig = Rig::new();
    rig.append(delta(
        1,
        vec![
            denied_file("keys/site.pem", 1_700, 5),
            file("svc/a.yml", b"ok", 5),
        ],
    ))
    .await;
    let rig = rig.reopen();
    let got = rig.replay(1).await;
    let denied = &got[0].entries[0];
    assert!(denied.denied && denied.bytes.is_none());
    assert_eq!(denied.size, 1_700);
    assert!(got[0].entries[1].bytes.is_some());
    // And nothing of a denied path's content could be in the files: there was none to write.
    for segment in segment_files(rig.path()) {
        let bytes = std::fs::read(segment).unwrap();
        assert!(!bytes.windows(4).any(|w| w == b"PRIV"));
    }
}

#[tokio::test(start_paused = true)]
async fn skipped_entries_and_the_job_tag_survive_the_spool() {
    let rig = Rig::new();
    let mut d = delta(1, vec![file("a.yml", b"a", 1)]);
    d.skipped.push(domain::SkippedEntry {
        path: domain::NfsPath::parse("big.bin").unwrap(),
        reason: domain::ShortText::parse("too_large").unwrap(),
    });
    d.during_job = Some(domain::JobRef::new("dataload-1", "uid-1234").unwrap());
    rig.append(d.clone()).await;
    let rig = rig.reopen();
    let got = rig.replay(1).await;
    assert_eq!(got[0], d);
}

#[tokio::test(start_paused = true)]
async fn a_seq_that_does_not_increase_is_refused() {
    let rig = Rig::new();
    rig.append(delta(5, vec![file("a.yml", b"a", 1)])).await;
    assert!(
        rig.spool
            .append(delta(5, vec![file("b.yml", b"b", 2)]))
            .await
            .is_err()
    );
    assert!(
        rig.spool
            .append(delta(4, vec![file("b.yml", b"b", 2)]))
            .await
            .is_err()
    );
    assert_eq!(rig.spool.stats().entries, 1);
}

#[tokio::test(start_paused = true)]
async fn spool_files_are_private_to_the_agent() {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new();
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    for entry in std::fs::read_dir(rig.path()).unwrap() {
        let entry = entry.unwrap();
        let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "{:?} is {mode:o}", entry.file_name());
    }
}

#[tokio::test(start_paused = true)]
async fn files_the_spool_did_not_write_are_left_alone() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("lost+found"), b"x").unwrap();
    std::fs::write(dir.path().join("README"), b"not mine").unwrap();
    let rig = Rig::open_in(dir, SpoolLimits::new(1 << 30, 1_000));
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    let rig = rig.reopen();
    assert!(rig.path().join("README").exists() && rig.path().join("lost+found").exists());
    assert_eq!(rig.replay(1).await.len(), 1);
}

// ------------------------------------------------------------------------------------------------ groups

#[tokio::test(start_paused = true)]
async fn the_parts_of_one_delta_are_replayed_together_and_in_order() {
    let rig = Rig::new();
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 1)])).await;
    // Not a complete delta yet: the hub would wait for it, so the pump does not send the first part alone.
    rig.append(part(2, 1, true, vec![file("b.yml", b"b", 1)])).await;
    rig.append(part(3, 2, false, vec![file("c.yml", b"c", 1)])).await;
    let got = rig.replay(3).await;
    assert_eq!(
        got.iter().map(|d| (d.seq, d.part, d.more)).collect::<Vec<_>>(),
        [(1, 0, true), (2, 1, true), (3, 2, false)]
    );
}

#[tokio::test(start_paused = true)]
async fn an_unfinished_delta_is_not_sent() {
    let rig = Rig::new();
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 1)])).await;
    assert!(rig.nothing_more_to_replay(0).await);
    rig.append(part(2, 1, false, vec![file("b.yml", b"b", 1)])).await;
    assert_eq!(rig.replay(2).await.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn spool_replays_whole_group_after_partial_ack() {
    // The hub keeps the parts of a delta per connection. After a reconnect the first parts are gone from its memory,
    // so the delta is sent again from part 0 even though part 0 was acknowledged before.
    let rig = Rig::new();
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 1)])).await;
    rig.append(part(2, 1, true, vec![file("b.yml", b"b", 1)])).await;
    rig.append(part(3, 2, false, vec![file("c.yml", b"c", 1)])).await;
    rig.spool.ack(1);
    rig.spool.ack(2);
    rig.spool.maintain().await;
    let got = rig.replay(3).await;
    assert_eq!(got.iter().map(|d| d.seq).collect::<Vec<_>>(), [1, 2, 3]);
    rig.spool.ack(3);
    rig.spool.maintain().await;
    assert_eq!(
        rig.spool.stats().records,
        0,
        "the group goes when its last part is acknowledged"
    );
}

#[tokio::test(start_paused = true)]
async fn a_restart_replays_the_whole_of_a_partly_acknowledged_group() {
    let rig = Rig::new();
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 1)])).await;
    rig.append(part(2, 1, false, vec![file("b.yml", b"b", 1)])).await;
    rig.spool.ack(1);
    rig.spool.maintain().await;
    let rig = rig.reopen();
    assert_eq!(
        rig.replay(2).await.iter().map(|d| d.seq).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[tokio::test(start_paused = true)]
async fn incomplete_trailing_group_is_discarded_on_open_and_reported() {
    let rig = Rig::new();
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    rig.append(part(2, 0, true, vec![file("b.yml", b"b", 2)])).await;
    rig.append(part(3, 1, true, vec![file("c.yml", b"c", 3)])).await;
    let rig = rig.reopen();
    assert_eq!(rig.replay(1).await[0].seq, 1);
    assert!(rig.nothing_more_to_replay(1).await);
    let gap = rig.recovery.gap.expect("the lost versions are reported");
    assert_eq!(gap.lost_entries, 2);
    assert!(gap.from <= gap.to);
}

// ------------------------------------------------------------------------------------------------ bounds

#[tokio::test(start_paused = true)]
async fn spool_bounds_drop_oldest_and_report_gap() {
    let rig = rig_with_limits(1 << 30, 10);
    for seq in 1..=15 {
        let observed = 10_000 + 1_000 * seq as i64;
        rig.append(delta(
            seq,
            vec![file(
                &format!("svc/f{seq}.yml"),
                seq.to_string().as_bytes(),
                observed,
            )],
        ))
        .await;
        assert!(rig.spool.stats().entries <= 10, "never above the entry bound");
    }
    assert_eq!(rig.spool.stats().entries, 10);
    let got = rig.replay(10).await;
    assert_eq!(
        got.iter().map(|d| d.seq).collect::<Vec<_>>(),
        (6..=15).collect::<Vec<_>>()
    );
    // The oldest five were dropped. The first message says so, with the time range of what is gone.
    let gap = got[0].gap.expect("the gap rides on the first message");
    assert_eq!(gap.lost_entries, 5);
    assert_eq!(gap.from.unix_millis(), 11_000);
    assert_eq!(gap.to.unix_millis(), 15_000);
    assert!(got[1..].iter().all(|d| d.gap.is_none()), "once is enough");
    assert_eq!(rig.spool.stats().lost_entries_total, 5);
}

#[tokio::test(start_paused = true)]
async fn the_gap_is_repeated_until_the_hub_acknowledges_the_message_that_carried_it() {
    let rig = rig_with_limits(1 << 30, 2);
    for seq in 1..=4 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    // A connection that dies before acknowledging it: the next one carries it again.
    let first = rig.replay(2).await;
    assert!(first[0].gap.is_some());
    let second = rig.replay(2).await;
    assert_eq!(second[0].gap, first[0].gap);
    rig.spool.ack(second[0].seq);
    let third = rig.replay(1).await;
    assert!(
        third.iter().all(|d| d.gap.is_none()),
        "acknowledged, so it is not sent again"
    );
    assert!(rig.spool.gap().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_delta_is_dropped_whole_never_cut_in_the_middle() {
    let rig = rig_with_limits(1 << 30, 4);
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 100)])).await;
    rig.append(part(2, 1, true, vec![file("b.yml", b"b", 200)])).await;
    rig.append(part(3, 2, false, vec![file("c.yml", b"c", 300)]))
        .await;
    rig.append(delta(4, vec![file("d.yml", b"d", 400)])).await;
    // Five entries do not fit in four: the oldest delta goes, all three parts of it.
    rig.append(delta(5, vec![file("e.yml", b"e", 500)])).await;
    let got = rig.replay(2).await;
    assert_eq!(
        got.iter().map(|d| (d.seq, d.part)).collect::<Vec<_>>(),
        [(4, 0), (5, 0)]
    );
    let gap = got[0].gap.unwrap();
    assert_eq!(gap.lost_entries, 3);
    assert_eq!((gap.from.unix_millis(), gap.to.unix_millis()), (100, 300));
}

#[tokio::test(start_paused = true)]
async fn a_delta_that_cannot_fit_even_alone_is_dropped_and_reported() {
    let rig = rig_with_limits(1 << 30, 2);
    assert_eq!(
        rig.spool
            .append(part(1, 0, true, vec![file("a.yml", b"a", 10)]))
            .await
            .unwrap(),
        Appended::Stored
    );
    assert_eq!(
        rig.spool
            .append(part(2, 1, true, vec![file("b.yml", b"b", 20)]))
            .await
            .unwrap(),
        Appended::Stored
    );
    assert_eq!(
        rig.spool
            .append(part(3, 2, false, vec![file("c.yml", b"c", 30)]))
            .await
            .unwrap(),
        Appended::Discarded
    );
    assert_eq!(
        rig.spool.stats().entries,
        0,
        "the parts already written went with it"
    );
    rig.append(delta(4, vec![file("d.yml", b"d", 40)])).await;
    let got = rig.replay(1).await;
    assert_eq!(got[0].seq, 4);
    let gap = got[0].gap.unwrap();
    assert_eq!(
        (gap.lost_entries, gap.from.unix_millis(), gap.to.unix_millis()),
        (3, 10, 30)
    );
}

#[tokio::test(start_paused = true)]
async fn a_record_bigger_than_the_whole_spool_is_dropped_and_reported() {
    let rig = Rig::with_limits(SpoolLimits::new(8 * 1024, 1_000).with_segment_bytes(4 * 1024));
    let big = vec![b'z'; 32 * 1024];
    assert_eq!(
        rig.spool
            .append(delta(1, vec![file("big.yml", &big, 7)]))
            .await
            .unwrap(),
        Appended::Discarded
    );
    assert_eq!(rig.spool.stats().disk_bytes, 0);
    rig.append(delta(2, vec![file("small.yml", b"s", 8)])).await;
    assert_eq!(rig.replay(1).await[0].gap.unwrap().lost_entries, 1);
}

#[tokio::test(start_paused = true)]
async fn the_byte_bound_holds_on_disk() {
    let limit = 64 * 1024;
    let rig = Rig::with_limits(SpoolLimits::new(limit, 1_000_000).with_segment_bytes(8 * 1024));
    let content = vec![b'q'; 3_000];
    for seq in 1..=100 {
        rig.append(delta(
            seq,
            vec![file(&format!("f{seq}.yml"), &content, seq as i64)],
        ))
        .await;
        let on_disk: u64 = rig
            .segments()
            .iter()
            .map(|p| std::fs::metadata(p).unwrap().len())
            .sum();
        assert!(on_disk <= limit, "{on_disk} bytes on disk after {seq} deltas");
        assert!(rig.spool.stats().disk_bytes <= limit);
    }
    let got = rig.replay(1).await;
    assert!(got[0].seq > 1, "the oldest were dropped");
    assert!(got[0].gap.is_some());
}

#[tokio::test(start_paused = true)]
async fn the_gap_range_always_runs_forwards_whatever_order_the_times_come_in() {
    // The hub refuses a delta whose gap ends before it starts, entries included (the contract notes). So the range is
    // built with min and max, never with "first seen" and "last seen".
    let rig = rig_with_limits(1 << 30, 3);
    let times = [900_i64, 100, 500, 50, 800, 300, 700];
    for (i, t) in times.iter().enumerate() {
        rig.append(delta(i as u64 + 1, vec![file(&format!("f{i}.yml"), b"x", *t)]))
            .await;
    }
    let got = rig.replay(3).await;
    let gap = got[0].gap.unwrap();
    assert!(gap.from <= gap.to, "{gap:?}");
    assert_eq!((gap.from.unix_millis(), gap.to.unix_millis()), (50, 900));
    assert_eq!(gap.lost_entries, 4);
    // And it survives the wire conversion the hub applies.
    let wire = proto::convert::FromAgent::Delta(got[0].clone()).into_proto();
    assert!(proto::convert::FromAgent::from_proto(wire).unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn a_gap_survives_a_restart_before_it_was_delivered() {
    let rig = rig_with_limits(1 << 30, 2);
    for seq in 1..=4 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    let owed = rig.spool.gap().unwrap();
    let rig = rig.reopen();
    assert_eq!(rig.recovery.gap, Some(owed));
    assert_eq!(rig.replay(1).await[0].gap, Some(owed));
}

#[tokio::test(start_paused = true)]
async fn lowering_the_limits_between_runs_drops_the_oldest_on_open() {
    let rig = Rig::new();
    for seq in 1..=6 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    let dir = {
        let Rig { dir, spool, .. } = rig;
        drop(spool);
        dir
    };
    let rig = Rig::open_in(dir, SpoolLimits::new(1 << 30, 4).with_segment_bytes(4 * 1024));
    assert_eq!(rig.spool.stats().entries, 4);
    let got = rig.replay(4).await;
    assert_eq!(got[0].seq, 3);
    assert_eq!(got[0].gap.unwrap().lost_entries, 2);
}

// ------------------------------------------------------------------------------------------------ outage

#[tokio::test(start_paused = true)]
async fn spool_replays_a_b_c_in_order_after_one_hour_outage() {
    let rig = Rig::new();
    let version = |n: u64, content: &[u8]| {
        let observed = rig.clock.now().unix_millis();
        delta(n, vec![file("svc/app.yml", content, observed)])
    };
    // The hub is down for an hour. The file is A, then B twenty minutes later, then C.
    rig.append(version(1, b"A")).await;
    tokio::time::advance(Duration::from_secs(20 * 60)).await;
    rig.append(version(2, b"B")).await;
    tokio::time::advance(Duration::from_secs(20 * 60)).await;
    rig.append(version(3, b"C")).await;
    tokio::time::advance(Duration::from_secs(20 * 60)).await;

    let got = rig.replay_with(3, |d| rig.spool.ack(d.seq)).await;
    let content: Vec<&[u8]> = got
        .iter()
        .map(|d| d.entries[0].bytes.as_deref().unwrap())
        .collect();
    assert_eq!(content, [b"A".as_slice(), b"B", b"C"]);
    let times: Vec<i64> = got
        .iter()
        .map(|d| d.entries[0].observed_at.unix_millis())
        .collect();
    assert!(times[0] < times[1] && times[1] < times[2], "{times:?}");
    assert_eq!(times[1] - times[0], 20 * 60 * 1000);
    assert_eq!(got.last().unwrap().new_root, root(3));
    assert!(got.iter().all(|d| d.gap.is_none()), "nothing was lost");
    rig.spool.maintain().await;
    assert_eq!(rig.spool.stats().records, 0, "everything acknowledged is gone");
}

#[tokio::test(start_paused = true)]
async fn what_the_hub_never_acknowledged_comes_again_on_the_next_connection() {
    let rig = Rig::new();
    for seq in 1..=3 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    let first = rig.replay(3).await;
    rig.spool.ack(first[0].seq);
    rig.spool.maintain().await;
    let second = rig.replay(2).await;
    assert_eq!(second.iter().map(|d| d.seq).collect::<Vec<_>>(), [2, 3]);
}

#[tokio::test(start_paused = true)]
async fn a_message_appended_while_connected_is_sent_without_a_new_connection() {
    let rig = Rig::new();
    let (outbox, mut rx) = agent::transport::outbox::channel(agent::transport::OutboxLimits::default());
    let pump = tokio::spawn(rig.spool.attach(outbox).run());
    tokio::task::yield_now().await;
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    let queued = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let (message, _) = queued.into_parts();
    let Some(proto::convert::FromAgent::Delta(d)) = proto::convert::FromAgent::from_proto(message).unwrap()
    else {
        panic!("a delta");
    };
    assert_eq!(d.seq, 1);
    pump.abort();
}

#[tokio::test(start_paused = true)]
async fn the_pump_stops_sending_new_messages_when_too_many_are_unacknowledged() {
    let rig = Rig::new();
    for seq in 1..=300 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", 1)]))
            .await;
    }
    let (outbox, mut rx) = agent::transport::outbox::channel(agent::transport::OutboxLimits {
        messages: 1_000,
        bytes: 64 * 1024 * 1024,
    });
    let pump = tokio::spawn(rig.spool.attach(outbox).run());
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut queued = 0;
    while rx.try_recv().is_some() {
        queued += 1;
    }
    assert!(
        queued > 0 && queued < 300,
        "{queued} sent before any acknowledgement"
    );
    // Acknowledging lets it go on.
    for seq in 1..=queued {
        rig.spool.ack(seq as u64);
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(rx.try_recv().is_some());
    pump.abort();
}

// ------------------------------------------------------------------------------------------------ seq

#[tokio::test(start_paused = true)]
async fn spool_seq_monotonic_across_restart() {
    let rig = Rig::new();
    let seq = rig.spool.seq();
    let first = seq.next();
    assert!(first >= 1);
    let mut last = first;
    for _ in 0..5 {
        last = seq.next();
        rig.append(delta(last, vec![file(&format!("f{last}.yml"), b"x", 1)]))
            .await;
    }
    let rig = rig.reopen();
    assert!(
        rig.spool.seq().peek() > last,
        "{} <= {last}",
        rig.spool.seq().peek()
    );
}

#[tokio::test(start_paused = true)]
async fn seq_continues_after_everything_was_acknowledged_and_removed() {
    // An empty spool remembers nothing in its segments, so the counter has to be kept somewhere else.
    let rig = Rig::new();
    let seq = rig.spool.seq();
    let mut last = 0;
    for _ in 0..3 {
        last = seq.next();
        rig.append(delta(last, vec![file(&format!("f{last}.yml"), b"x", 1)]))
            .await;
    }
    rig.replay_with(3, |d| rig.spool.ack(d.seq)).await;
    rig.spool.maintain().await;
    assert!(rig.segments().is_empty());
    let rig = rig.reopen();
    assert!(rig.spool.seq().peek() > last);
}

#[tokio::test(start_paused = true)]
async fn seq_never_repeats_when_most_numbers_went_to_deltas_that_were_not_spooled() {
    // Answers to the hub's own requests use the same counter and are never spooled. A restart must not hand out a
    // number that one of them used.
    let rig = Rig::new();
    let seq = rig.spool.seq();
    let mut highest = 0;
    for _ in 0..10_000 {
        highest = seq.next();
        if highest % 500 == 0 {
            rig.spool.maintain().await;
        }
    }
    let rig = rig.reopen();
    assert!(rig.spool.seq().peek() > highest);
}

#[tokio::test(start_paused = true)]
async fn a_damaged_state_file_still_cannot_make_the_counter_go_back() {
    let rig = Rig::new();
    let seq = rig.spool.seq();
    let mut last = 0;
    for _ in 0..4 {
        last = seq.next();
        rig.append(delta(last, vec![file(&format!("f{last}.yml"), b"x", 1)]))
            .await;
    }
    let path = rig.path().join("state");
    assert!(path.exists());
    std::fs::write(&path, b"garbage").unwrap();
    let rig = rig.reopen();
    assert!(rig.spool.seq().peek() > last);
}

// ------------------------------------------------------------------------------------------------ crashes

#[tokio::test(start_paused = true)]
async fn spool_corrupt_tail_skipped_and_reported() {
    let rig = Rig::new();
    for seq in 1..=3 {
        rig.append(delta(
            seq,
            vec![file(&format!("f{seq}.yml"), b"some content", 1_000 * seq as i64)],
        ))
        .await;
    }
    let segments = rig.segments();
    assert_eq!(segments.len(), 1);
    let mut bytes = std::fs::read(&segments[0]).unwrap();
    let last = bytes.len() - 3;
    bytes[last] ^= 0xFF;
    std::fs::write(&segments[0], &bytes).unwrap();

    let rig = rig.reopen();
    assert_eq!(rig.recovery.damaged_segments, 1);
    let owed = rig.recovery.gap.expect("a damaged tail is reported to the hub");
    assert!(owed.lost_entries >= 1 && owed.from <= owed.to, "{owed:?}");
    let got = rig.replay(2).await;
    assert_eq!(got.iter().map(|d| d.seq).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(got[0].gap, Some(owed));
    assert!(rig.nothing_more_to_replay(2).await);
    assert_eq!(rig.spool.stats().damaged_total, 1);
}

#[tokio::test(start_paused = true)]
async fn damage_is_reported_once_not_at_every_restart() {
    let rig = Rig::new();
    for seq in 1..=3 {
        rig.append(delta(
            seq,
            vec![file(&format!("f{seq}.yml"), b"some content", seq as i64)],
        ))
        .await;
    }
    let path = rig.segments().remove(0);
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 3;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let rig = rig.reopen();
    assert_eq!(rig.recovery.damaged_segments, 1);
    let owed = rig.recovery.gap.unwrap();
    // The damaged tail is cut off, so the next start finds a clean file, and the loss is not counted again.
    let rig = rig.reopen();
    assert_eq!(rig.recovery.damaged_segments, 0);
    assert_eq!(rig.recovery.gap, Some(owed));
    assert_eq!(rig.replay(2).await.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn acknowledged_records_are_not_sent_again_after_a_restart() {
    // They are still in the active segment, which stays until it is full or empty. The floor in the state file says they
    // are settled.
    let rig = Rig::new();
    for seq in 1..=4 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    rig.spool.ack(1);
    rig.spool.ack(2);
    rig.spool.maintain().await;
    assert_eq!(rig.segments().len(), 1, "the segment still holds 3 and 4");
    let rig = rig.reopen();
    assert_eq!(
        rig.replay(2).await.iter().map(|d| d.seq).collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(rig.nothing_more_to_replay(2).await);
}

#[tokio::test(start_paused = true)]
async fn records_dropped_to_make_room_are_not_counted_lost_a_second_time_after_a_restart() {
    let rig = rig_with_limits(1 << 30, 2);
    for seq in 1..=4 {
        rig.append(delta(seq, vec![file(&format!("f{seq}.yml"), b"x", seq as i64)]))
            .await;
    }
    let owed = rig.spool.gap().unwrap();
    assert_eq!(owed.lost_entries, 2);
    let rig = rig.reopen().reopen();
    assert_eq!(
        rig.recovery.gap,
        Some(owed),
        "still two, however often it restarts"
    );
}

#[tokio::test(start_paused = true)]
async fn damage_in_the_middle_of_a_segment_costs_what_follows_it_and_no_more() {
    let rig = Rig::new();
    for seq in 1..=4 {
        rig.append(delta(
            seq,
            vec![file(&format!("f{seq}.yml"), b"some content", seq as i64)],
        ))
        .await;
    }
    // Flip a byte inside the second record.
    let path = rig.segments().remove(0);
    let mut bytes = std::fs::read(&path).unwrap();
    let record_len = (bytes.len() - 8) / 4;
    bytes[8 + record_len + record_len / 2] ^= 0x55;
    std::fs::write(&path, &bytes).unwrap();
    let rig = rig.reopen();
    assert_eq!(rig.replay(1).await[0].seq, 1);
    assert!(rig.nothing_more_to_replay(1).await);
    assert_eq!(rig.recovery.damaged_segments, 1);
}

#[tokio::test(start_paused = true)]
async fn a_damaged_segment_does_not_stop_the_ones_after_it() {
    let rig = Rig::with_limits(SpoolLimits::new(1 << 30, 1_000).with_segment_bytes(300));
    for seq in 1..=6 {
        rig.append(delta(
            seq,
            vec![file(&format!("f{seq}.yml"), &[b'a'; 100], seq as i64)],
        ))
        .await;
    }
    let segments = rig.segments();
    assert!(segments.len() >= 3, "{} segments", segments.len());
    // Wreck the header of the first one.
    let mut bytes = std::fs::read(&segments[0]).unwrap();
    bytes[0] ^= 0xFF;
    std::fs::write(&segments[0], &bytes).unwrap();
    let rig = rig.reopen();
    assert_eq!(rig.recovery.damaged_segments, 1);
    let got = rig.replay(5).await;
    assert_eq!(got.iter().map(|d| d.seq).collect::<Vec<_>>(), [2, 3, 4, 5, 6]);
    assert!(got[0].gap.is_some(), "the first segment's loss is reported");
}

#[tokio::test(start_paused = true)]
async fn spool_truncation_at_every_offset_recovers_prefix() {
    // A crash can leave the file cut anywhere. Whatever the cut, the spool opens, holds exactly the deltas that were
    // whole before it, and says so when something was cut.
    let rig = Rig::with_limits(SpoolLimits::new(1 << 30, 1_000).with_segment_bytes(1 << 20));
    let mut ends = Vec::new(); // (file length after the delta, last seq of the delta)
    let segment = |rig: &Rig| rig.segments().remove(0);
    let steps: Vec<Vec<domain::ScanDelta>> = vec![
        vec![delta(1, vec![file("a.yml", b"alpha", 10)])],
        vec![
            part(2, 0, true, vec![file("b.yml", b"beta", 20)]),
            part(3, 1, false, vec![file("c.yml", b"gamma", 30)]),
        ],
        vec![removal(4, "a.yml")],
        vec![delta(
            5,
            vec![file("d.yml", &[b'd'; 700], 50), file("e.yml", b"e", 50)],
        )],
    ];
    for step in steps {
        let last = step.last().unwrap().seq;
        for d in step {
            rig.append(d).await;
        }
        ends.push((std::fs::metadata(segment(&rig)).unwrap().len(), last));
    }
    let full = std::fs::read(segment(&rig)).unwrap();
    let name = segment(&rig).file_name().unwrap().to_owned();
    let state = std::fs::read(rig.path().join("state")).ok();
    drop(rig);

    for cut in 0..=full.len() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(&name), &full[..cut]).unwrap();
        if let Some(state) = &state {
            std::fs::write(dir.path().join("state"), state).unwrap();
        }
        let rig = Rig::open_in(dir, SpoolLimits::new(1 << 30, 1_000).with_segment_bytes(1 << 20));
        let expected: Vec<u64> = ends
            .iter()
            .take_while(|(end, _)| *end <= cut as u64)
            .flat_map(|(_, last)| match last {
                3 => vec![2, 3],
                n => vec![*n],
            })
            .collect();
        let whole = rig.spool.stats().records;
        assert_eq!(
            whole as usize,
            expected.len(),
            "cut at {cut}: {whole} records, expected {expected:?}"
        );
        if !expected.is_empty() {
            let got = rig.replay(expected.len()).await;
            assert_eq!(
                got.iter().map(|d| d.seq).collect::<Vec<_>>(),
                expected,
                "cut at {cut}"
            );
        }
        let on_a_boundary = cut == 0 || cut == 8 || ends.iter().any(|(end, _)| *end == cut as u64);
        assert_eq!(
            rig.recovery.damaged_segments == 0 && rig.recovery.gap.is_none(),
            on_a_boundary,
            "cut at {cut}: damaged {} gap {:?}",
            rig.recovery.damaged_segments,
            rig.recovery.gap
        );
        assert!(rig.recovery.gap.is_none_or(|g| g.from <= g.to));
    }
}

/// Writes only part of a frame, then plays dead.
struct CrashDuring {
    frame: AtomicUsize,
    crash_on: usize,
    keep: usize,
    syncs: Arc<AtomicUsize>,
}

impl SpoolHooks for CrashDuring {
    fn torn_write(&self, _len: usize) -> Option<usize> {
        let n = self.frame.fetch_add(1, Ordering::SeqCst) + 1;
        (n == self.crash_on).then_some(self.keep)
    }

    fn synced(&self) {
        self.syncs.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn spool_kill9_recovers_synced_prefix() {
    // The process dies in the middle of writing the fifth frame, which is the second part of a delta. Everything that
    // was synced before is there afterwards; the half-written delta is reported as lost.
    let syncs = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(CrashDuring {
        frame: AtomicUsize::new(0),
        crash_on: 5,
        keep: 30,
        syncs: syncs.clone(),
    });
    let dir = tempfile::TempDir::new().unwrap();
    let rig = Rig::open_with(
        dir,
        SpoolLimits::new(1 << 30, 1_000).with_segment_bytes(1 << 20),
        |o| o.with_hooks(hooks),
    );
    rig.append(delta(1, vec![file("a.yml", b"a", 10)])).await;
    rig.append(delta(2, vec![file("b.yml", b"b", 20)])).await;
    rig.append(delta(3, vec![file("c.yml", b"c", 30)])).await;
    rig.append(part(4, 0, true, vec![file("d.yml", b"d", 40)])).await;
    let died = rig
        .spool
        .append(part(5, 1, false, vec![file("e.yml", b"e", 50)]))
        .await;
    assert!(died.is_err(), "the write was cut short");
    // Once dead, always dead: a process that was killed does not append again.
    assert!(
        rig.spool
            .append(delta(6, vec![file("f.yml", b"f", 60)]))
            .await
            .is_err()
    );

    let rig = rig.reopen();
    let got = rig.replay(3).await;
    assert_eq!(got.iter().map(|d| d.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(rig.nothing_more_to_replay(3).await);
    assert_eq!(rig.recovery.damaged_segments, 1);
    assert!(rig.recovery.gap.unwrap().lost_entries >= 1);
    assert!(
        rig.spool.seq().peek() > 5,
        "the numbers of the lost deltas are not reused"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_write_never_leaves_the_spool_unable_to_take_the_next_delta() {
    // An I/O error that is not a crash (the disk was full for a moment): the delta is refused, the segment that took
    // the failed write is left behind, and the next delta goes to a fresh segment, after nothing broken.
    let hooks = Arc::new(FailOnce::default());
    let dir = tempfile::TempDir::new().unwrap();
    let rig = Rig::open_with(dir, SpoolLimits::new(1 << 30, 1_000), |o| {
        o.with_hooks(hooks.clone())
    });
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    hooks.arm();
    assert!(
        rig.spool
            .append(delta(2, vec![file("b.yml", b"b", 2)]))
            .await
            .is_err()
    );
    rig.append(delta(3, vec![file("c.yml", b"c", 3)])).await;
    let rig = rig.reopen();
    assert_eq!(
        rig.replay(2).await.iter().map(|d| d.seq).collect::<Vec<_>>(),
        [1, 3]
    );
}

#[derive(Default)]
struct FailOnce(std::sync::atomic::AtomicBool);

impl FailOnce {
    fn arm(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl SpoolHooks for FailOnce {
    fn fail_write(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

#[tokio::test(start_paused = true)]
async fn spool_fsyncs_once_per_logical_delta() {
    let syncs = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(CrashDuring {
        frame: AtomicUsize::new(0),
        crash_on: usize::MAX,
        keep: 0,
        syncs: syncs.clone(),
    });
    let dir = tempfile::TempDir::new().unwrap();
    let rig = Rig::open_with(
        dir,
        SpoolLimits::new(1 << 30, 100_000).with_segment_bytes(1 << 20),
        |o| o.with_hooks(hooks),
    );
    let at_open = syncs.load(Ordering::SeqCst);
    rig.append(delta(1, vec![file("a.yml", b"a", 1)])).await;
    assert_eq!(
        syncs.load(Ordering::SeqCst) - at_open,
        1,
        "one single-message delta, one sync"
    );
    let before = syncs.load(Ordering::SeqCst);
    for (i, more) in [(0, true), (1, true), (2, true), (3, false)] {
        rig.append(part(
            2 + u64::from(i),
            i,
            more,
            vec![file(&format!("p{i}.yml"), b"p", 2)],
        ))
        .await;
    }
    assert_eq!(
        syncs.load(Ordering::SeqCst) - before,
        1,
        "four messages of one delta, one sync"
    );
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_before_it_is_durable() {
    // A message goes out only after the sync that made it survive a crash; otherwise the hub could be told about a
    // version the agent has already forgotten.
    let rig = Rig::new();
    rig.append(part(1, 0, true, vec![file("a.yml", b"a", 1)])).await;
    assert!(rig.nothing_more_to_replay(0).await);
}

// ------------------------------------------------------------------------------------------------ memory

#[tokio::test(start_paused = true)]
async fn spool_replay_memory_bounded() {
    // About 24 MiB in 2 MiB records. Replaying must hold one record at a time, not the spool.
    let rig = Rig::with_limits(SpoolLimits::new(512 * 1024 * 1024, 100_000));
    let content = vec![b'm'; 2 * 1024 * 1024 - 1024];
    for seq in 1..=12 {
        rig.append(delta(
            seq,
            vec![file(&format!("big/f{seq}.bin"), &content, seq as i64)],
        ))
        .await;
    }
    let got = rig.replay_with(12, |d| rig.spool.ack(d.seq)).await;
    assert_eq!(got.len(), 12);
    let held = rig.spool.stats().max_replay_buffer;
    assert!(held > 0, "the meter is not wired up");
    assert!(held <= 3 * 1024 * 1024, "the reader held {held} bytes at once");
}

// ------------------------------------------------------------------------------------------------ drained (T10)

/// Pump a spool to a fresh outbox and keep the receiving end draining, as a connection's writer does.
fn connect(
    rig: &Rig,
) -> (
    tokio::task::JoinHandle<agent::spool::PumpEnd>,
    tokio::sync::mpsc::UnboundedReceiver<u64>,
) {
    use agent::transport::outbox::{self, OutboxLimits};
    use proto::convert::FromAgent;
    let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
    let pump = tokio::spawn(rig.spool.attach(outbox).run());
    let (seen, seqs) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(queued) = rx.recv().await {
            let (message, _permit) = queued.into_parts();
            if let Some(FromAgent::Delta(d)) = FromAgent::from_proto(message).unwrap() {
                let _ = seen.send(d.seq);
            }
        }
    });
    (pump, seqs)
}

async fn wait_until_drained(rig: &Rig) {
    let mut changes = rig.spool.subscribe_drain();
    for _ in 0..1_000 {
        changes.borrow_and_update();
        if rig.spool.drained() {
            return;
        }
        tokio::time::timeout(Duration::from_secs(1), changes.changed())
            .await
            .ok();
    }
    panic!("the spool never drained");
}

#[tokio::test(start_paused = true)]
async fn a_spool_with_no_pump_is_not_drained_even_when_empty() {
    let rig = Rig::new();
    assert!(
        !rig.spool.drained(),
        "nobody is sending, so nothing has been sent"
    );
    rig.append(delta(1, vec![file("svc/a.yml", b"1", 1_000)])).await;
    assert!(!rig.spool.drained());
}

#[tokio::test(start_paused = true)]
async fn the_spool_is_drained_when_the_pump_has_sent_everything_it_holds() {
    let rig = Rig::new();
    for seq in 1..=3 {
        rig.append(delta(
            seq,
            vec![file(&format!("svc/f{seq}.yml"), b"x", 1_000 * seq as i64)],
        ))
        .await;
    }
    let (_pump, mut seqs) = connect(&rig);
    wait_until_drained(&rig).await;
    let mut sent = Vec::new();
    while let Ok(seq) = seqs.try_recv() {
        sent.push(seq);
    }
    assert_eq!(
        sent,
        [1, 2, 3],
        "drained means the pump has handed all three to the connection"
    );
}

#[tokio::test(start_paused = true)]
async fn an_empty_spool_is_drained_as_soon_as_a_pump_looks() {
    let rig = Rig::new();
    let (_pump, _seqs) = connect(&rig);
    wait_until_drained(&rig).await;
}

#[tokio::test(start_paused = true)]
async fn a_new_delta_makes_the_spool_undrained_until_it_is_sent() {
    let rig = Rig::new();
    let (_pump, mut seqs) = connect(&rig);
    wait_until_drained(&rig).await;

    rig.append(delta(1, vec![file("svc/a.yml", b"1", 1_000)])).await;
    assert!(
        !rig.spool.drained(),
        "the delta is in the spool, and the pump has not looked at it yet"
    );
    wait_until_drained(&rig).await;
    assert_eq!(seqs.recv().await, Some(1));
}

#[tokio::test(start_paused = true)]
async fn a_part_of_a_delta_that_is_not_complete_does_not_count() {
    let rig = Rig::new();
    let (_pump, _seqs) = connect(&rig);
    wait_until_drained(&rig).await;
    // The first part of two: stored, but not sendable until the last part is.
    rig.append(part(1, 0, true, vec![file("svc/a.yml", b"1", 1_000)]))
        .await;
    assert!(rig.spool.drained(), "nothing complete is waiting");
    rig.append(part(2, 1, false, vec![file("svc/b.yml", b"2", 1_001)]))
        .await;
    assert!(!rig.spool.drained());
    wait_until_drained(&rig).await;
}

#[tokio::test(start_paused = true)]
async fn the_spool_is_not_drained_once_the_pump_has_ended() {
    let rig = Rig::new();
    let (pump, _seqs) = connect(&rig);
    wait_until_drained(&rig).await;
    pump.abort();
    let _ = pump.await;
    assert!(!rig.spool.drained(), "the connection is gone");
}

#[tokio::test(start_paused = true)]
async fn a_new_pump_starts_undrained_and_replays_before_it_says_so() {
    let rig = Rig::new();
    for seq in 1..=2 {
        rig.append(delta(
            seq,
            vec![file(&format!("svc/f{seq}.yml"), b"x", 1_000 * seq as i64)],
        ))
        .await;
    }
    let (first, _seqs) = connect(&rig);
    wait_until_drained(&rig).await;
    first.abort();
    let _ = first.await;

    // The hub never acknowledged them: the next connection replays both.
    let (_second, mut seqs) = connect(&rig);
    assert!(!rig.spool.drained(), "the replay has not started");
    wait_until_drained(&rig).await;
    assert_eq!((seqs.recv().await, seqs.recv().await), (Some(1), Some(2)));
}
