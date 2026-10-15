//! A fake agent behind the `AgentGateway` port, with an in-memory NFS. Unlike `ports::fakes::FakeAgentGateway` (whose script
//! is synchronous), this one can wait, so a test can hold a write in flight, give a hop a delay, or make the reply get lost
//! after the file landed.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use domain::{
    AgentReply, AgentStatus, ContentHash, Expected, HubCommand, NfsPath, OpError, OpResult, SwimlaneId,
};
use ports::{AgentGateway, GatewayError, content_hash};
use tokio::sync::{Notify, Semaphore};

/// What the next write does after the command arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// No agent stream: the command is never sent.
    NotConnected,
    /// The file lands, then the reply is lost (a timeout): the hub cannot know.
    LandThenTimeout,
    /// The file lands, then the stream closes.
    LandThenClosed,
    /// The agent says it had an I/O error and wrote nothing.
    Io,
    /// The agent refuses (a deny glob on its side).
    Denied,
}

impl Fault {
    /// True for a fault that applies after the command has run (the reply is lost).
    pub fn lands(self) -> bool {
        matches!(self, Self::LandThenTimeout | Self::LandThenClosed)
    }
}

struct State {
    files: BTreeMap<(SwimlaneId, NfsPath), Bytes>,
    delay: Duration,
    faults: VecDeque<Fault>,
    gate: Option<Arc<Semaphore>>,
    commands: Vec<HubCommand>,
    down: bool,
}

#[derive(Clone)]
pub struct FakeAgent {
    state: Arc<Mutex<State>>,
    /// Signalled each time a `WriteFile` reaches the agent (before it waits at the gate).
    pub entered: Arc<Notify>,
    writes: Arc<AtomicUsize>,
    deletes: Arc<AtomicUsize>,
    restarts: Arc<AtomicUsize>,
}

impl std::fmt::Debug for FakeAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeAgent").finish_non_exhaustive()
    }
}

impl Default for FakeAgent {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeAgent {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                files: BTreeMap::new(),
                delay: Duration::ZERO,
                faults: VecDeque::new(),
                gate: None,
                commands: Vec::new(),
                down: false,
            })),
            entered: Arc::new(Notify::new()),
            writes: Arc::new(AtomicUsize::new(0)),
            deletes: Arc::new(AtomicUsize::new(0)),
            restarts: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn put(&self, s: &SwimlaneId, p: &NfsPath, bytes: &[u8]) {
        lock(&self.state)
            .files
            .insert((s.clone(), p.clone()), Bytes::copy_from_slice(bytes));
    }

    pub fn content(&self, s: &SwimlaneId, p: &NfsPath) -> Option<Bytes> {
        lock(&self.state).files.get(&(s.clone(), p.clone())).cloned()
    }

    pub fn files_of(&self, s: &SwimlaneId) -> Vec<(NfsPath, Bytes)> {
        lock(&self.state)
            .files
            .iter()
            .filter(|((sl, _), _)| sl == s)
            .map(|((_, p), b)| (p.clone(), b.clone()))
            .collect()
    }

    /// A pause before every command is answered, as the extra hop of a request that reaches another replica.
    pub fn set_delay(&self, d: Duration) {
        lock(&self.state).delay = d;
    }

    /// While down, every command fails as if the agent had no stream.
    pub fn set_down(&self, down: bool) {
        lock(&self.state).down = down;
    }

    /// The next write (and only that one) misbehaves like this.
    pub fn fault_next_write(&self, f: Fault) {
        lock(&self.state).faults.push_back(f);
    }

    /// Hold every `WriteFile` at the agent until a permit is added to the returned semaphore.
    pub fn hold_writes(&self) -> Arc<Semaphore> {
        let gate = Arc::new(Semaphore::new(0));
        lock(&self.state).gate = Some(gate.clone());
        gate
    }

    pub fn release_gate(&self) {
        lock(&self.state).gate = None;
    }

    /// How many `WriteFile` commands changed a file.
    pub fn writes_applied(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    /// How many `DeleteFile` commands removed a file.
    pub fn deletes_applied(&self) -> usize {
        self.deletes.load(Ordering::SeqCst)
    }

    /// How many `RestartDeployment` commands ran.
    pub fn restarts_applied(&self) -> usize {
        self.restarts.load(Ordering::SeqCst)
    }

    pub fn commands(&self) -> Vec<HubCommand> {
        lock(&self.state).commands.clone()
    }

    pub fn write_commands(&self) -> usize {
        self.commands()
            .iter()
            .filter(|c| matches!(c, HubCommand::WriteFile { .. }))
            .count()
    }

    fn op(
        request_id: domain::RequestId,
        ok: bool,
        error: Option<OpError>,
        current: Option<ContentHash>,
    ) -> AgentReply {
        AgentReply::Op(OpResult {
            request_id,
            ok,
            error,
            current_hash: current,
        })
    }
}

