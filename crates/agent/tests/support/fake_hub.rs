//! A fake hub for the join and renewal paths (S5): it checks the credential, then issues a real certificate for the
//! key in the agent's real CSR, from a real CA, and records everything it was asked.
//!
//! This is the hub's `Join` and `CertRenewal` handling without the transport. The gRPC server built from
//! `proto::grpc::agent_server` arrives with the transport tests (T3) and wraps this.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use agent::clock::Clock;
use agent::identity::idtoken::IdTokenSource;
use agent::identity::joiner::{JoinClient, RenewalChannel};
use agent::identity::jointoken::JoinTokenSource;
use agent::identity::{IdTokenError, JoinError, JoinTokenError, RenewError};
use async_trait::async_trait;
use bytes::Bytes;
use domain::{Secret, ShortText};
use proto::convert::{IssuedCert, JoinCredential, JoinParams, JoinSubject};
use tokio::time::Instant;

use super::test_ca::{IssueSpec, TestCa};

/// Which kind of credential a join carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    GoogleIdToken,
    JoinToken,
}

#[derive(Debug, Clone)]
pub struct JoinRecord {
    pub subject: String,
    pub kind: CredentialKind,
    pub credential: String,
    pub csr: Bytes,
    pub at: Instant,
}

#[derive(Debug, Clone)]
pub struct RenewRecord {
    pub csr: Bytes,
    pub at: Instant,
}

/// What the next certificate looks like, as a function of the swimlane and the wall clock.
type Issuer = dyn Fn(&str, i64) -> IssueSpec + Send + Sync;

struct State {
    expect_google: Option<String>,
    expect_join: Option<String>,
    /// Refuse this many joins before accepting.
    reject_joins: usize,
    /// Fail renewals with these errors, in order; once empty, renewals succeed (unless `renewals_down`).
    renewal_failures: VecDeque<RenewError>,
    renewals_down: bool,
    joins: Vec<JoinRecord>,
    renewals: Vec<RenewRecord>,
    issuer: Arc<Issuer>,
}

pub struct FakeHub {
    ca: TestCa,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl fmt::Debug for FakeHub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FakeHub")
    }
}

impl FakeHub {
    /// A hub that accepts any credential and issues the S5 certificate for the swimlane it is asked about.
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            ca: TestCa::new(),
            clock,
            state: Mutex::new(State {
                expect_google: None,
                expect_join: None,
                reject_joins: 0,
                renewal_failures: VecDeque::new(),
                renewals_down: false,
                joins: Vec::new(),
                renewals: Vec::new(),
                issuer: Arc::new(IssueSpec::agent),
            }),
        })
    }

    /// Accept only this Google ID token.
    pub fn expect_google_token(&self, token: &str) {
        self.state.lock().unwrap().expect_google = Some(token.to_owned());
    }

    /// Accept only this join token.
    pub fn expect_join_token(&self, token: &str) {
        self.state.lock().unwrap().expect_join = Some(token.to_owned());
    }

    pub fn reject_next_joins(&self, n: usize) {
        self.state.lock().unwrap().reject_joins = n;
    }

    pub fn fail_next_renewals(&self, failures: &[RenewError]) {
        self.state
            .lock()
            .unwrap()
            .renewal_failures
            .extend(failures.iter().copied());
    }

    /// Every renewal fails with `NotConnected` from now on, as when the stream is down.
    pub fn stream_down(&self) {
        self.state.lock().unwrap().renewals_down = true;
    }

    /// Issue certificates described by `issuer` instead of the S5 default.
    pub fn issue_with(&self, issuer: impl Fn(&str, i64) -> IssueSpec + Send + Sync + 'static) {
        self.state.lock().unwrap().issuer = Arc::new(issuer);
    }

    pub fn joins(&self) -> Vec<JoinRecord> {
        self.state.lock().unwrap().joins.clone()
    }

    pub fn renewals(&self) -> Vec<RenewRecord> {
        self.state.lock().unwrap().renewals.clone()
    }

    /// A certificate for any CSR, as the hub would issue to `swimlane` now.
    pub fn issue(&self, swimlane: &str, csr: &[u8]) -> IssuedCert {
        let now_ms = self.clock.now().unix_millis();
        let spec = (self.state.lock().unwrap().issuer.clone())(swimlane, now_ms);
        let chain = self.ca.issue_for_csr(csr, &spec);
        IssuedCert {
            cert_chain_der: chain,
            not_after: domain::Timestamp::from_unix_millis(spec.not_after * 1000),
        }
    }
}

