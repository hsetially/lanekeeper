//! Deny globs and the spool (T11, D79, S17): no bytes of a denied file are written to a segment, and a record that was
//! written before the path became denied is stripped when it is replayed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::time::Duration;

use agent::deny::DenyList;
use agent::transport::outbox::{self, OutboxLimits};
use domain::ScanDelta;
use proto::convert::FromAgent;
use support::spool_rig::{Rig, delta, file};

const MARKER: &[u8] = b"MARKER-91c0d6aa-never-in-a-segment";

/// Everything in the spool's directory, as one byte string.
fn bytes_on_the_volume(rig: &Rig) -> Vec<u8> {
    let mut all = Vec::new();
    for entry in fs::read_dir(rig.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            all.extend(fs::read(path).unwrap());
        }
    }
    all
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// What a connection would get from `spool`, as the pump sends it.
async fn replay(spool: &agent::spool::Spool, count: usize) -> Vec<ScanDelta> {
    let (outbox, mut rx) = outbox::channel(OutboxLimits::default());
    let task = tokio::spawn(spool.attach(outbox).run());
    let mut got = Vec::new();
    while got.len() < count {
        let queued = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("the pump sends within a virtual minute")
            .expect("the connection stays open");
        let (message, _permit) = queued.into_parts();
        let Some(FromAgent::Delta(d)) = FromAgent::from_proto(message).unwrap() else {
            panic!("the pump sends deltas only");
        };
        got.push(d);
    }
    task.abort();
    got
}

#[tokio::test(start_paused = true)]
async fn the_spool_never_writes_the_bytes_of_a_denied_path() {
    let rig = Rig::new();
    let spool = rig.spool.clone().with_deny(DenyList::default());
    let mut secret = MARKER.to_vec();
    secret.extend_from_slice(b" pem body");
    // A delta whose builder (wrongly, or before a glob) gave a denied path its bytes: the spool is the last guard.
    spool
        .append(delta(
            1,
            vec![
                file("keys/server.pem", &secret, 10),
                file("svc/app.yml", b"plain: 1\n", 10),
            ],
        ))
        .await
        .unwrap();

    assert!(
        !contains(&bytes_on_the_volume(&rig), MARKER),
        "the volume holds the denied file's bytes"
    );
    let got = replay(&spool, 1).await;
    let [pem, app] = &got[0].entries[..] else {
        panic!("two entries");
    };
    assert!(pem.denied && pem.bytes.is_none());
    assert_eq!(pem.size, secret.len() as u64, "name, size and hash are kept");
    assert!(!app.denied && app.bytes.as_deref() == Some(&b"plain: 1\n"[..]));
}

#[tokio::test(start_paused = true)]
async fn a_record_written_before_the_glob_is_stripped_when_it_is_replayed() {
    let rig = Rig::new();
    // Written while nothing denied `*.late.yml`: the bytes are in the record.
    let early = rig.spool.clone().with_deny(DenyList::default());
    early
        .append(delta(
            1,
            vec![
                file("svc/x.late.yml", MARKER, 10),
                file("svc/app.yml", b"a: 1\n", 10),
            ],
        ))
        .await
        .unwrap();
    let before = replay(&early, 1).await;
    assert!(
        before[0].entries[0].bytes.as_deref() == Some(MARKER),
        "the test is not blind: the early record does carry the bytes"
    );

    // The hub's configuration arrives and adds the glob. The same spool, following the agent's list.
    let list = DenyList::default();
    list.set_hub_globs(&["*.late.yml"]);
    let now = rig.spool.clone().with_deny(list);
    let after = replay(&now, 1).await;
    let [late, app] = &after[0].entries[..] else {
        panic!("two entries");
    };
    assert!(late.denied && late.bytes.is_none(), "{late:?}");
    assert_eq!(late.size, MARKER.len() as u64);
    assert!(!app.denied && app.bytes.is_some());
    assert!(!format!("{after:?}").contains("MARKER-91c0d6aa"));
}

#[tokio::test(start_paused = true)]
async fn stripping_changes_no_sequence_number_root_or_count() {
    let rig = Rig::new();
    let spool = rig.spool.clone().with_deny(DenyList::default());
    for seq in 1..=3_u64 {
        // A different content each time, so that the spool's dedup keeps all three.
        let mut content = MARKER.to_vec();
        content.push(u8::try_from(seq).unwrap());
        spool
            .append(delta(
                seq,
                vec![file("keys/a.key", &content, i64::try_from(seq).unwrap() * 10)],
            ))
            .await
            .unwrap();
    }
    let got = replay(&spool, 3).await;
    assert_eq!(got.iter().map(|d| d.seq).collect::<Vec<_>>(), [1, 2, 3]);
    for (i, d) in got.iter().enumerate() {
        assert_eq!(d.entries.len(), 1);
        assert!(d.entries[0].denied && d.entries[0].bytes.is_none());
        assert_eq!(
            d.new_root,
            support::spool_rig::root(u64::try_from(i).unwrap() + 1)
        );
    }
}
