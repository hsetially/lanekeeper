//! The errors this crate answers with outside a tool result (S21, rule 3).
//!
//! A closed set with fixed text. No variant carries input (a token, a header, a label, a path) and none carries the text of a
//! library error, so what reaches a client or a log cannot leak. The cause of a failure goes to the log, as a class, and the
//! client gets the status and the stable code of the `application/problem+json` body the hub renders.

use http::StatusCode;
use hub_platform::ApiError;
use ports::AuthError;

/// Why an authentication or token request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum McpError {
    /// No valid credential: no token, a token that is not valid, expired, revoked, for another audience, or of an unknown user.
    #[error("authentication is required")]
    Unauthenticated,
    /// A valid credential of a user who may not do this: pending, disabled, or a role that is too low (D36).
    #[error("not allowed")]
    Forbidden,
    /// The identity provider or the token store cannot answer. Not a verdict on the credential, so not a 401.
    #[error("authentication is unavailable")]
    AuthUnavailable,
    /// The thing does not exist for this caller, or this deployment does not use the feature (personal tokens in OAuth mode).
    #[error("not found")]
    NotFound,
    /// The request is malformed: a body, a label, a cursor, a limit.
    #[error("the request is not valid")]
    InvalidRequest,
    /// The body of a request is larger than the route reads.
    #[error("the request body is too large")]
    PayloadTooLarge,
    /// The body of a request is not JSON.
    #[error("unsupported media type")]
    UnsupportedMediaType,
    /// The user already holds the most live personal tokens the hub allows.
    #[error("too many live tokens")]
    TokenLimit,
    /// The user already holds the most MCP sessions the server allows.
    #[error("too many sessions")]
    TooManySessions,
    /// The database or another dependency is not available. Retryable.
    #[error("the service is unavailable")]
    Unavailable,
    /// A bug or a damaged store. The log has the class.
    #[error("internal error")]
    Internal,
}

impl McpError {
    /// The problem this error renders as. The hub's edge adds the request id.
    #[must_use]
    pub fn api_error(self) -> ApiError {
        match self {
            Self::Unauthenticated => ApiError::Unauthorized,
            Self::Forbidden => ApiError::Forbidden,
            Self::NotFound => ApiError::NotFound,
            Self::InvalidRequest => ApiError::BadRequest,
            Self::PayloadTooLarge => ApiError::PayloadTooLarge,
            Self::UnsupportedMediaType => ApiError::UnsupportedMediaType,
            Self::TokenLimit => ApiError::Conflict,
            Self::TooManySessions => ApiError::TooManyRequests,
            Self::AuthUnavailable | Self::Unavailable => ApiError::Unavailable,
            Self::Internal => ApiError::Internal,
        }
    }

    /// The HTTP status.
    #[must_use]
    pub fn status(self) -> StatusCode {
        self.api_error().status()
    }

    /// The stable code of the problem body.
    #[must_use]
    pub fn code(self) -> &'static str {
        self.api_error().code()
    }
}

impl From<AuthError> for McpError {
    /// The same verdicts the hub's route guard gives: 401 for a credential that is not good, 403 for a user who may not, 503 when
    /// the provider cannot say.
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::InvalidToken
            | AuthError::Expired
            | AuthError::WrongAudience
            | AuthError::UnknownUser => Self::Unauthenticated,
            AuthError::Pending | AuthError::Disabled | AuthError::InsufficientRole => Self::Forbidden,
            AuthError::Unavailable => Self::AuthUnavailable,
        }
    }
}
