//! The notification channel, until the write paths bring Teams (plan 18, T4 and T5; D-notifications).
//!
//! The identity module tells the admins when someone asks for access, and when the nightly verification of the audit chain finds
//! a mismatch. Both go through the [`Notifier`] port and both are written never to fail the action that caused them. The channel
//! itself, Teams, belongs to the write paths (T5); until they are wired, this is what the identity module is given: a notifier that
//! says it cannot deliver, and logs that it did not. It stores nothing, delivers nothing and invents nothing, so a hub that runs
//! on it loses the notification and not the event: the access request is in the table the admin page lists, and a failed
//! verification is in `audit_verify_failures_total`.

use async_trait::async_trait;
use ports::{Notification, NotificationKind, Notifier, NotifyError};

/// A notifier with no channel behind it. Every call is [`NotifyError::Unavailable`].
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableNotifier;

#[async_trait]
impl Notifier for UnavailableNotifier {
    async fn notify(&self, notification: Notification) -> Result<(), NotifyError> {
        // The kind is a closed set; the title and the summary are the notification's content and stay out of the log.
        let kind: NotificationKind = notification.kind;
        tracing::warn!(
            ?kind,
            "no notification channel is wired; the notification was not delivered"
        );
        Err(NotifyError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use domain::{Severity, ShortText};

    use super::*;

    #[tokio::test]
    async fn nothing_is_delivered_and_the_caller_is_told() {
        let note = Notification {
            kind: NotificationKind::AccessRequested,
            severity: Severity::Low,
            swimlane: None,
            title: ShortText::parse("Access requested").unwrap(),
            summary: ShortText::parse("Ada signed in for the first time").unwrap(),
            link: None,
        };
        assert_eq!(
            UnavailableNotifier.notify(note).await.unwrap_err(),
            NotifyError::Unavailable
        );
    }
}
