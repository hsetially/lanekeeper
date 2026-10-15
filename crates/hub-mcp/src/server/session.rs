//! Sessions (rule 5, S14): how many an MCP user and this replica may hold, and what happens to the count when one ends.
//!
//! rmcp keeps a session, and the channel an elicitation answer returns on, in one replica's memory. Without a limit a caller could
//! open sessions until the replica ran out of memory, so there are two: 5 per user and 200 per replica (configurable). The gate
//! counts a session from the moment it is asked for (a reservation, so that two requests at the limit cannot both pass), and the
//! count falls when the session ends: `DELETE`, the idle expiry of rmcp (30 minutes) or the end of the owner's record.
//! [`CappedSessionManager`] is the second line: rmcp's own manager behind a semaphore, in case a session is made by a path that did
//! not pass the gate.
//!
//! **A slot is held by a value, not by a code path.** The gate takes an [`Opening`] when a request may open a session. The slot
//! goes back when that value is dropped, so every way out gives it back: an early return, a panic, and the client that goes away
//! before the response head (hyper then drops the gate's future, and no line after the `await` runs). Exactly one thing disarms it,
//! [`Opening::hand_over`], when the response carries the session's id and the owner table takes the slot over. A request that made
//! an rmcp session and then did not hand it over (the client left, or the handshake failed) also closes that session: the client
//! never learned its id, so it could not end it, and it would keep a place in rmcp's table until it idled out.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::UserId;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use rmcp::transport::streamable_http_server::{SessionId, SessionManager};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// How many sessions are open or being opened.
#[derive(Debug)]
pub struct SessionCounts {
    per_user_cap: usize,
    replica_cap: usize,
    total: AtomicUsize,
    users: Mutex<HashMap<UserId, usize>>,
}

/// Why a session was not allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLimit {
    /// The user already holds the most sessions allowed.
    User,
    /// The replica is full.
    Replica,
}

impl SessionCounts {
    #[must_use]
    pub fn new(per_user_cap: u32, replica_cap: u32) -> Self {
        Self {
            per_user_cap: per_user_cap as usize,
            replica_cap: replica_cap as usize,
            total: AtomicUsize::new(0),
            users: Mutex::new(HashMap::new()),
        }
    }

    /// Count one more session for `user` and return the value that holds the slot, or say why not. The slot goes back when the
    /// value is dropped, unless [`Opening::hand_over`] was called; `close` ends a session the request made and did not hand over.
    ///
    /// # Errors
    ///
    /// [`SessionLimit`] when a limit is reached. Nothing is counted then.
    pub fn open(self: &Arc<Self>, user: &UserId, close: Closer) -> Result<Opening, SessionLimit> {
        self.reserve(user)?;
        Ok(Opening {
            counts: Arc::clone(self),
            user: user.clone(),
            opened: Arc::new(Opened::default()),
            close,
            handed_over: false,
        })
    }

    fn reserve(&self, user: &UserId) -> Result<(), SessionLimit> {
        let mut users = self.users.lock().unwrap_or_else(PoisonError::into_inner);
        if self.total.load(Ordering::SeqCst) >= self.replica_cap {
            return Err(SessionLimit::Replica);
        }
        let held = users.entry(user.clone()).or_insert(0);
        if *held >= self.per_user_cap {
            return Err(SessionLimit::User);
        }
        *held += 1;
        self.total.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// One session of `user` is gone (or was never made). Only an [`Opening`] and the owner table call this.
    pub(crate) fn release(&self, user: &UserId) {
        let mut users = self.users.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = users.get_mut(user) {
            *held -= 1;
            if *held == 0 {
                users.remove(user);
            }
            self.total.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Sessions counted now.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }

    /// Sessions counted for `user`.
    #[must_use]
    pub fn of(&self, user: &UserId) -> usize {
        self.users
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(user)
            .copied()
            .unwrap_or(0)
    }
}

/// The longest a session that nobody will use is given to close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Ends a session that no one will use. It runs from `Drop`, so it does not wait: it starts the work and returns.
pub type Closer = Arc<dyn Fn(SessionId) + Send + Sync>;

/// A [`Closer`] that closes the session on `sessions`, on the runtime the caller is on (within [`CLOSE_TIMEOUT`]). Off a runtime
/// there is nothing to close it with, and the session ends at its idle expiry, still behind the manager's own ceiling.
pub fn closer<M>(sessions: Arc<CappedSessionManager<M>>) -> Closer
where
    M: SessionManager + 'static,
{
    Arc::new(move |id: SessionId| {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let sessions = Arc::clone(&sessions);
        runtime.spawn(async move {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, sessions.close_session(&id)).await;
        });
    })
}

tokio::task_local! {
    /// Where [`CappedSessionManager::create_session`] writes the id of the session the current request opens. rmcp makes the
    /// session inside the request's own task, so the task is the one place that ties a session to the request that made it.
    static OPENED: Arc<Opened>;
}

/// The id of the session a request opened, once rmcp has made it.
#[derive(Debug, Default)]
struct Opened(Mutex<Option<SessionId>>);

impl Opened {
    fn set(&self, id: SessionId) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(id);
    }

