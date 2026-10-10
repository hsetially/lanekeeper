use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use domain::{AgentReply, AgentStatus, HubCommand, OpResult, RequestId, SwimlaneId};

use super::util::lock;
use crate::conformance::AgentGatewayScenario;
use crate::{AgentGateway, GatewayError};

/// Most commands in flight per agent; more is [`GatewayError::Busy`].
const MAX_IN_FLIGHT: usize = 64;
/// Most received commands the fake remembers per swimlane.
const KEEP_COMMANDS: usize = 1024;

/// Decides the reply to a command. `None` means the agent stays silent, so the request times out.
pub type AgentScript = Arc<dyn Fn(&HubCommand) -> Option<AgentReply> + Send + Sync>;

#[derive(Clone)]
enum Mode {
    Scripted(AgentScript),
    Detached,
}

struct Agent {
    mode: Mode,
    in_flight: Arc<AtomicUsize>,
    received: VecDeque<HubCommand>,
}

/// Decrements the in-flight counter when the request ends, including when its future is dropped.
struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A gateway to scripted agents. Requests honour their timeout on the tokio clock.
#[derive(Clone, Default)]
pub struct FakeAgentGateway {
    agents: Arc<Mutex<HashMap<SwimlaneId, Agent>>>,
}

impl std::fmt::Debug for FakeAgentGateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeAgentGateway").finish_non_exhaustive()
    }
}

/// Acknowledge every command with a successful `Op`, except `NotifyConfigServer`, which is answered by
/// `AgentReply::Notify` with status 200, as a real agent answers it. Commands without a request id
/// (`RequestDelta`, `RequestFullScan`) are acknowledged with the id `ack`.
pub fn echo_script() -> AgentScript {
    Arc::new(|cmd: &HubCommand| {
        let request_id = match cmd {
            HubCommand::NotifyConfigServer { request_id, .. } => {
                return Some(AgentReply::Notify {
                    request_id: request_id.clone(),
                    status: 200,
                });
            }
            HubCommand::ReadFile { request_id, .. }
            | HubCommand::WriteFile { request_id, .. }
            | HubCommand::DeleteFile { request_id, .. }
            | HubCommand::RestartDeployment { request_id, .. }
            | HubCommand::RequestClusterReport { request_id }
            | HubCommand::FetchServed { request_id, .. } => request_id.clone(),
            HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => RequestId::parse("ack").ok()?,
        };
        Some(AgentReply::Op(OpResult {
            request_id,
            ok: true,
            error: None,
            current_hash: None,
        }))
    })
}

impl FakeAgentGateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connect an agent for `s` that answers with `script`. Replaces any earlier agent.
    pub fn attach(&self, s: &SwimlaneId, script: AgentScript) {
        self.set_mode(s, Mode::Scripted(script));
    }

    /// Disconnect the agent for `s` (its status becomes `Disconnected`).
    pub fn detach(&self, s: &SwimlaneId) {
        self.set_mode(s, Mode::Detached);
    }

    /// The commands the agent for `s` has received, oldest first (the last 1,024).
    pub fn received(&self, s: &SwimlaneId) -> Vec<HubCommand> {
        lock(&self.agents)
            .get(s)
            .map(|a| a.received.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn set_mode(&self, s: &SwimlaneId, mode: Mode) {
        let mut agents = lock(&self.agents);
        match agents.get_mut(s) {
            Some(a) => a.mode = mode,
            None => {
                agents.insert(
                    s.clone(),
                    Agent {
                        mode,
                        in_flight: Arc::new(AtomicUsize::new(0)),
                        received: VecDeque::new(),
                    },
                );
            }
        }
    }
}

#[async_trait]
impl AgentGateway for FakeAgentGateway {
    async fn request(
        &self,
        s: &SwimlaneId,
        cmd: HubCommand,
        timeout: Duration,
    ) -> Result<AgentReply, GatewayError> {
        if timeout.is_zero() {
            return Err(GatewayError::Timeout);
        }
        let (script, slot) = {
            let mut agents = lock(&self.agents);
            let agent = agents.get_mut(s).ok_or(GatewayError::NotConnected)?;
            let Mode::Scripted(script) = agent.mode.clone() else {
                return Err(GatewayError::NotConnected);
            };
            if agent.in_flight.fetch_add(1, Ordering::Relaxed) >= MAX_IN_FLIGHT {
                agent.in_flight.fetch_sub(1, Ordering::Relaxed);
                return Err(GatewayError::Busy);
            }
            if agent.received.len() >= KEEP_COMMANDS {
                agent.received.pop_front();
            }
            agent.received.push_back(cmd.clone());
            (script, Slot(agent.in_flight.clone()))
        };
        let _slot = slot;
        if let Some(reply) = script(&cmd) {
            return Ok(reply);
        }
        // The agent never answers: wait out the timeout, as a real stream would.
        let _ = tokio::time::timeout(timeout, std::future::pending::<()>()).await;
        Err(GatewayError::Timeout)
    }

    async fn status(&self, s: &SwimlaneId) -> AgentStatus {
        match lock(&self.agents).get(s).map(|a| &a.mode) {
            None => AgentStatus::NeverSeen,
            Some(Mode::Detached) => AgentStatus::Disconnected,
            Some(Mode::Scripted(_)) => AgentStatus::Connected,
        }
    }
}

#[async_trait]
impl AgentGatewayScenario for FakeAgentGateway {
    async fn attach_echo_agent(&self, s: &SwimlaneId) {
        self.attach(s, echo_script());
    }

    async fn attach_silent_agent(&self, s: &SwimlaneId) {
        self.attach(s, Arc::new(|_| None));
    }

    async fn detach_agent(&self, s: &SwimlaneId) {
        self.detach(s);
    }
}