#[async_trait]
impl JoinClient for FakeHub {
    async fn join(&self, params: JoinParams) -> Result<IssuedCert, JoinError> {
        let (kind, credential) = match &params.credential {
            JoinCredential::GoogleIdToken(t) => (CredentialKind::GoogleIdToken, t.expose().clone()),
            JoinCredential::JoinToken(t) => (CredentialKind::JoinToken, t.expose().clone()),
        };
        let JoinSubject::Agent(swimlane) = &params.subject else {
            return Err(JoinError::Rejected);
        };
        let accepted = {
            let mut state = self.state.lock().unwrap();
            state.joins.push(JoinRecord {
                subject: swimlane.as_str().to_owned(),
                kind,
                credential: credential.clone(),
                csr: params.csr_der.clone(),
                at: Instant::now(),
            });
            let expected = match kind {
                CredentialKind::GoogleIdToken => state.expect_google.as_ref(),
                CredentialKind::JoinToken => state.expect_join.as_ref(),
            };
            let matches = expected.is_none_or(|e| *e == credential);
            if state.reject_joins > 0 {
                state.reject_joins -= 1;
                false
            } else {
                matches
            }
        };
        if !accepted {
            return Err(JoinError::Rejected);
        }
        Ok(self.issue(swimlane.as_str(), &params.csr_der))
    }
}

#[async_trait]
impl RenewalChannel for FakeHub {
    async fn renew(&self, csr_der: Bytes) -> Result<IssuedCert, RenewError> {
        let swimlane = {
            let mut state = self.state.lock().unwrap();
            state.renewals.push(RenewRecord {
                csr: csr_der.clone(),
                at: Instant::now(),
            });
            if state.renewals_down {
                return Err(RenewError::NotConnected);
            }
            if let Some(failure) = state.renewal_failures.pop_front() {
                return Err(failure);
            }
            // The stream's mTLS identity names the swimlane; the fake uses the one it is told about.
            "sit1"
        };
        Ok(self.issue(swimlane, &csr_der))
    }
}

/// An ID token source that answers from a script and counts its calls.
#[derive(Debug)]
pub struct ScriptedIdTokens {
    answer: Mutex<Result<String, IdTokenError>>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedIdTokens {
    pub fn new(answer: Result<&str, IdTokenError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer.map(str::to_owned)),
            calls: Mutex::new(Vec::new()),
        })
    }

    /// The audiences it was asked for.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl IdTokenSource for ScriptedIdTokens {
    async fn id_token(&self, audience: &ShortText) -> Result<Secret<String>, IdTokenError> {
        self.calls.lock().unwrap().push(audience.as_str().to_owned());
        self.answer.lock().unwrap().clone().map(Secret::new)
    }
}

/// A join token source that answers from a script and counts its calls.
#[derive(Debug)]
pub struct ScriptedJoinTokens {
    answer: Mutex<Result<String, JoinTokenError>>,
    calls: Mutex<usize>,
}

impl ScriptedJoinTokens {
    pub fn new(answer: Result<&str, JoinTokenError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer.map(str::to_owned)),
            calls: Mutex::new(0),
        })
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

#[async_trait]
impl JoinTokenSource for ScriptedJoinTokens {
    async fn join_token(&self) -> Result<Secret<String>, JoinTokenError> {
        *self.calls.lock().unwrap() += 1;
        self.answer.lock().unwrap().clone().map(Secret::new)
    }
}