    fn take(&self) -> Option<SessionId> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// One slot of the session count, held by a request that may open a session. See the module documentation.
#[must_use = "an Opening that is dropped gives its slot back"]
pub struct Opening {
    counts: Arc<SessionCounts>,
    user: UserId,
    opened: Arc<Opened>,
    close: Closer,
    handed_over: bool,
}

impl fmt::Debug for Opening {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opening")
            .field("handed_over", &self.handed_over)
            .finish_non_exhaustive()
    }
}

impl Opening {
    /// Run `request`, which may open a session, as the request of this slot: the session manager records the session it makes
    /// against it. The slot is not given back while `request` runs, and it is when `request` is dropped before it finishes.
    pub async fn run<F: Future>(&self, request: F) -> F::Output {
        OPENED.scope(Arc::clone(&self.opened), request).await
    }

    /// The response carries the session's id and the owner table remembers it: from here the table's entry stands for the slot, and
    /// releases it when the entry goes. Nothing awaits between the response and this call, so the future cannot be dropped in
    /// between.
    pub fn hand_over(mut self) {
        self.handed_over = true;
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        if self.handed_over {
            return;
        }
        self.counts.release(&self.user);
        // The slot is free first, then the session goes: for the moment it takes the close to run, the count is one short of rmcp's
        // table, which the manager's own ceiling covers.
        if let Some(id) = self.opened.take() {
            (self.close)(id);
        }
    }
}

/// What to do when a session closes: forget who owned it.
pub type OnClose = Arc<dyn Fn(&str) + Send + Sync>;

/// rmcp's session manager with a ceiling on live sessions, and a hook for the end of each.
pub struct CappedSessionManager<M> {
    inner: M,
    permits: Arc<Semaphore>,
    held: Mutex<HashMap<SessionId, OwnedSemaphorePermit>>,
    on_close: OnClose,
}

impl<M> CappedSessionManager<M> {
    pub fn new(inner: M, cap: usize, on_close: OnClose) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(cap)),
            held: Mutex::new(HashMap::new()),
            on_close,
        }
    }

    /// How many sessions are live right now.
    pub fn live(&self) -> usize {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// The wrapped manager.
    pub fn inner(&self) -> &M {
        &self.inner
    }
}

impl<M> fmt::Debug for CappedSessionManager<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CappedSessionManager")
            .field("live", &self.live())
            .finish_non_exhaustive()
    }
}

/// The ceiling was reached, or the wrapped manager failed.
#[derive(Debug)]
pub enum CappedError<E> {
    Full,
    Inner(E),
}

impl<E: fmt::Display> fmt::Display for CappedError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("too many live sessions"),
            Self::Inner(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CappedError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Full => None,
            Self::Inner(error) => Some(error),
        }
    }
}

