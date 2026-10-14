//! The notifier port: Teams webhooks and similar (D-notifications). Never fails the caller's action.

use async_trait::async_trait;
use domain::{Severity, ShortText, SwimlaneId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    AccessRequested,
    ProposalPending,
    ProposalDecided,
    ServiceRestarted,
    PrRaised,
    AgentDisconnected,
    CriticalFinding,
    /// Drift was detected in a swimlane. Held while a sync window is open and evaluated once, when it closes (D75).
    DriftDetected,
    /// A sync window has been open for over two hours: the Job may be stuck (D75).
    SyncWindowStale,
}

/// A message for a person. Contains names, ids and counts, never file content or a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub kind: NotificationKind,
    pub severity: Severity,
    pub swimlane: Option<SwimlaneId>,
    pub title: ShortText,
    pub summary: ShortText,
    /// A path inside the web app, for example `/swimlanes/sit1`. Never an absolute URL.
    pub link: Option<ShortText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotifyError {
    #[error("notification channel is unavailable")]
    Unavailable,
    #[error("notification was rejected by the channel")]
    Rejected,
}

#[async_trait]
pub trait Notifier: Send + Sync + 'static {
    /// Callers log a failure and carry on: a notification problem must never fail or roll back the action
    /// that caused it.
    async fn notify(&self, n: Notification) -> Result<(), NotifyError>;
}
