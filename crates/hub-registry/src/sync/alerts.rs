//! Drift alerts (T8, D49, D75, D76): what the hub tells people about drift, and when.
//!
//! # Two paths, one rule
//!
//! - **Live.** The pipeline commits a swimlane's findings and offers the ones that just became drift worth knowing about
//!   ([`Alerts::offer`]). If a sync window holds the swimlane ([`super::hold`]) the alert waits. Otherwise one notification goes out for
//!   each severity, from a task of its own, so that a slow or dead notifier never slows the pipeline down.
//! - **At a window's close.** The watch job processes the closed window ([`super::window`]): the sync is recorded, C7 comes first, and
//!   then [`Alerts::evaluate`] reads what the swimlane looks like now and sends **one** notification for each severity, whatever
//!   the number of files. That is "evaluate once" of D75: 600 files that a sync rewrote and 100 of them that still differ from Git are one
//!   message about 100 files, not 700 messages, and not a message about the 500 that the sync put right.
//!
//! # What is drift worth an alert
//!
//! A file whose drift state says that **NFS has something Git does not**: `nfs_ahead`, `conflict`, `untracked`
//! ([`alertable`]). `git_ahead` is not: it is a change waiting for its sync, which is what every push to the base repository makes of
//! all forty swimlanes, and a sync is what resolves it. Neither is `unknown`, which in an adopted swimlane is almost always a file that
//! Git has and NFS has not yet (no baseline for it, N missing); it is a finding in the list, not an alert (a residual risk: a file
//! created on NFS and also in Git with other content has no baseline and is `unknown`). `in_sync` and `intentional_divergence` are not
//! drift. Only an adopted swimlane alerts: before an admin confirms the baseline every file is `unknown` and none of it is news.
//!
//! The kind of notification is [`NotificationKind::CriticalFinding`] when it names a C7 (a sync replaced changes that were only on
//! NFS), which is where `docs/domain-model.md` puts it, and [`NotificationKind::DriftDetected`] otherwise. The notifier routes by kind
//! and minimum severity from `notification_settings` (D49), so everything is offered and the admin chooses what reaches Teams.
//!
//! # What a notification carries
//!
//! Ids and counts: the swimlane id, how many files and the severity (S10). Never a path, a hash or any content. The link is the
//! swimlane's page.
//!
//! # What can go wrong
//!
//! A notifier that fails or hangs is logged, counted and forgotten; it never fails the pipeline's commit or a window's processing
//! (the port says so; every call is under `notify_timeout`). A live alert is at most once (a crash after the commit and before the send
//! loses it); the evaluation is at least once (a window whose processing failed after the send is processed again, and sends again).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::{DriftState, FindingKind, NfsPath, Severity, ShortText, SwimlaneId};
use engine::{ChangeContext, severity};
use ports::{Notification, NotificationKind, Notifier};
use sqlx::PgPool;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::hold::{Gate, Policy, Span, State};
use crate::clock::Clock;
use crate::config::{ConfigError, RegistryConfig};
use crate::db::{Db, ts};
use crate::error::RegistryError;
use crate::metrics::{AlertResult, RegistryMetrics};
use crate::severity::SeverityRuleSource;

/// The drift states an alert is raised for: NFS has something Git does not.
pub(crate) const ALERTABLE: [DriftState; 3] =
    [DriftState::NfsAhead, DriftState::Conflict, DriftState::Untracked];

/// The most C7 findings read for one evaluation: the engine refuses more files than this for a swimlane anyway.
const MAX_C7: i64 = 200_001;

/// Whether a file in this drift state is news (see the module documentation).
pub(crate) fn alertable(state: DriftState) -> bool {
    ALERTABLE.contains(&state)
}

/// The wire names of [`ALERTABLE`], for a statement.
pub(crate) fn alertable_names() -> Vec<&'static str> {
    ALERTABLE.iter().map(|s| s.as_str()).collect()
}

