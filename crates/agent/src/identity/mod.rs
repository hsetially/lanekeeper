//! Agent identity (T2, S5): the key, the certificate, and how both are obtained and kept fresh.
pub mod cert;
mod csr;
pub mod error;
pub mod idtoken;
pub mod joiner;
pub mod jointoken;
pub mod key;
pub(crate) mod kubecall;
pub mod schedule;
pub mod store;

pub use cert::{AGENT_SAN_PREFIX, ClientIdentity};
pub use error::{
    CertProblem, IdTokenError, IdentityError, JoinError, JoinTokenError, KeyError, KubeError, RenewError,
    StoreError,
};
pub use key::KeyMaterial;
pub use schedule::{Phase, RenewalSchedule};
