//! The notification an admin gets when someone asks for access (T2, Q9).
//!
//! Built after the transaction commits and sent with a timeout. A notification problem is logged by variant and never fails
//! or rolls back the sign-in or request that caused it (`Notifier` contract).

use std::sync::Arc;
use std::time::Duration;

use domain::{Role, Severity, ShortText, User};
use ports::{Notification, NotificationKind, Notifier};

/// A chat webhook that does not answer must not hold a sign-in for long.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(5);
/// The name shown in the summary is cut here, so the summary always fits a [`ShortText`].
const NAME_BYTES: usize = 100;
/// Where the admin page lives in the web app (a path, never a URL).
const ADMIN_LINK: &str = "/admin/access-requests";

#[derive(Clone)]
pub struct AccessNotifier {
    inner: Arc<dyn Notifier>,
}

impl std::fmt::Debug for AccessNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessNotifier").finish_non_exhaustive()
    }
}

impl AccessNotifier {
    pub fn new(inner: Arc<dyn Notifier>) -> Self {
        Self { inner }
    }

    /// Tell the admins that `user` waits for a decision. `requested` is the role asked for, if the user named one.
    pub async fn access_requested(&self, user: &User, requested: Option<Role>) {
        let shown = if user.display_name.as_str().is_empty() {
            user.email.as_str()
        } else {
            user.display_name.as_str()
        };
        let who = crate::users::clip_text(shown, NAME_BYTES);
        let summary = match requested {
            None => format!("{who} signed in for the first time and waits for a role"),
            Some(role) => format!("{who} asked for the {role} role"),
        };
        let (Ok(title), Ok(summary), Ok(link)) = (
            ShortText::parse("Access requested"),
            ShortText::parse(&summary),
            ShortText::parse(ADMIN_LINK),
        ) else {
            tracing::warn!("access notification could not be built");
            return;
        };
        let note = Notification {
            kind: NotificationKind::AccessRequested,
            severity: Severity::Low,
            swimlane: None,
            title,
            summary,
            link: Some(link),
        };
        match tokio::time::timeout(NOTIFY_TIMEOUT, self.inner.notify(note)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "access notification was not delivered"),
            Err(_) => tracing::warn!("access notification timed out"),
        }
    }
}