/// What an alert counts for one severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Files whose NFS-only change a sync replaced (C7).
    pub c7: usize,
    /// Other files whose NFS content Git does not have.
    pub drift: usize,
}

/// The notifications for the groups, most serious first, one for each severity. `after_sync` says the evaluation is a window's.
///
/// Ids and counts only (S10): nothing in the text comes from a file.
pub(crate) fn notifications(
    swimlane: &SwimlaneId,
    groups: &BTreeMap<Severity, Counts>,
    after_sync: bool,
) -> Vec<Notification> {
    groups
        .iter()
        .rev()
        .filter(|(_, c)| c.c7 + c.drift > 0)
        .filter_map(|(severity, c)| {
            let (kind, title, summary) = if c.c7 > 0 {
                let more = if c.drift > 0 {
                    format!(", and {} more file(s) differ on NFS", c.drift)
                } else {
                    String::new()
                };
                (
                    NotificationKind::CriticalFinding,
                    "A sync replaced changes that were only on NFS".to_owned(),
                    format!(
                        "{} file(s) of {swimlane} had changes that were only on NFS and a sync replaced them{more}. The replaced versions can be restored.",
                        c.c7
                    ),
                )
            } else if after_sync {
                (
                    NotificationKind::DriftDetected,
                    "NFS differs from Git after a sync".to_owned(),
                    format!(
                        "{} file(s) of {swimlane} differ on NFS and Git does not have the change. Alerts were held while the sync ran.",
                        c.drift
                    ),
                )
            } else {
                (
                    NotificationKind::DriftDetected,
                    "NFS differs from Git".to_owned(),
                    format!(
                        "{} file(s) of {swimlane} differ on NFS and Git does not have the change.",
                        c.drift
                    ),
                )
            };
            Some(Notification {
                kind,
                severity: *severity,
                swimlane: Some(swimlane.clone()),
                title: ShortText::parse(&title).ok()?,
                summary: ShortText::parse(&summary).ok()?,
                link: ShortText::parse(&format!("/swimlanes/{swimlane}")).ok(),
            })
        })
        .collect()
}

/// What an evaluation found and did, in counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Evaluation {
    /// Files whose NFS-only change a sync replaced.
    pub c7: usize,
    /// Other files that differ on NFS.
    pub drift: usize,
    /// Notifications handed to the notifier, and how many of them it took.
    pub notifications: usize,
    pub sent: usize,
}

/// What the alerts need.
pub struct AlertsDeps {
    /// The pool of the `lanekeeper_app` role.
    pub pool: PgPool,
    pub notifier: Arc<dyn Notifier>,
    pub clock: Arc<dyn Clock>,
    pub config: RegistryConfig,
    pub metrics: RegistryMetrics,
}

impl std::fmt::Debug for AlertsDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertsDeps").finish_non_exhaustive()
    }
}

struct Inner {
    db: Db,
    gate: Gate,
    notifier: Arc<dyn Notifier>,
    metrics: RegistryMetrics,
    notify_timeout: std::time::Duration,
    permits: Arc<Semaphore>,
    /// The rules a C7 is rated with. Everything else an alert says about severity is the severity of the findings the pipeline wrote.
    severity: SeverityRuleSource,
}

/// Drift alerts. Cloning shares the notifier, the gate and the bound on alerts in flight.
#[derive(Clone)]
pub struct Alerts {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Alerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alerts").finish_non_exhaustive()
    }
}

impl Alerts {
    /// # Errors
    ///
    /// The configuration is refused when a limit is out of range.
    pub fn new(deps: AlertsDeps) -> Result<Self, ConfigError> {
        deps.config.validate()?;
        let severity = SeverityRuleSource::new(deps.pool.clone(), &deps.config, deps.metrics.clone())?;
        let db = Db::new(deps.pool, deps.config.query_timeout);
        Ok(Self {
            inner: Arc::new(Inner {
                gate: Gate::new(db.clone(), deps.clock, Policy::of(&deps.config)),
                db,
                notifier: deps.notifier,
                metrics: deps.metrics,
                notify_timeout: deps.config.notify_timeout,
                permits: Arc::new(Semaphore::new(deps.config.alert_in_flight)),
                severity,
            }),
        })
    }

