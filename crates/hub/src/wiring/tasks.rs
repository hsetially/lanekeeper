//! The tasks the process spawns, owned in one place (plan 18, T3; rule 5).
//!
//! Every task the composition starts is adopted here under a fixed name. Three things follow. A task that is meant to run for the
//! life of the process and ends while the hub is still serving is logged by name (an error that ended it, or a panic, would
//! otherwise be a silent hole), and `/readyz` fails until a person looks (`TasksAlive`): a replica that has lost its event relay or
//! its certificate renewal is not a replica to send work to. At shutdown, every task is waited for under a limit and aborted when
//! it overruns, so the process never hangs on one and never leaves one behind. And a test can see which tasks a replica runs.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{error, warn};

use crate::ops::ReadyCheck;

/// The tasks of one running hub.
pub struct Tasks {
    closing: watch::Receiver<bool>,
    /// A watcher per task: it ends when the task does, so waiting for the watchers is waiting for the tasks.
    watchers: Vec<(&'static str, JoinHandle<()>)>,
    /// The tasks whose end, while serving, is a failure. Shared with [`TasksAlive`], so that a task adopted after the check was
    /// made (the identity module's, the web certificate's reload) is covered by it too.
    required: Arc<Mutex<Vec<(&'static str, AbortHandle)>>>,
    /// Every task, so that the ones that overrun the shutdown can be cut off.
    all: Vec<AbortHandle>,
}

impl std::fmt::Debug for Tasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tasks")
            .field("tasks", &self.watchers.len())
            .field("required", &self.required_names().len())
            .finish_non_exhaustive()
    }
}

impl Tasks {
    /// `closing` turns true when the hub starts to stop: a task that ends after that is doing what it was asked.
    #[must_use]
    pub fn new(closing: watch::Receiver<bool>) -> Self {
        Self {
            closing,
            watchers: Vec::new(),
            required: Arc::new(Mutex::new(Vec::new())),
            all: Vec::new(),
        }
    }

    fn required_names(&self) -> Vec<&'static str> {
        self.required
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(name, _)| *name)
            .collect()
    }

    /// A task that runs until the hub stops. Its ending before then is logged and turns readiness off.
    pub fn adopt(&mut self, name: &'static str, task: JoinHandle<()>) {
        self.required
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name, task.abort_handle()));
        self.watch(name, task, true);
    }

    /// A task that may end on its own (it fills a mirror and is done). Its ending is not a failure, but a panic is still logged.
    pub fn adopt_finite(&mut self, name: &'static str, task: JoinHandle<()>) {
        self.watch(name, task, false);
    }

    fn watch(&mut self, name: &'static str, task: JoinHandle<()>, long_lived: bool) {
        self.all.push(task.abort_handle());
        let closing = self.closing.clone();
        let watcher = tokio::spawn(async move {
            let ended = task.await;
            let stopping = *closing.borrow();
            match ended {
                Err(failure) if failure.is_panic() => {
                    error!(task = name, "a task panicked");
                }
                Err(_) => {
                    // Cancelled: by the shutdown's abort, or by a test.
                    if !stopping {
                        warn!(task = name, "a task was cancelled");
                    }
                }
                Ok(()) if long_lived && !stopping => {
                    error!(
                        task = name,
                        "a task that should run for the life of the process has ended"
                    );
                }
                Ok(()) => {}
            }
        });
        self.watchers.push((name, watcher));
    }

    /// The readiness check over the tasks adopted so far and the ones adopted after: it shares the list.
    #[must_use]
    pub fn alive(&self) -> TasksAlive {
        TasksAlive(Arc::clone(&self.required))
    }

    /// The names of the tasks, in the order they were adopted.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.watchers.iter().map(|(name, _)| *name).collect()
    }

    /// Cut off every task now, as a replica that crashed is cut off. For tests that kill a replica, and for the second signal.
    pub fn abort_all(&self) {
        for task in &self.all {
            task.abort();
        }
    }

    /// Wait for every task to end, up to `within` in all, and abort the ones that have not. Returns the names of those.
    pub async fn finish(self, within: Duration) -> Vec<&'static str> {
        let deadline = tokio::time::Instant::now() + within;
        let mut overran = Vec::new();
        for ((name, mut watcher), task) in self.watchers.into_iter().zip(self.all) {
            if tokio::time::timeout_at(deadline, &mut watcher).await.is_err() {
                warn!(task = name, "a task did not stop in time and was cut off");
                task.abort();
                overran.push(name);
            }
        }
        overran
    }
}

