use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use domain::{Severity, ShortText};

use super::sample::swimlane;
use crate::{Notification, NotificationKind, Notifier};

/// What the notifier conformance suite needs to see.
#[async_trait]
pub trait NotifierProbe: Notifier {
    /// Everything delivered to the channel so far, oldest first.
    async fn delivered(&self) -> Vec<Notification>;
}

fn note(kind: NotificationKind, title: &str) -> Notification {
    Notification {
        kind,
        severity: Severity::Medium,
        swimlane: Some(swimlane("sit1")),
        title: ShortText::parse(title).unwrap(),
        summary: ShortText::parse("a summary").unwrap(),
        link: Some(ShortText::parse("/swimlanes/sit1").unwrap()),
    }
}

/// A number no other call of [`notifier`] in this process has used, so that the titles of one call are never the titles of an
/// earlier call on the same probe.
static RUN: AtomicU64 = AtomicU64::new(0);

/// A notification is delivered once, in order, and `notify` succeeds. Every kind is deliverable, the ones added for the
/// registry (`DriftDetected`, `SyncWindowStale`) included.
///
/// The probe may already hold notifications (from an earlier call, or from other tests that share it). The titles of this call
/// are its own, so the last notification can only be in the probe if this call delivered it, and a `notify` that drops what it is
/// given cannot pass on what an earlier call left behind.
pub async fn notifier<N: NotifierProbe + ?Sized>(n: &N) {
    let before = n.delivered().await.len();
    let run = RUN.fetch_add(1, Ordering::Relaxed);
    let titled = |kind, what: &str| note(kind, &format!("{what} (run {run})"));
    // One of every kind, in the order of the enum.
    let sent = [
        titled(NotificationKind::AccessRequested, "Access requested"),
        titled(NotificationKind::ProposalPending, "Proposal waiting"),
        titled(NotificationKind::ProposalDecided, "Proposal decided"),
        titled(NotificationKind::ServiceRestarted, "Service restarted"),
        titled(NotificationKind::PrRaised, "PR raised"),
        titled(NotificationKind::AgentDisconnected, "Agent disconnected"),
        titled(NotificationKind::CriticalFinding, "Critical finding"),
        titled(NotificationKind::DriftDetected, "Drift detected"),
        titled(NotificationKind::SyncWindowStale, "Sync window open too long"),
    ];
    for note in &sent {
        n.notify(note.clone()).await.expect("notify");
    }
    let delivered = n.delivered().await;
    // A bounded probe may have dropped its oldest entries, so compare the tail: whatever it kept, it kept in order, once.
    let tail_len = delivered.len().min(sent.len());
    assert!(tail_len > 0 && delivered.len() <= before + sent.len());
    assert_eq!(
        &delivered[delivered.len() - tail_len..],
        &sent[sent.len() - tail_len..],
        "delivered once, in order"
    );
}
