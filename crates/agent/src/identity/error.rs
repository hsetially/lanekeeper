//! Every way that joining, storing and renewing a certificate can fail (S5).
//!
//! These are shown to operators in logs and in the health endpoint, so none of them carries a token, a key, a Secret
//! value or a response body (S10, S21): only what failed, and the HTTP status where there is one.

/// Why an issued certificate was not accepted. Checked before anything is stored (S5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CertProblem {
    #[error("the certificate chain is empty")]
    EmptyChain,
    #[error("a certificate in the chain cannot be parsed")]
    Unparseable,
    #[error("the certificate was issued for a different key than ours")]
    WrongKey,
    #[error("the certificate is not valid yet")]
    NotYetValid,
    #[error("the certificate has expired")]
    Expired,
    #[error("the certificate does not end after it begins")]
    NoLifetime,
    #[error("the certificate is valid for longer than the hub should ever issue")]
    LifetimeTooLong,
    #[error("the certificate has no URI subject alternative name")]
    NoUriSan,
    #[error("the certificate has more than one URI subject alternative name")]
    SeveralUriSans,
    #[error("the certificate names another identity than this swimlane's agent identity")]
    WrongIdentity,
}

/// The P-256 key could not be made, read or used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a usable ECDSA P-256 key")]
pub struct KeyError;

/// A call to the Kubernetes API failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KubeError {
    #[error("the Kubernetes API answered {status} to {op}")]
    Status { op: &'static str, status: u16 },
    #[error("the Kubernetes API did not answer {op} in time")]
    Timeout { op: &'static str },
    #[error("{op} reached no usable answer from the Kubernetes API")]
    Transport { op: &'static str },
}

impl KubeError {
    /// The HTTP status, when the API server answered.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::Timeout { .. } | Self::Transport { .. } => None,
        }
    }
}

/// The Google ID token could not be had from the metadata server (Workload Identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdTokenError {
    /// Nothing answered: connection refused, no route, or no reply in time. This is the one case in which the join
    /// token may be used instead (Q26).
    #[error("the metadata server is not reachable")]
    Unavailable,
    /// The metadata server answered, but not with a token. A misconfiguration, so no fallback.
    #[error("the metadata server answered {status}")]
    Refused { status: u16 },
    #[error("the metadata server returned something that is not an ID token")]
    Malformed,
}

/// The join-token fallback Secret could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinTokenError {
    #[error("{0}")]
    Api(#[from] KubeError),
    #[error("the join token Secret does not exist")]
    SecretMissing,
    #[error("the join token Secret has no usable `token` entry")]
    Missing,
}

/// The certificate Secret could not be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Api(#[from] KubeError),
    /// The chart must create the Secret: the agent may only get and update it (S17).
    #[error("the certificate Secret does not exist; the chart must create it")]
    SecretMissing,
    #[error("the certificate Secret does not hold a usable key and certificate")]
    Malformed,
}

/// What the hub said to a `Join`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JoinError {
    #[error("the hub could not be reached")]
    Unavailable,
    /// The hub refused the credential or the swimlane. Retrying the same credential will not help.
    #[error("the hub refused the credential")]
    Rejected,
    #[error("the hub answered with something that is not a certificate")]
    Invalid,
}

/// What went wrong with a renewal over the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RenewError {
    #[error("the stream to the hub is not connected")]
    NotConnected,
    #[error("the hub did not answer the renewal in time")]
    Timeout,
    #[error("the hub refused the renewal")]
    Refused,
    #[error("the hub answered the renewal with something that is not a certificate")]
    Invalid,
}

/// Everything that can stop the agent from having a client certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("no Workload Identity token: {0}")]
    IdToken(#[from] IdTokenError),
    #[error("no join token: {0}")]
    JoinToken(#[from] JoinTokenError),
    #[error("this join mode needs a join token Secret, and LK_JOIN_TOKEN_SECRET is not set")]
    NoJoinTokenSecret,
    #[error("join failed: {0}")]
    Join(#[from] JoinError),
    #[error("renewal failed: {0}")]
    Renew(#[from] RenewError),
    #[error("the issued certificate is not usable: {0}")]
    Certificate(#[from] CertProblem),
    #[error("certificate Secret: {0}")]
    Store(#[from] StoreError),
}