    /// The live path: findings of one swimlane that just became drift worth an alert, with their severities. Never fails and never
    /// waits for the notifier: a notification that goes out is sent from a task of its own, at most `alert_in_flight` at a time.
    pub async fn offer(&self, swimlane: &SwimlaneId, opened: &[Severity]) {
        if opened.is_empty() {
            return;
        }
        let mut groups: BTreeMap<Severity, Counts> = BTreeMap::new();
        for severity in opened {
            groups.entry(*severity).or_default().drift += 1;
        }
        let notifications = notifications(swimlane, &groups, false);
        let n = notifications.len() as u64;
        // A gate that cannot be read does not hold: an alert that is not wanted is better than one that was swallowed.
        let state = self.inner.gate.state(swimlane).await.unwrap_or(State {
            adopted: true,
            held: false,
        });
        if !state.adopted {
            self.inner.metrics.alert(AlertResult::NotAdopted, n);
            return;
        }
        if state.held {
            debug!(swimlane = %swimlane, alerts = n, "drift alerts held for the sync window");
            self.inner.metrics.alert(AlertResult::Held, n);
            return;
        }
        let Ok(permit) = Arc::clone(&self.inner.permits).try_acquire_owned() else {
            warn!(swimlane = %swimlane, alerts = n, "too many drift alerts in flight: dropped");
            self.inner.metrics.alert(AlertResult::Dropped, n);
            return;
        };
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            // Held until the sends are done, so that the bound is on alerts being sent and not only on alerts started.
            let _permit = permit;
            for notification in notifications {
                inner.send(notification).await;
            }
        });
    }

    /// A window's evaluation: what the swimlane looks like now, as one notification for each severity.
    ///
    /// `held` are the spans of the hub's time in which the windows held the swimlane's alerts ([`super::hold::held_spans`]): only what was
    /// written in them is evaluated, because the rest was told when it happened. `claimed` are the files whose NFS-only change the sync's
    /// own processing found it had replaced (C7, found before the pipeline has written the finding, and unioned with the C7 findings the
    /// pipeline wrote in the spans).
    ///
    /// # Errors
    ///
    /// A database failure, before anything was sent: the caller tries again.
    pub(crate) async fn evaluate(
        &self,
        swimlane: &SwimlaneId,
        held: &[Span],
        claimed: &BTreeSet<NfsPath>,
    ) -> Result<Evaluation, RegistryError> {
        let inner = &self.inner;
        // A file the sync replaced equals Git now: it is a C7, whatever the pipeline has recomputed so far, and not drift as well.
        let replaced: Vec<&str> = claimed.iter().map(NfsPath::as_str).collect();
        let drift = inner.drift_in(swimlane, held, &replaced).await?;
        let c7_found = inner.c7_in(swimlane, held).await?;
        let c7: BTreeSet<&str> = claimed
            .iter()
            .map(NfsPath::as_str)
            .chain(c7_found.iter().map(String::as_str))
            .collect();

        let c7_severity = severity(
            &ChangeContext {
                finding_kind: Some(FindingKind::OverwrittenNfsChange),
                ..ChangeContext::default()
            },
            &inner.severity.current_or_last().await.rules,
        )
        .severity;
        let mut groups: BTreeMap<Severity, Counts> = drift
            .iter()
            .map(|(s, n)| (*s, Counts { c7: 0, drift: *n }))
            .collect();
        groups.entry(c7_severity).or_default().c7 = c7.len();

        let notifications = notifications(swimlane, &groups, true);
        let mut evaluation = Evaluation {
            c7: c7.len(),
            drift: drift.values().sum(),
            notifications: notifications.len(),
            sent: 0,
        };
        inner.metrics.alert_evaluation();
        for notification in notifications {
            if inner.send(notification).await {
                evaluation.sent += 1;
            }
        }
        Ok(evaluation)
    }

    /// One notification that is not about drift (a stale window): the same bound on time and the same counting.
    pub(crate) async fn notify(&self, notification: Notification) -> bool {
        self.inner.send(notification).await
    }
}