impl<M: SessionManager> SessionManager for CappedSessionManager<M> {
    type Error = CappedError<M::Error>;
    type Transport = M::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        // The permit is taken before the session exists, so two requests cannot both pass a check-then-create race.
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| CappedError::Full)?;
        let (id, transport) = self.inner.create_session().await.map_err(CappedError::Inner)?;
        // No await from here to the end: a request that is dropped has either not made the session or has recorded it. A session
        // made on a path that did not come through the gate has no request to tell.
        let _ = OPENED.try_with(|opened| opened.set(id.clone()));
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), permit);
        Ok((id, transport))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        self.inner
            .initialize_session(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        self.inner.has_session(id).await.map_err(CappedError::Inner)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        // Dropping the permit frees the slot. rmcp calls this again when the worker ends; the second call finds nothing.
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        (self.on_close)(id.as_ref());
        self.inner.close_session(id).await.map_err(CappedError::Inner)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .create_stream(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn accept_message(&self, id: &SessionId, message: ClientJsonRpcMessage) -> Result<(), Self::Error> {
        self.inner
            .accept_message(id, message)
            .await
            .map_err(CappedError::Inner)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .create_standalone_stream(id)
            .await
            .map_err(CappedError::Inner)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl futures::Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.inner
            .resume(id, last_event_id)
            .await
            .map_err(CappedError::Inner)
    }
}

#[cfg(test)]
mod tests {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;

    use super::*;

    fn user(n: u8) -> UserId {
        format!("11111111-2222-4333-8444-555555555555:aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeee{n:02x}")
            .parse()
            .unwrap()
    }

    /// A closer that only writes down which sessions it was asked to close.
    fn recording() -> (Closer, Arc<Mutex<Vec<String>>>) {
        let closed = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&closed);
        let close: Closer = Arc::new(move |id: SessionId| sink.lock().unwrap().push(id.to_string()));
        (close, closed)
    }

    fn manager(cap: usize) -> Arc<CappedSessionManager<LocalSessionManager>> {
        Arc::new(CappedSessionManager::new(
            LocalSessionManager::default(),
            cap,
            Arc::new(|_: &str| {}),
        ))
    }

    #[test]
    fn a_refused_opening_holds_nothing_and_a_dropped_one_gives_its_slot_back() {
        let counts = Arc::new(SessionCounts::new(2, 3));
        let (close, closed) = recording();
        let a = counts.open(&user(1), Arc::clone(&close)).unwrap();
        let b = counts.open(&user(1), Arc::clone(&close)).unwrap();
        assert_eq!(
            counts.open(&user(1), Arc::clone(&close)).unwrap_err(),
            SessionLimit::User
        );
        assert_eq!(
            (counts.of(&user(1)), counts.total()),
            (2, 2),
            "a refusal holds nothing"
        );
        let c = counts.open(&user(2), Arc::clone(&close)).unwrap();
        assert_eq!(
            counts.open(&user(3), Arc::clone(&close)).unwrap_err(),
            SessionLimit::Replica
        );
        assert_eq!(counts.total(), 3, "a refusal holds nothing");
        drop(a);
        assert_eq!((counts.of(&user(1)), counts.total()), (1, 2));
        drop((b, c));
        assert_eq!(counts.total(), 0);
        assert!(
            closed.lock().unwrap().is_empty(),
            "no session was made, so none is closed"
        );
    }

    #[test]
    fn a_handed_over_opening_keeps_the_slot_for_the_owner_table() {
        let counts = Arc::new(SessionCounts::new(2, 3));
        let (close, closed) = recording();
        let opening = counts.open(&user(1), close).unwrap();
        opening.hand_over();
        assert_eq!(counts.of(&user(1)), 1, "handing over does not release");
        // The owner table's listener is the one release, exactly once.
        counts.release(&user(1));
        assert_eq!((counts.of(&user(1)), counts.total()), (0, 0));
        assert!(
            closed.lock().unwrap().is_empty(),
            "a session that was handed over is not closed"
        );
    }

    #[tokio::test]
    async fn a_dropped_opening_closes_the_session_its_request_made_and_only_that() {
        let counts = Arc::new(SessionCounts::new(5, 5));
        let sessions = manager(5);
        let (close, closed) = recording();
        // The request made a session, and then was dropped (here: the opening is, as the gate's future would drop it).
        let opening = counts.open(&user(1), Arc::clone(&close)).unwrap();
        let (id, _transport) = opening
            .run(async { sessions.create_session().await.unwrap() })
            .await;
        // A session made by a path that did not come through the gate is nobody's to close.
        let (other, _other_transport) = sessions.create_session().await.unwrap();
        assert_eq!(sessions.live(), 2);
        drop(opening);
        assert_eq!(counts.total(), 0);
        assert_eq!(*closed.lock().unwrap(), vec![id.to_string()]);
        assert_ne!(id, other);
    }

    #[tokio::test]
    async fn the_closer_ends_the_session_in_the_manager() {
        let counts = Arc::new(SessionCounts::new(5, 5));
        let sessions = manager(5);
        let opening = counts.open(&user(1), closer(Arc::clone(&sessions))).unwrap();
        let (_id, _transport) = opening
            .run(async { sessions.create_session().await.unwrap() })
            .await;
        assert_eq!(sessions.live(), 1);
        drop(opening);
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            sessions.live(),
            0,
            "the session a dropped request made is still there"
        );
        assert_eq!(counts.total(), 0);
    }

    #[test]
    fn a_closer_with_no_runtime_does_not_panic() {
        let sessions = manager(1);
        // Dropped off a runtime (a thread of its own, at shutdown): nothing to close with, nothing to fail.
        closer(sessions)(Arc::from("no-runtime"));
    }
}
