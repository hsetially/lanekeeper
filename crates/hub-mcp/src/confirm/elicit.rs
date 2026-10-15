//! Asking the user (D52): one question, a form with one boolean, and what can end the wait.
//!
//! The question goes out on the stream of the tool call. It ends in exactly one of five ways, and in every way but one nothing is
//! written:
//!
//! - the user answers, and [`decide`](super::decision::decide) says what the answer means;
//! - **no answer arrives in time.** The wait is always bounded (120 s by default). rmcp's own cancellation token does not wake a
//!   handler that waits for an answer, and a client that vanishes without cancelling is not noticed by its stateful transport, so
//!   the timeout is what ends those waits (D91);
//! - the client cancels the tool call (the user closed the prompt): the question is withdrawn;
//! - the server stops, which it announces on its **own** signal for the same reason: the question is withdrawn;
//! - the session ends, and rmcp drops the pending request.
//!
//! A withdrawal goes out as a `notifications/cancelled` for the question; rmcp sends it on the standalone stream of the session,
//! not on the stream of the call (D91). The sessions and the questions open on them live in the memory of one replica: an answer
//! that reaches another replica finds no question and changes nothing (A5).
//!
//! [`Asker`] is the seam: the gate talks to it, and only [`RmcpAsker`] knows rmcp (plan A19).

use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{
    BooleanSchema, CancelledNotificationParam, ClientResult, ElicitRequest, ElicitRequestParams,
    ElicitationAction, ElicitationSchema, PrimitiveSchemaDefinition, RequestId, ServerRequest,
};
use rmcp::service::{ElicitationMode, PeerRequestOptions, RequestContext};
use rmcp::{RoleServer, ServiceError};
use tokio::sync::watch;

use super::decision::{Decision, Reply, decide};

/// How a question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The user answered, and this is what the answer means.
    Replied(Decision),
    /// No answer arrived within the time allowed.
    TimedOut,
    /// The question was withdrawn or could not be asked: the call was cancelled, the server is stopping, or the session is gone.
    Aborted,
}

/// Something that can ask the user a yes-or-no question and say how it ended.
#[async_trait]
pub trait Asker: Send + Sync {
    /// Whether the client declared that it can show a form. A client that cannot gets the draft link (D52).
    fn can_ask(&self) -> bool;

    /// Ask `message`, with one boolean to answer, and wait for the end of the question.
    async fn ask(&self, message: &str) -> Answer;
}

/// The question of the form: one boolean, `confirm`, which is off until the user turns it on.
fn form(message: &str) -> Option<ElicitRequestParams> {
    let confirm = BooleanSchema::new()
        .title("Confirm")
        .description("Turn this on and accept to go ahead. Nothing is done otherwise.")
        .with_default(false);
    let schema = ElicitationSchema::builder()
        .required_property("confirm", PrimitiveSchemaDefinition::Boolean(confirm))
        .build()
        .ok()?;
    Some(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: message.to_owned(),
        requested_schema: schema,
    })
}

fn reply_of(action: &ElicitationAction) -> Reply {
    match action {
        ElicitationAction::Accept => Reply::Accept,
        ElicitationAction::Decline => Reply::Decline,
        // A cancel, and any action a later revision of the protocol adds: not a yes.
        _ => Reply::Cancel,
    }
}

/// The asker of a tool call in a session of rmcp.
pub struct RmcpAsker<'a> {
    context: &'a RequestContext<RoleServer>,
    stopping: watch::Receiver<bool>,
    timeout: Duration,
}

impl<'a> RmcpAsker<'a> {
    #[must_use]
    pub fn new(
        context: &'a RequestContext<RoleServer>,
        stopping: watch::Receiver<bool>,
        timeout: Duration,
    ) -> Self {
        Self {
            context,
            stopping,
            timeout,
        }
    }

    async fn withdraw(&self, question: RequestId, reason: &str) {
        let notice = CancelledNotificationParam::new(Some(question), Some(reason.to_owned()));
        // The client may be gone already; a withdrawal that cannot be delivered changes nothing about what was written.
        let _ = self.context.peer.notify_cancelled(notice).await;
    }
}

impl std::fmt::Debug for RmcpAsker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RmcpAsker").finish_non_exhaustive()
    }
}

#[async_trait]
impl Asker for RmcpAsker<'_> {
    fn can_ask(&self) -> bool {
        self.context
            .peer
            .supported_elicitation_modes()
            .contains(&ElicitationMode::Form)
    }

    async fn ask(&self, message: &str) -> Answer {
        if self.context.ct.is_cancelled() || *self.stopping.borrow() {
            return Answer::Aborted;
        }
        let Some(params) = form(message) else {
            return Answer::Aborted;
        };
        let Ok(handle) = self
            .context
            .peer
            .send_request_with_option(
                ServerRequest::ElicitRequest(ElicitRequest::new(params)),
                PeerRequestOptions::with_timeout(self.timeout),
            )
            .await
        else {
            return Answer::Aborted;
        };
        let question = handle.id.clone();
        let reply = handle.await_response();
        tokio::pin!(reply);
        let mut stopping = self.stopping.clone();
        tokio::select! {
            () = self.context.ct.cancelled() => {
                self.withdraw(question, "the tool call was cancelled").await;
                Answer::Aborted
            }
            // The server is stopping, or the sender of the signal is gone, which means the same.
            () = async { let _ = stopping.wait_for(|stop| *stop).await; } => {
                self.withdraw(question, "the server is stopping").await;
                Answer::Aborted
            }
            reply = &mut reply => match reply {
                Ok(ClientResult::ElicitResult(result)) => {
                    Answer::Replied(decide(reply_of(&result.action), result.content.as_ref()))
                }
                // rmcp has already told the client that the question is withdrawn.
                Err(ServiceError::Timeout { .. }) => Answer::TimedOut,
                // The session ended, or the transport closed.
                Err(_) => Answer::Aborted,
                // An answer of another kind than the one asked for is not a yes.
                Ok(_) => Answer::Replied(Decision::Cancelled),
            },
        }
    }
}
