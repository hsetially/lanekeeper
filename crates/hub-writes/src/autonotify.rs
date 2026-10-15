//! Auto-notify (T11, D85): when an admin has turned it on for a swimlane, the config-server is told about the files an Operator
//! has written, once, a couple of seconds after the writes, with all the paths of those writes in the one call.
//!
//! - **Off by default** (`swimlanes.auto_notify`). The switch is read when a write commits, so a write made before the switch
//!   was turned on is not announced afterwards.
//! - **Operator or above** (plan A13, Q37). The config-server's update endpoint has no authentication, so a call to it is a
//!   call by a person who may make it. The batch is kept for the user whose write it holds, the role is read from the directory
//!   when the call is made (a user demoted in the meantime is not announced), and a user who needs approval is never announced:
//!   their change is a proposal until an admin applies it, and then the admin is the user.
//! - **Batched.** The first write opens a window ([`crate::WriteConfig::auto_notify_window`], 2 seconds); every write that
//!   commits within it adds its path to the batch of its swimlane and user; when the window closes each batch is one
//!   [`crate::WriteHub::notify`]-shaped call (at most 100 paths each), audited once with the user whose writes it holds and
//!   `via = system`.
//! - **Bounded.** The queue between the write path and the batcher has a size ([`crate::WriteConfig::auto_notify_queue`]); a write
//!   that finds it full is not announced (it is logged and counted, and the "Notify services" button is still there). The
//!   batcher holds at most that many paths at a time and closes the window early when it has them.
//! - **Best effort.** A call that fails is logged and not retried: the person can press the button. It never reaches the write
//!   that caused it, which has been answered by then.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use domain::{IdempotencyKey, NfsPath, NotifyRequest, RequestId, Role, SwimlaneId, UserId, Via, WriteCtx};
use futures::StreamExt;
use ports::{AuditAction, AuditActor, AuditEvent, AuditVia};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::pipeline::authorize::authorize_user;
use crate::service::Inner;

/// The most batches one window flushes at the same time.
const FLUSH_PARALLELISM: usize = 8;

/// One written path, as the batcher is told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoItem {
    pub swimlane: SwimlaneId,
    pub user: UserId,
    pub path: NfsPath,
}

impl AutoItem {
    /// The path a recorded action wrote, if the action is a write of one file by a person (an edit, an upload, a delete, a revert,
    /// an applied draft). A restart, a notification, a proposal and everything else has no path to announce.
    pub fn of(event: &AuditEvent) -> Option<Self> {
        if !matches!(
            event.action,
            AuditAction::FileEdited
                | AuditAction::FileUploaded
                | AuditAction::FileDeleted
                | AuditAction::FileReverted
                | AuditAction::DraftApplied
        ) {
            return None;
        }
        let AuditActor::User { user } = &event.actor else {
            return None;
        };
        Some(Self {
            swimlane: event.swimlane.clone()?,
            user: user.clone(),
            path: event.path.clone()?,
        })
    }
}

/// The queue from the write path to the batcher.
pub(crate) struct AutoQueue {
    sender: mpsc::Sender<AutoItem>,
    receiver: Mutex<Option<mpsc::Receiver<AutoItem>>>,
    /// Set when the batcher has been handed out: without one, nothing is queued and nothing is read.
    enabled: AtomicBool,
    /// Paths that were not announced because the queue was full.
    dropped: AtomicU64,
}

pub(crate) const SWITCH: &str = "SELECT auto_notify FROM swimlanes WHERE id = $1";

impl AutoQueue {
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        Self {
            sender,
            receiver: Mutex::new(Some(receiver)),
            enabled: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    }

    /// The batcher's end of the queue, once. Turns the queue on.
    pub fn take_receiver(&self) -> Option<mpsc::Receiver<AutoItem>> {
        let taken = self
            .receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if taken.is_some() {
            self.enabled.store(true, Ordering::SeqCst);
        }
        taken
    }

    /// How many paths were not announced because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Queue a path a write has committed, if the swimlane has auto-notify on. Never fails the write: the write has been
    /// recorded and answered as far as the caller is concerned.
    pub async fn offer(&self, hub: &Inner, item: AutoItem) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        let on = hub
            .within(
                "auto_notify_switch",
                sqlx::query_scalar::<_, bool>(SWITCH)
                    .bind(item.swimlane.as_str())
                    .fetch_optional(&hub.pool),
            )
            .await;
        match on {
            Ok(Ok(Some(true))) => {}
            Ok(Ok(_)) => return,
            // Without the switch the write is not announced; the button is still there.
            Ok(Err(_)) | Err(_) => {
                tracing::warn!("the auto-notify switch could not be read");
                return;
            }
        }
        if self.sender.try_send(item).is_err() {
            let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                dropped = total,
                "auto-notify is behind: a written path was not announced"
            );
        }
    }
}