#[async_trait]
impl AgentGateway for FakeAgent {
    async fn request(
        &self,
        s: &SwimlaneId,
        cmd: HubCommand,
        timeout: Duration,
    ) -> Result<AgentReply, GatewayError> {
        if timeout.is_zero() {
            return Err(GatewayError::Timeout);
        }
        let (delay, gate, fault) = {
            let mut st = lock(&self.state);
            if st.down {
                return Err(GatewayError::NotConnected);
            }
            st.commands.push(cmd.clone());
            let fault = if matches!(
                cmd,
                HubCommand::WriteFile { .. }
                    | HubCommand::DeleteFile { .. }
                    | HubCommand::RestartDeployment { .. }
            ) {
                st.faults.pop_front()
            } else {
                None
            };
            (st.delay, st.gate.clone(), fault)
        };
        if fault == Some(Fault::NotConnected) {
            return Err(GatewayError::NotConnected);
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        match cmd {
            HubCommand::ReadFile { request_id, path } => {
                let found = lock(&self.state).files.get(&(s.clone(), path.clone())).cloned();
                Ok(match found {
                    Some(bytes) => AgentReply::File {
                        request_id,
                        path,
                        hash: content_hash(&bytes),
                        bytes,
                    },
                    None => Self::op(request_id, false, Some(OpError::NotFound), None),
                })
            }
            HubCommand::WriteFile {
                request_id,
                path,
                expected,
                bytes,
            } => {
                self.entered.notify_waiters();
                self.entered.notify_one();
                if let Some(gate) = gate {
                    // Held until the test adds a permit.
                    let permit = gate.acquire().await.map_err(|_| GatewayError::Closed)?;
                    permit.forget();
                }
                match fault {
                    Some(Fault::Io) => return Ok(Self::op(request_id, false, Some(OpError::Io), None)),
                    Some(Fault::Denied) => {
                        return Ok(Self::op(request_id, false, Some(OpError::Denied), None));
                    }
                    _ => {}
                }
                let key = (s.clone(), path);
                let reply = {
                    let mut st = lock(&self.state);
                    let current = st.files.get(&key).map(|b| content_hash(b));
                    if expected.matches(current.as_ref()) {
                        let new_hash = content_hash(&bytes);
                        st.files.insert(key, bytes);
                        self.writes.fetch_add(1, Ordering::SeqCst);
                        Self::op(request_id, true, None, Some(new_hash))
                    } else {
                        Self::op(request_id, false, Some(OpError::Conflict), current)
                    }
                };
                match fault {
                    Some(Fault::LandThenTimeout) => Err(GatewayError::Timeout),
                    Some(Fault::LandThenClosed) => Err(GatewayError::Closed),
                    _ => Ok(reply),
                }
            }
            HubCommand::DeleteFile {
                request_id,
                path,
                expected,
            } => {
                match fault {
                    Some(Fault::Io) => return Ok(Self::op(request_id, false, Some(OpError::Io), None)),
                    Some(Fault::Denied) => {
                        return Ok(Self::op(request_id, false, Some(OpError::Denied), None));
                    }
                    _ => {}
                }
                let key = (s.clone(), path);
                let reply = {
                    let mut st = lock(&self.state);
                    let current = st.files.get(&key).map(|b| content_hash(b));
                    match current {
                        None => Self::op(request_id, false, Some(OpError::NotFound), None),
                        Some(c) if c == expected => {
                            st.files.remove(&key);
                            self.deletes.fetch_add(1, Ordering::SeqCst);
                            Self::op(request_id, true, None, None)
                        }
                        Some(c) => Self::op(request_id, false, Some(OpError::Conflict), Some(c)),
                    }
                };
                match fault {
                    Some(Fault::LandThenTimeout) => Err(GatewayError::Timeout),
                    Some(Fault::LandThenClosed) => Err(GatewayError::Closed),
                    _ => Ok(reply),
                }
            }
            HubCommand::NotifyConfigServer { request_id, .. } => Ok(AgentReply::Notify {
                request_id,
                status: 200,
            }),
            HubCommand::RestartDeployment { request_id, .. } => {
                match fault {
                    Some(Fault::Io) => return Ok(Self::op(request_id, false, Some(OpError::Io), None)),
                    Some(Fault::Denied) => {
                        return Ok(Self::op(request_id, false, Some(OpError::Denied), None));
                    }
                    _ => {}
                }
                self.restarts.fetch_add(1, Ordering::SeqCst);
                match fault {
                    Some(Fault::LandThenTimeout) => Err(GatewayError::Timeout),
                    Some(Fault::LandThenClosed) => Err(GatewayError::Closed),
                    _ => Ok(Self::op(request_id, true, None, None)),
                }
            }
            HubCommand::RequestClusterReport { request_id } => Ok(Self::op(request_id, true, None, None)),
            HubCommand::FetchServed { .. }
            | HubCommand::RequestDelta { .. }
            | HubCommand::RequestFullScan => Err(GatewayError::Unavailable),
        }
    }

    async fn status(&self, _s: &SwimlaneId) -> AgentStatus {
        AgentStatus::Connected
    }
}

/// The expected state a write would carry for a file that has this content, or `Absent` for none.
pub fn expect_of(bytes: Option<&[u8]>) -> Expected {
    match bytes {
        Some(b) => Expected::Hash {
            hash: content_hash(b),
        },
        None => Expected::Absent,
    }
}
