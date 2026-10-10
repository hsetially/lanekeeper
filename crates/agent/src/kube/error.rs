//! What can go wrong with the cluster side (T6). None of these carries an object, a response body or an environment
//! value (S10, S21): only what failed, and the HTTP status where there is one.

use domain::OpError;

use crate::identity::KubeError;

/// A glob or label from the settings could not be built. `Settings` has already validated them, so this is a bug in
/// the caller, not in the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("{setting} holds a glob that does not compile")]
    BadGlob { setting: &'static str },
}

/// A full cluster report could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// The watchers have not yet listed every Deployment and Pod. A report now would say that pods are missing.
    #[error("the cluster watchers have not finished their first list")]
    NotSynced,
}

impl ReportError {
    /// The code the hub is told.
    pub fn code(&self) -> OpError {
        OpError::Io
    }
}

/// Why a restart did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RestartError {
    /// The Deployment is not in a namespace this agent watches (S17). Nothing was sent to the API server.
    #[error("the namespace is not one this agent may restart Deployments in")]
    NamespaceNotAllowed,
    #[error("the Deployment does not exist")]
    NotFound,
    #[error("{0}")]
    Api(#[from] KubeError),
}

impl RestartError {
    /// The code the hub is told.
    pub fn code(&self) -> OpError {
        match self {
            Self::NamespaceNotAllowed => OpError::Denied,
            Self::NotFound => OpError::NotFound,
            Self::Api(_) => OpError::Io,
        }
    }
}
