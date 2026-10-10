//! Why a connection to the hub could not be made or kept (S6).
//!
//! These end up in logs and in the health endpoint, so none of them carries a token, a key, a certificate, a path or
//! text the hub sent (S10, S21): only what failed.

/// What went wrong with the TLS handshake. Told apart so that an operator can see at once whether the hub speaks an
/// old protocol version or presents a certificate the pinned CA does not vouch for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TlsFailure {
    /// The hub offers no TLS 1.3 (the agent speaks nothing older, S6).
    #[error("the hub does not speak TLS 1.3")]
    Version,
    /// The hub's certificate is not valid for the hub's name under the pinned CA.
    #[error("the hub's certificate is not accepted under the pinned CA")]
    Certificate,
    /// Anything else: an alert from the hub, a reset, or a protocol violation.
    #[error("the TLS handshake failed")]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The address could not be reached: DNS, connection refused, no route.
    #[error("the hub cannot be reached")]
    Unreachable,
    #[error("TLS: {0}")]
    Tls(TlsFailure),
    /// The hub took too long to answer.
    #[error("the hub did not answer in time")]
    Timeout,
    /// The hub answered the call with a refusal (not authenticated, not allowed, not valid). Trying again with the
    /// same certificate will not help; a renewal or a new join might.
    #[error("the hub refused the agent")]
    Refused,
    /// The stream broke: a reset, a keepalive timeout, or an error from the hub's side.
    #[error("the stream to the hub failed")]
    Stream,
    /// The hub ended the stream in an orderly way, or the local side did.
    #[error("the stream to the hub is closed")]
    Closed,
    /// The transport itself is set up wrongly (an address that is not `https://host[:port]`). Found at startup.
    #[error("the hub address is not usable: {0}")]
    Address(&'static str),
}

/// The hub CA or the client identity could not be turned into a TLS configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TlsError {
    #[error("the hub CA file cannot be read")]
    CaUnreadable,
    #[error("the hub CA file is too large")]
    CaTooLarge,
    #[error("the hub CA file holds no certificate")]
    CaEmpty,
    #[error("the hub CA file holds too many certificates")]
    CaTooMany,
    #[error("the hub CA file is not valid PEM, or holds something other than certificates")]
    CaMalformed,
    #[error("the pinned hub CA certificate is not usable as a trust anchor")]
    CaRejected,
    #[error("the client key is not an ECDSA P-256 key")]
    ClientKey,
    #[error("the TLS library refused the configuration")]
    Config,
}
