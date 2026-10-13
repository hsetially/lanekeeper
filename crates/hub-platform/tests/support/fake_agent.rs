//! A fake agent that speaks the real protocol: real mutual TLS, real `Connect` stream, real `HubMessage` decoding. It answers
//! commands by request id the way a real agent does (an operation result, or the specific reply for a notify), after an
//! optional delay, or never.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use domain::{AgentReply, HubCommand, OpResult, RequestId};
use proto::convert::{FromAgent, ToAgent};
use proto::pb;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Status};

use super::ca::TestCa;
use super::grpc::{ClientIdentity, agent_client, agent_identity, client_tls, dial};

#[derive(Debug, Clone, Copy)]
pub enum Behavior {
    /// Answer every command with the answer a real agent gives, after `delay`.
    Echo { delay: Duration },
    /// Read commands and never answer.
    Silent,
}

impl Behavior {
    pub fn echo() -> Self {
        Self::Echo {
            delay: Duration::ZERO,
        }
    }
}

/// The answer a real agent gives to `command`, or `None` for the commands that have no answer message.
pub fn answer(command: &HubCommand) -> Option<AgentReply> {
    let ok = |request_id: &RequestId| {
        AgentReply::Op(OpResult {
            request_id: request_id.clone(),
            ok: true,
            error: None,
            current_hash: None,
        })
    };
    match command {
        HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan => None,
        HubCommand::NotifyConfigServer { request_id, .. } => Some(AgentReply::Notify {
            request_id: request_id.clone(),
            status: 200,
        }),
        HubCommand::ReadFile { request_id, .. }
        | HubCommand::WriteFile { request_id, .. }
        | HubCommand::DeleteFile { request_id, .. }
        | HubCommand::RestartDeployment { request_id, .. }
        | HubCommand::RequestClusterReport { request_id }
        | HubCommand::FetchServed { request_id, .. } => Some(ok(request_id)),
    }
}

pub struct FakeAgent {
    pub swimlane: String,
    pub identity: ClientIdentity,
}

impl FakeAgent {
    pub async fn new(ca: &TestCa, swimlane: &str) -> Self {
        Self {
            swimlane: swimlane.to_owned(),
            identity: agent_identity(ca, swimlane).await,
        }
    }

    fn hello(&self) -> pb::AgentMessage {
        pb::AgentMessage {
            kind: Some(pb::agent_message::Kind::Hello(pb::Hello {
                agent_version: "0.1.0".to_owned(),
                swimlane: self.swimlane.clone(),
                cluster: "cluster-a".to_owned(),
                project: "proj".to_owned(),
                nfs_server: "nfs.internal".to_owned(),
                export: "/export".to_owned(),
                mount_root: "/mnt/config".to_owned(),
            })),
        }
    }

    /// Open a `Connect` stream to the hub at `addr`. Returns once the hub has answered, which for a hub with a gateway is after
    /// the stream is registered.
    pub async fn connect(
        &self,
        ca: &TestCa,
        addr: SocketAddr,
        behavior: Behavior,
    ) -> Result<AgentLink, Status> {
        let tls = client_tls(ca, Some(&self.identity));
        let h2 = dial(addr, &tls)
            .await
            .map_err(|e| Status::unavailable(e.to_string()))?;
        let mut client = agent_client(h2);
        let (tx, rx) = mpsc::channel::<pb::AgentMessage>(256);
        tx.send(self.hello()).await.unwrap();
        let mut stream = client
            .connect(Request::new(ReceiverStream::new(rx)))
            .await?
            .into_inner();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let ended = Arc::new(AtomicBool::new(false));
        // The only long-lived sender: closing the link drops it, which ends the request stream toward the hub.
        let sender = Arc::new(Mutex::new(Some(tx)));
        let task = {
            let (commands, ended, sender) = (Arc::clone(&commands), Arc::clone(&ended), Arc::clone(&sender));
            tokio::spawn(async move {
                // A closed stream or an error both end the loop.
                while let Ok(Some(message)) = stream.message().await {
                    let Ok(Some(ToAgent::Command(command))) = ToAgent::from_proto(message) else {
                        continue;
                    };
                    commands
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(command.clone());
                    let (Behavior::Echo { delay }, Some(reply)) = (behavior, answer(&command)) else {
                        continue;
                    };
                    let Some(tx) = sender.lock().unwrap_or_else(PoisonError::into_inner).clone() else {
                        continue;
                    };
                    tokio::spawn(async move {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        let _ = tx.send(FromAgent::Reply(reply).into_proto()).await;
                    });
                }
                ended.store(true, Ordering::SeqCst);
            })
        };
        Ok(AgentLink {
            commands,
            ended,
            sender,
            task,
        })
    }
}

/// One open `Connect` stream of a fake agent.
pub struct AgentLink {
    commands: Arc<Mutex<Vec<HubCommand>>>,
    ended: Arc<AtomicBool>,
    sender: Arc<Mutex<Option<mpsc::Sender<pb::AgentMessage>>>>,
    task: JoinHandle<()>,
}

impl AgentLink {
    /// The commands this stream has received, oldest first.
    pub fn received(&self) -> Vec<HubCommand> {
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the hub ended the stream (or it broke).
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Close the stream from the agent's side, as an agent that is going away does.
    pub async fn close(&mut self) {
        self.sender.lock().unwrap_or_else(PoisonError::into_inner).take();
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.task).await;
    }

    /// Send a message on the stream, as the agent.
    pub async fn send(&self, message: pb::AgentMessage) {
        let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(sender) = sender {
            let _ = sender.send(message).await;
        }
    }
}

impl Drop for AgentLink {
    fn drop(&mut self) {
        self.task.abort();
    }
}
