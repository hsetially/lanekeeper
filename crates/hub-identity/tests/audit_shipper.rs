//! T5 (S9, S10, decision B8): the copy of the audit log in Cloud Logging.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use hub_identity::audit::PgAuditLog;
use hub_identity::audit::shipper::{AuditLogShipper, ShipOutcome};
use ports::conformance::sample::{hash, nfs, swimlane};
use ports::{AuditLog, Leases, Tx};
use sha2::{Digest, Sha256};
use sqlx::Row;
use support::audit_rig::{AuditRig, event};
use support::log_capture;

fn shipper(rig: &AuditRig) -> AuditLogShipper {
    AuditLogShipper::new(rig.pool.clone(), rig.leases.clone(), rig.metrics.clone())
}

fn lines(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| l.contains("lanekeeper::audit") && l.contains("audit event"))
        .collect()
}

async fn hashes(rig: &AuditRig) -> Vec<String> {
    sqlx::query("SELECT encode(hash, 'hex') AS h FROM audit_events ORDER BY seq")
        .fetch_all(&rig.pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get("h"))
        .collect()
}

#[tokio::test]
async fn shipper_logs_only_committed_events() {
    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    let shipper = shipper(&rig);

    // Recorded, not committed: the chain lock is held and the row is invisible to everyone else.
    let mut tx = rig.pool.begin().await.unwrap();
    PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), event(1))
        .await
        .unwrap();
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 0 });
    assert!(
        lines(&capture.text()).is_empty(),
        "a line for an event that may still roll back"
    );

    tx.commit().await.unwrap();
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 1 });
    assert_eq!(lines(&capture.text()).len(), 1);

    // A rolled-back event never appears.
    let mut tx = rig.pool.begin().await.unwrap();
    PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), event(2))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 0 });
    assert_eq!(lines(&capture.text()).len(), 1);
    assert_eq!(rig.metrics.audit_lines_shipped_total(), 1);
}

#[tokio::test]
async fn each_event_is_shipped_once_in_batches() {
    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    rig.record(5).await;
    let shipper = shipper(&rig).with_batch(2);
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 2 });
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 2 });
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 1 });
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 0 });
    let log = capture.text();
    let shipped = lines(&log);
    assert_eq!(shipped.len(), 5);
    // In chain order, each hash once.
    for (line, hash) in shipped.iter().zip(hashes(&rig).await) {
        assert!(line.contains(&format!("audit_event_hash={hash}")), "{line}");
    }
    rig.record(1).await;
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 1 });
}

#[tokio::test]
async fn shipper_line_has_event_hash_and_no_diff_text() {
    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    let secret = "password: SECRET-DIFF-TEXT-8812";
    let diff = format!("-a: 1\n+{secret}\n");
    let mut e = event(1);
    e.swimlane = Some(swimlane("sit1"));
    e.path = Some(nfs("a/b.yml"));
    e.hash_before = Some(hash(b"x"));
    e.hash_after = Some(hash(b"y"));
    e.diff = Some(diff.clone());
    let mut tx = rig.pool.begin().await.unwrap();
    PgAuditLog::new()
        .record(&mut Tx::from_pg(&mut tx), e)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    shipper(&rig).tick().await.unwrap();
    let log = capture.text();
    let line = lines(&log).pop().expect("a line").to_owned();
    let stored = hashes(&rig).await.remove(0);
    assert!(line.contains(&format!("audit_event_hash={stored}")), "{line}");
    assert!(line.contains("seq=1"), "{line}");
    assert!(
        line.contains("action=SettingsChanged") && line.contains("actor_kind=user"),
        "{line}"
    );
    assert!(
        line.contains("swimlane=sit1") && line.contains("path=a/b.yml"),
        "{line}"
    );
    assert!(line.contains(&format!("hash_after={}", hash(b"y"))), "{line}");
    assert!(line.contains(&format!("diff_len={}", diff.len())), "{line}");
    let digest = hex_of(&Sha256::digest(diff.as_bytes()));
    assert!(line.contains(&format!("diff_sha256={digest}")), "{line}");
    assert!(
        !log.contains("SECRET-DIFF-TEXT"),
        "the diff reached the log: {log}"
    );
}

fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

#[tokio::test]
async fn shipper_resumes_from_last_checkpoint_and_dedupes_by_hash() {
    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    rig.record(3).await;
    rig.checkpoint_job().run_once().await.unwrap();
    rig.record(2).await;
    let all = hashes(&rig).await;

    // A shipper that starts now begins at the checkpoint: it does not replay the whole history.
    let first = shipper(&rig);
    assert_eq!(first.tick().await.unwrap(), ShipOutcome::Shipped { lines: 2 });
    // The replica goes away and another takes over. It repeats the last stretch, and the repeated lines carry the same hashes,
    // which is what the log backend dedupes on.
    first.shutdown().await;
    let second = shipper(&rig);
    assert_eq!(second.tick().await.unwrap(), ShipOutcome::Shipped { lines: 2 });

    let log = capture.text();
    let shipped = lines(&log);
    assert_eq!(shipped.len(), 4);
    for (i, line) in shipped.iter().enumerate() {
        let want = &all[3 + (i % 2)];
        assert!(line.contains(&format!("audit_event_hash={want}")), "{line}");
    }
    assert!(
        !log.contains(&format!("audit_event_hash={}", all[0])),
        "events before the checkpoint are not replayed"
    );
}

#[tokio::test]
async fn shipper_requires_the_lease() {
    let (capture, _guard) = log_capture::install();
    let rig = AuditRig::new(db!()).await;
    rig.record(2).await;
    let other = rig
        .leases
        .try_acquire("audit.shipper", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    let shipper = Arc::new(shipper(&rig));
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::NotLeader);
    assert!(lines(&capture.text()).is_empty());
    other.release().await.unwrap();
    assert_eq!(shipper.tick().await.unwrap(), ShipOutcome::Shipped { lines: 2 });
}
