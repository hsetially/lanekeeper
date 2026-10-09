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

/// A notification is delivered once, in order, and `notify` succeeds.
pub async fn notifier<N: NotifierProbe + ?Sized>(n: &N) {
    let before = n.delivered().await.len();
    let first = note(NotificationKind::ProposalPending, "Proposal waiting");
    let second = note(NotificationKind::ServiceRestarted, "Service restarted");
    n.notify(first.clone()).await.expect("notify");
    n.notify(second.clone()).await.expect("notify");
    let delivered = n.delivered().await;
    // A bounded probe may have dropped its oldest entries, so compare the tail.
    assert!(delivered.len() >= 2 && delivered.len() <= before + 2);
    assert_eq!(
        &delivered[delivered.len() - 2..],
        &[first, second],
        "delivered once, in order"
    );
}