impl Inner {
    /// Hand a notification to the notifier under the timeout. Whatever happens is counted and logged, and nothing fails.
    async fn send(&self, n: Notification) -> bool {
        let swimlane = n.swimlane.clone();
        match tokio::time::timeout(self.notify_timeout, self.notifier.notify(n)).await {
            Ok(Ok(())) => {
                self.metrics.alert(AlertResult::Sent, 1);
                true
            }
            Ok(Err(why)) => {
                warn!(swimlane = ?swimlane.as_ref().map(ToString::to_string), ?why, "a notification was not sent");
                self.metrics.alert(AlertResult::Failed, 1);
                false
            }
            Err(_) => {
                warn!(swimlane = ?swimlane.as_ref().map(ToString::to_string), "a notification timed out");
                self.metrics.alert(AlertResult::TimedOut, 1);
                false
            }
        }
    }

    /// Files that differ on NFS now and whose derived state was written in one of the `held` spans, by the severity of their drift
    /// finding, except the `replaced` ones.
    async fn drift_in(
        &self,
        swimlane: &SwimlaneId,
        held: &[Span],
        replaced: &[&str],
    ) -> Result<BTreeMap<Severity, usize>, RegistryError> {
        let sql = format!(
            "SELECT f.severity, count(*)::bigint FROM file_state s \
             JOIN findings f ON f.swimlane_id = s.swimlane_id AND f.kind = 'drift' AND f.path = s.path AND f.resolved_at IS NULL \
             WHERE s.swimlane_id = $1 AND s.state = ANY($2::text[]) AND s.path <> ALL($3::text[]) \
               AND EXISTS (SELECT 1 FROM unnest($4::bigint[], $5::bigint[]) AS w(a, b) \
                           WHERE s.updated_at >= {from} AND s.updated_at <= {to} LIMIT 1) \
             GROUP BY f.severity",
            from = ts("w.a"),
            to = ts("w.b"),
        );
        let (froms, tos) = bounds(held);
        let rows: Vec<(String, i64)> = self
            .db
            .run(
                "alert_drift",
                sqlx::query_as(&sql)
                    .bind(swimlane.as_str())
                    .bind(alertable_names())
                    .bind(replaced)
                    .bind(&froms)
                    .bind(&tos)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        let mut out = BTreeMap::new();
        for (severity, n) in rows {
            let severity: Severity = severity.parse().map_err(|_| RegistryError::Corrupt)?;
            *out.entry(severity).or_insert(0) += usize::try_from(n).unwrap_or(usize::MAX);
        }
        Ok(out)
    }

    /// The paths of the open C7 findings written in one of the `held` spans.
    async fn c7_in(&self, swimlane: &SwimlaneId, held: &[Span]) -> Result<Vec<String>, RegistryError> {
        let sql = format!(
            "SELECT path FROM findings WHERE swimlane_id = $1 AND kind = 'overwritten_nfs_change' AND resolved_at IS NULL \
               AND path IS NOT NULL \
               AND EXISTS (SELECT 1 FROM unnest($2::bigint[], $3::bigint[]) AS w(a, b) \
                           WHERE detected_at >= {from} AND detected_at <= {to} LIMIT 1) \
             ORDER BY path LIMIT $4",
            from = ts("w.a"),
            to = ts("w.b"),
        );
        let (froms, tos) = bounds(held);
        let rows: Vec<String> = self
            .db
            .run(
                "alert_c7",
                sqlx::query_scalar(&sql)
                    .bind(swimlane.as_str())
                    .bind(&froms)
                    .bind(&tos)
                    .bind(MAX_C7)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        if rows.len() >= usize::try_from(MAX_C7).unwrap_or(usize::MAX) {
            return Err(RegistryError::Capacity);
        }
        Ok(rows)
    }
}

/// The two arrays of a statement: where each span starts and where it ends.
fn bounds(spans: &[Span]) -> (Vec<i64>, Vec<i64>) {
    (
        spans.iter().map(|s| s.from_ms).collect(),
        spans.iter().map(|s| s.to_ms).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> SwimlaneId {
        SwimlaneId::parse("sit1").unwrap()
    }

    fn groups(items: &[(Severity, usize, usize)]) -> BTreeMap<Severity, Counts> {
        items
            .iter()
            .map(|(s, c7, drift)| {
                (
                    *s,
                    Counts {
                        c7: *c7,
                        drift: *drift,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn nfs_side_drift_is_news_and_a_change_waiting_for_its_sync_is_not() {
        for state in DriftState::ALL {
            let expected = matches!(
                state,
                DriftState::NfsAhead | DriftState::Conflict | DriftState::Untracked
            );
            assert_eq!(alertable(state), expected, "{state}");
        }
        assert_eq!(alertable_names(), vec!["nfs_ahead", "conflict", "untracked"]);
    }

    #[test]
    fn one_notification_for_each_severity_most_serious_first() {
        let n = notifications(
            &lane(),
            &groups(&[
                (Severity::Low, 0, 3),
                (Severity::Critical, 0, 1),
                (Severity::Medium, 0, 400),
                (Severity::High, 0, 0),
            ]),
            true,
        );
        assert_eq!(
            n.iter().map(|n| n.severity).collect::<Vec<_>>(),
            vec![Severity::Critical, Severity::Medium, Severity::Low],
            "a severity with nothing in it says nothing"
        );
        assert!(n.iter().all(|n| n.kind == NotificationKind::DriftDetected));
        assert!(
            n[1].summary.as_str().contains("400 file(s)"),
            "{:?}",
            n[1].summary
        );
        assert!(n.iter().all(|n| n.swimlane.as_ref() == Some(&lane())));
        assert!(
            n.iter()
                .all(|n| n.link.as_ref().map(ShortText::as_str) == Some("/swimlanes/sit1"))
        );
    }

    #[test]
    fn a_group_with_a_c7_is_a_critical_finding_and_still_counts_the_rest() {
        let n = notifications(&lane(), &groups(&[(Severity::Critical, 2, 5)]), true);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, NotificationKind::CriticalFinding);
        assert!(n[0].summary.as_str().contains("2 file(s)"), "{:?}", n[0].summary);
        assert!(n[0].summary.as_str().contains("5 more"), "{:?}", n[0].summary);
        let only = notifications(&lane(), &groups(&[(Severity::High, 3, 0)]), true);
        assert_eq!(only[0].kind, NotificationKind::CriticalFinding);
        assert_eq!(
            only[0].severity,
            Severity::High,
            "the kind says what it is, the severity how bad"
        );
        assert!(!only[0].summary.as_str().contains("more"));
    }

    #[test]
    fn the_live_text_and_the_window_text_differ_only_in_saying_what_was_held() {
        let g = groups(&[(Severity::Medium, 0, 2)]);
        let live = notifications(&lane(), &g, false);
        let window = notifications(&lane(), &g, true);
        assert_eq!(live[0].kind, window[0].kind);
        assert!(!live[0].summary.as_str().contains("held"));
        assert!(window[0].summary.as_str().contains("held"));
    }

    #[test]
    fn a_notification_names_ids_and_counts_and_no_file() {
        let n = notifications(&lane(), &groups(&[(Severity::Medium, 1, 7)]), true);
        for n in &n {
            for text in [n.title.as_str(), n.summary.as_str()] {
                assert!(text.len() <= 256);
                assert!(!text.contains(".yml") && !text.contains("sha256"), "{text}");
            }
        }
    }

    #[test]
    fn nothing_to_say_is_no_notification() {
        assert!(notifications(&lane(), &BTreeMap::new(), true).is_empty());
        assert!(notifications(&lane(), &groups(&[(Severity::Low, 0, 0)]), false).is_empty());
    }
}