/// Paths waiting for the window to close, by swimlane and user. Pure: it reads no clock, so its rules are tested without time.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    groups: BTreeMap<(SwimlaneId, UserId), BTreeSet<NfsPath>>,
    total: usize,
}

impl Batch {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Add a path. A path written twice in a window is announced once.
    pub fn add(&mut self, item: AutoItem) {
        let set = self.groups.entry((item.swimlane, item.user)).or_default();
        if set.insert(item.path) {
            self.total += 1;
        }
    }

    /// True when the batch holds `limit` paths: the window closes early.
    pub fn is_full(&self, limit: usize) -> bool {
        self.total >= limit
    }

    /// Take everything: for each swimlane and user, the paths in order, in chunks of the most one notification may name.
    pub fn take(&mut self) -> Vec<(SwimlaneId, UserId, Vec<NfsPath>)> {
        self.total = 0;
        std::mem::take(&mut self.groups)
            .into_iter()
            .flat_map(|((swimlane, user), paths)| {
                let paths: Vec<NfsPath> = paths.into_iter().collect();
                paths
                    .chunks(NotifyRequest::MAX_PATHS)
                    .map(|chunk| (swimlane.clone(), user.clone(), chunk.to_vec()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// The batcher: take it from [`crate::WriteHub::auto_notifier`] and run it as a task of the process. One per hub; a replica that
/// handled a write announces it, so replicas never announce the same write twice.
pub struct AutoNotifier {
    hub: Arc<Inner>,
    queue: mpsc::Receiver<AutoItem>,
}

impl std::fmt::Debug for AutoNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoNotifier").finish_non_exhaustive()
    }
}

impl AutoNotifier {
    pub(crate) fn new(hub: Arc<Inner>, queue: mpsc::Receiver<AutoItem>) -> Self {
        Self { hub, queue }
    }

    /// Batch and send until `stop` completes. Paths still waiting then are not announced.
    pub async fn run(mut self, stop: impl Future<Output = ()>) {
        tokio::pin!(stop);
        let window = self.hub.cfg.auto_notify_window;
        let limit = self.hub.cfg.auto_notify_queue;
        let mut batch = Batch::default();
        loop {
            let first = tokio::select! {
                () = &mut stop => return,
                item = self.queue.recv() => item,
            };
            let Some(first) = first else { return };
            batch.add(first);
            let closes = Instant::now() + window;
            while !batch.is_full(limit) {
                tokio::select! {
                    () = &mut stop => return,
                    () = tokio::time::sleep_until(closes) => break,
                    item = self.queue.recv() => match item {
                        Some(item) => batch.add(item),
                        None => break,
                    },
                }
            }
            // A call that has begun is finished even if the stop comes meanwhile: dropping it halfway would leave a notification that
            // went out unrecorded. It is bounded by the agent and database timeouts.
            let hub = &self.hub;
            futures::stream::iter(batch.take())
                .for_each_concurrent(FLUSH_PARALLELISM, |(swimlane, user, paths)| async move {
                    announce(hub, swimlane, user, paths).await;
                })
                .await;
        }
    }
}

/// Make one notification for the writes of a user, if the user may still ask for it.
async fn announce(hub: &Inner, swimlane: SwimlaneId, user: UserId, paths: Vec<NfsPath>) {
    // The role is the directory's now, not the one the user had when they wrote. A user who needs approval does not announce
    // (what they write waits for an admin).
    let Ok(user) = authorize_user(hub, &user, Role::Operator).await else {
        tracing::debug!("a written path is not announced: its writer is no longer an operator");
        return;
    };
    if user.requires_approval {
        return;
    }
    let (Some(key), Some(request_id)) = (fresh("an"), fresh("an")) else {
        tracing::warn!("auto-notify could not make an id");
        return;
    };
    let (Ok(idempotency_key), Ok(request_id)) = (IdempotencyKey::parse(&key), RequestId::parse(&request_id))
    else {
        return;
    };
    let ctx = WriteCtx {
        user,
        via: Via::Ui,
        idempotency_key,
        request_id,
    };
    let count = paths.len();
    let result =
        crate::ops::notify::notify_as(hub, &ctx, NotifyRequest { swimlane, paths }, AuditVia::System).await;
    match result {
        Ok(domain::WriteOutcome::Applied { .. }) => {
            tracing::info!(
                paths = count,
                "the config-server was told about a batch of writes"
            );
        }
        Ok(_) | Err(_) => {
            tracing::warn!(
                paths = count,
                "the config-server could not be told about a batch of writes"
            );
        }
    }
}

/// A random id with a prefix.
fn fresh(prefix: &str) -> Option<String> {
    let mut bytes = [0_u8; 8];
    aws_lc_rs::rand::fill(&mut bytes).ok()?;
    Some(format!("{prefix}-{}", crate::hex::encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Timestamp, UserId};

    fn item(swimlane: &str, user: u32, path: &str) -> AutoItem {
        AutoItem {
            swimlane: SwimlaneId::parse(swimlane).unwrap(),
            user: ports::conformance::sample::user_id(user),
            path: NfsPath::parse(path).unwrap(),
        }
    }

    #[test]
    fn a_batch_groups_by_swimlane_and_user_and_announces_a_path_once() {
        let mut b = Batch::default();
        assert!(b.is_empty());
        b.add(item("sit1", 1, "b.yml"));
        b.add(item("sit1", 1, "a.yml"));
        b.add(item("sit1", 1, "a.yml"));
        b.add(item("sit1", 2, "c.yml"));
        b.add(item("sit2", 1, "a.yml"));
        assert!(!b.is_empty());
        let taken = b.take();
        assert!(b.is_empty(), "taking empties it");
        let shape: Vec<(String, Vec<&str>)> = taken
            .iter()
            .map(|(s, _, p)| (s.to_string(), p.iter().map(NfsPath::as_str).collect()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("sit1".to_owned(), vec!["a.yml", "b.yml"]),
                ("sit1".to_owned(), vec!["c.yml"]),
                ("sit2".to_owned(), vec!["a.yml"]),
            ]
        );
        assert_eq!(taken[0].1, ports::conformance::sample::user_id(1));
        assert_eq!(taken[1].1, ports::conformance::sample::user_id(2));
    }

    #[test]
    fn a_notification_names_at_most_a_hundred_paths_and_none_is_lost() {
        let mut b = Batch::default();
        for n in 0..250 {
            b.add(item("sit1", 1, &format!("d/f{n:03}.yml")));
        }
        let taken = b.take();
        let sizes: Vec<usize> = taken.iter().map(|(_, _, p)| p.len()).collect();
        assert_eq!(sizes, [100, 100, 50]);
        let all: BTreeSet<&str> = taken
            .iter()
            .flat_map(|(_, _, p)| p.iter().map(NfsPath::as_str))
            .collect();
        assert_eq!(all.len(), 250);
    }

    #[test]
    fn a_batch_is_full_at_its_limit_and_counts_distinct_paths() {
        let mut b = Batch::default();
        b.add(item("sit1", 1, "a.yml"));
        b.add(item("sit1", 1, "a.yml"));
        assert!(!b.is_full(2), "the same path twice is one path");
        b.add(item("sit1", 1, "b.yml"));
        assert!(b.is_full(2));
        let _ = b.take();
        assert!(!b.is_full(2));
    }

    #[test]
    fn only_a_persons_write_of_one_file_is_announced() {
        let user: UserId = ports::conformance::sample::user_id(1);
        let at = Timestamp::from_unix_millis(0);
        let event = |action: AuditAction, actor: AuditActor, path: Option<&str>| {
            let mut e = AuditEvent::new(at, actor, action, AuditVia::Ui);
            e.swimlane = Some(SwimlaneId::parse("sit1").unwrap());
            e.path = path.map(|p| NfsPath::parse(p).unwrap());
            e
        };
        let person = || AuditActor::User { user: user.clone() };
        for action in [
            AuditAction::FileEdited,
            AuditAction::FileUploaded,
            AuditAction::FileDeleted,
            AuditAction::FileReverted,
            AuditAction::DraftApplied,
        ] {
            let got = AutoItem::of(&event(action, person(), Some("a.yml"))).expect("announced");
            assert_eq!((got.path.as_str(), &got.user), ("a.yml", &user), "{action:?}");
            assert!(
                AutoItem::of(&event(action, person(), None)).is_none(),
                "{action:?} without a path"
            );
        }
        for action in [
            AuditAction::ServiceRestarted,
            AuditAction::ConfigServerNotified,
            AuditAction::ProposalCreated,
            AuditAction::ProposalApproved,
            AuditAction::FlaggedValueRevealed,
            AuditAction::PrRaised,
            AuditAction::FileChangeDetected,
        ] {
            assert!(
                AutoItem::of(&event(action, person(), Some("a.yml"))).is_none(),
                "{action:?}"
            );
        }
        // And a change nobody made through the tool is not announced either.
        assert!(
            AutoItem::of(&event(
                AuditAction::FileEdited,
                AuditActor::Agent {
                    swimlane: SwimlaneId::parse("sit1").unwrap()
                },
                Some("a.yml")
            ))
            .is_none()
        );
    }
}