/// Ready while no task that should run for the life of the process has ended.
#[derive(Clone)]
pub struct TasksAlive(Arc<Mutex<Vec<(&'static str, AbortHandle)>>>);

impl std::fmt::Debug for TasksAlive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TasksAlive")
            .field(
                "tasks",
                &self.0.lock().unwrap_or_else(PoisonError::into_inner).len(),
            )
            .finish_non_exhaustive()
    }
}

impl TasksAlive {
    /// The names of the required tasks that have ended.
    #[must_use]
    pub fn ended(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, task)| task.is_finished())
            .map(|(name, _)| *name)
            .collect()
    }
}

#[async_trait]
impl ReadyCheck for TasksAlive {
    async fn ready(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .all(|(_, task)| !task.is_finished())
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use super::*;

    fn tasks() -> (watch::Sender<bool>, Tasks) {
        let (closing, rx) = watch::channel(false);
        (closing, Tasks::new(rx))
    }

    #[tokio::test(start_paused = true)]
    async fn a_required_task_that_ends_turns_readiness_off_and_names_itself() {
        let (_closing, mut tasks) = tasks();
        tasks.adopt("runs-forever", tokio::spawn(pending::<()>()));
        let (end, ended) = tokio::sync::oneshot::channel::<()>();
        tasks.adopt(
            "ends-early",
            tokio::spawn(async move {
                let _ = ended.await;
            }),
        );
        let alive = tasks.alive();
        assert!(alive.ready().await);
        assert!(alive.ended().is_empty());
        end.send(()).unwrap();
        tokio::task::yield_now().await;
        for _ in 0..50 {
            if !alive.ready().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!alive.ready().await);
        assert_eq!(alive.ended(), vec!["ends-early"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_adopted_after_the_check_was_made_is_covered_by_it() {
        let (_closing, mut tasks) = tasks();
        tasks.adopt("first", tokio::spawn(pending::<()>()));
        let alive = tasks.alive();
        // The identity module and the web certificate's reload are adopted once the platform's check exists.
        let (end, ended) = tokio::sync::oneshot::channel::<()>();
        tasks.adopt(
            "adopted-late",
            tokio::spawn(async move {
                let _ = ended.await;
            }),
        );
        assert!(alive.ready().await);
        end.send(()).unwrap();
        for _ in 0..50 {
            if !alive.ready().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!alive.ready().await);
        assert_eq!(alive.ended(), vec!["adopted-late"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_task_that_panics_is_a_task_that_ended() {
        let (_closing, mut tasks) = tasks();
        tasks.adopt(
            "panics",
            tokio::spawn(async {
                panic!("a planted panic");
            }),
        );
        let alive = tasks.alive();
        for _ in 0..100 {
            if !alive.ready().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(alive.ended(), vec!["panics"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_finite_task_may_end_without_failing_readiness() {
        let (_closing, mut tasks) = tasks();
        tasks.adopt_finite("fills-a-mirror", tokio::spawn(async {}));
        tasks.adopt("runs-forever", tokio::spawn(pending::<()>()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(tasks.alive().ready().await, "only required tasks count");
        assert_eq!(tasks.names(), vec!["fills-a-mirror", "runs-forever"]);
    }

    #[tokio::test(start_paused = true)]
    async fn finish_waits_for_the_tasks_that_stop_and_cuts_off_the_ones_that_do_not() {
        let (closing, mut tasks) = tasks();
        let mut stop = closing.subscribe();
        tasks.adopt(
            "stops-when-told",
            tokio::spawn(async move {
                let _ = stop.wait_for(|c| *c).await;
            }),
        );
        tasks.adopt("never-stops", tokio::spawn(pending::<()>()));
        closing.send_replace(true);
        let overran = tasks.finish(Duration::from_millis(200)).await;
        assert_eq!(overran, vec!["never-stops"]);
    }

    #[tokio::test(start_paused = true)]
    async fn abort_all_is_what_a_crashed_replica_looks_like() {
        let (_closing, mut tasks) = tasks();
        tasks.adopt("a", tokio::spawn(pending::<()>()));
        tasks.adopt("b", tokio::spawn(pending::<()>()));
        let alive = tasks.alive();
        assert!(alive.ready().await);
        tasks.abort_all();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!alive.ready().await);
        assert_eq!(alive.ended(), vec!["a", "b"]);
    }
}
