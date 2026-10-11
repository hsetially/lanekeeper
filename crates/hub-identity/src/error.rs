//! Errors of this crate. None of them carries input text: not a token, a cookie, a URL, a claim value or a secret (S10, S21).
//! The edge maps them to `application/problem+json` (T4) and logs only the variant.

use ports::SecretError;

use crate::config::ConfigError;
use crate::http::HttpsError;

/// Why an ID token or access token was refused. Each variant is a check with its own test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    /// Not three base64url segments, bad JSON, or a header or claim of the wrong type.
    #[error("token is malformed")]
    Malformed,
    /// Longer than [`ports::MAX_TOKEN_BYTES`]: refused without parsing.
    #[error("token is too large")]
    Oversize,
    /// `alg` is anything but RS256 (`none` and `HS256` included), or the header asks for `crit`.
    #[error("token algorithm is not accepted")]
    UnsupportedAlgorithm,
    #[error("token has no key id")]
    MissingKeyId,
    /// The key id is not in the key set, even after the one rate-limited refresh.
    #[error("token key is unknown")]
    UnknownKey,
    #[error("token signature is not valid")]
    BadSignature,
    #[error("token issuer is wrong")]
    WrongIssuer,
    #[error("token audience is wrong")]
    WrongAudience,
    #[error("token tenant is wrong")]
    WrongTenant,
    #[error("token nonce does not match")]
    NonceMismatch,
    #[error("token has expired")]
    Expired,
    #[error("token is not valid yet")]
    NotYetValid,
    /// A claim the profile requires is absent.
    #[error("token lacks a required claim")]
    MissingClaim,
    /// The `tid` or `oid` claim is not a GUID.
    #[error("token identity is not valid")]
    BadIdentity,
}

/// Why the discovery document was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("discovery document is malformed")]
    Malformed,
    #[error("discovery issuer does not match the tenant")]
    IssuerMismatch,
    /// An endpoint is not HTTPS, has credentials, or names a host outside the authority.
    #[error("discovery endpoint is not acceptable")]
    EndpointRejected,
}

/// Why a sign-in step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// The callback carried no `code` or `state`, or carried an `error` from the provider.
    #[error("sign-in request is not valid")]
    BadRequest,
    /// The login cookie is missing, tampered with, sealed with another key, or from another cookie name.
    #[error("login state is not valid")]
    LoginStateInvalid,
    #[error("login state has expired")]
    LoginStateExpired,
    #[error("login state does not match the request")]
    StateMismatch,
    /// The token endpoint answered with an error status or an unusable body.
    #[error("token endpoint refused the code")]
    TokenEndpoint,
    #[error("id token was refused: {0}")]
    Token(#[from] TokenError),
    /// The configuration is refused at startup.
    #[error("configuration is not valid: {0}")]
    Config(#[from] ConfigError),
    #[error("discovery failed: {0}")]
    Discovery(#[from] DiscoveryError),
    #[error("identity provider is unreachable: {0}")]
    Http(#[from] HttpsError),
    /// The key set is not JSON, has no usable RSA key, repeats a key id, or holds too many keys.
    #[error("key set is not valid")]
    Jwks,
    #[error("a secret could not be read: {0}")]
    Secret(#[from] SecretError),
    /// The federated assertion file is missing, unreadable, empty, too large or not text.
    #[error("client assertion is unavailable")]
    ClientAssertion,
    /// The system could not make random bytes or seal a cookie.
    #[error("crypto provider failed")]
    Crypto,
}
