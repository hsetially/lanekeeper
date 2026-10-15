//! A Teams Workflows webhook for the notifier tests (T7): a local server that answers from a script and keeps what it was sent.
//!
//! It is the generic local HTTP server of `fake_github` with a script on top: the answers to give next, the answer to give when
//! the queue is empty, and a switch for a healthy webhook that tests can flip between calls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::fake_github::{Handler, LocalGithub, Reply, Seen, Tls};

struct Script {
    next: VecDeque<Reply>,
    otherwise: Reply,
}

pub struct FakeTeams {
    server: LocalGithub,
    script: Arc<Mutex<Script>>,
}

/// What Workflows answers when it takes a card.
pub fn accepted() -> Reply {
    Reply::json(202, "")
}

pub fn status(code: u16) -> Reply {
    Reply::json(code, "{\"error\":{\"code\":\"x\"}}")
}

impl FakeTeams {
    pub async fn start() -> Self {
        Self::start_with(Tls::Plain).await
    }

    pub async fn start_with(tls: Tls) -> Self {
        let script = Arc::new(Mutex::new(Script {
            next: VecDeque::new(),
            otherwise: accepted(),
        }));
        let handler: Handler = {
            let script = Arc::clone(&script);
            Arc::new(move |_, _| {
                let mut s = script.lock().unwrap_or_else(PoisonError::into_inner);
                s.next.pop_front().unwrap_or_else(|| s.otherwise.clone())
            })
        };
        Self {
            server: LocalGithub::start_with(handler, tls).await,
            script,
        }
    }

    /// Every later request gets this answer (after the queued ones).
    pub fn always(&self, reply: Reply) {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .otherwise = reply;
    }

    /// The next request gets this answer.
    pub fn next(&self, reply: Reply) {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next
            .push_back(reply);
    }

    /// Answer slowly from now on.
    pub fn slow(&self, delay: Duration) {
        self.always(accepted().after(delay));
    }

    /// The webhook URL of this server, with a path and a signature in its query like a Workflows URL has.
    pub fn url(&self, flow: &str, signature: &str) -> String {
        format!(
            "{}/workflows/{flow}/triggers/manual/paths/invoke?sig={signature}",
            self.server.base()
        )
    }

    pub fn base(&self) -> String {
        self.server.base()
    }

    pub fn requests(&self) -> usize {
        self.server.requests()
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.server.seen()
    }

    /// The JSON bodies the server was sent.
    pub fn cards(&self) -> Vec<serde_json::Value> {
        self.seen().iter().map(Seen::json).collect()
    }

    /// The certificate of a TLS server.
    pub fn cert(&self) -> Option<rustls::pki_types::CertificateDer<'static>> {
        self.server.cert.clone()
    }
}
