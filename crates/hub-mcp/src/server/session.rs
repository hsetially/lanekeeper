//! Sessions (rule 5, S14): how many an MCP user and this replica may hold, and what happens to the count when one ends.
//!
//! rmcp keeps a session, and the channel an elicitation answer returns on, in one replica's memory. Without a limit a caller could
//! open sessions until the replica ran out of memory, so there are two: 5 per user and 200 per replica (configurable). The gate
//! counts a session from the moment it is asked for (a reservation, so that two requests at the limit cannot both pass), and the
//! count falls when the session ends: `DELETE`, the idle expiry of rmcp (30 minutes) or the end of the owner's record.
//! [`CappedSessionManager`] is the second line: rmcp's own manager behind a semaphore, in case a session is made by a path that did
//! not pass the gate.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

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

    /// Count one more session for `user`, or say why not.
    ///
    /// # Errors
    ///
    /// [`SessionLimit`] when a limit is reached. Nothing is counted then.
    pub fn reserve(&self, user: &UserId) -> Result<(), SessionLimit> {
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

    /// One session of `user` is gone (or was never made).
    pub fn release(&self, user: &UserId) {
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
