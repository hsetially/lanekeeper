//! A failed call (rule 3, S21): `isError: true` and `structuredContent` shaped as the contract's `ToolError`.
//!
//! The message is fixed text chosen by the code; no variant carries a path, a pattern, a hash that was asked for, SQL, or the text of
//! a library error. The cause goes to the log with the request id, and the client gets the code, a plain sentence and the request id to
//! quote. The codes are the contract's, and so are their meanings:
//!
//! | code | when |
//! |---|---|
//! | `invalid_argument` | the arguments are not valid (S11) |
//! | `forbidden` | the role is too low, or the user is pending or disabled (S4, D36) |
//! | `not_found` | the swimlane, file, draft or version does not exist for this caller |
//! | `conflict` | the expected hash did not match; carries `current_hash`; nothing was written |
//! | `locked` | a sync window is open (D75) |
//! | `payload_too_large` | content over 2 MiB |
//! | `unprocessable` | a denied file, invalid content, a secret found (S13) |
//! | `rate_limited` | over the caller's rate; carries `retry_after_secs` |
//! | `confirmation_declined`, `confirmation_cancelled` | the user said no |
//! | `unavailable` | the agent, the Git host, the index or this replica cannot answer now. Retryable |
//! | `internal` | a defect |

use domain::{ContentHash, RejectReason, ValidationIssue};
use ports::{DocError, ReadError, WriteError};
use serde_json::{Value, json};

use crate::limits::Refusal;
use crate::output::redact::RedactError;

/// The contract's error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    InvalidArgument,
    Forbidden,
    NotFound,
    Conflict,
    Locked,
    PayloadTooLarge,
    Unprocessable,
    RateLimited,
    ConfirmationDeclined,
    ConfirmationCancelled,
    Unavailable,
    Internal,
}

impl Code {
    pub const ALL: [Code; 12] = [
        Code::InvalidArgument,
        Code::Forbidden,
        Code::NotFound,
        Code::Conflict,
        Code::Locked,
        Code::PayloadTooLarge,
        Code::Unprocessable,
        Code::RateLimited,
        Code::ConfirmationDeclined,
        Code::ConfirmationCancelled,
        Code::Unavailable,
        Code::Internal,
    ];

    /// The wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Locked => "locked",
            Self::PayloadTooLarge => "payload_too_large",
            Self::Unprocessable => "unprocessable",
            Self::RateLimited => "rate_limited",
            Self::ConfirmationDeclined => "confirmation_declined",
            Self::ConfirmationCancelled => "confirmation_cancelled",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }

    /// The sentence the client reads. Fixed per code.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidArgument => "An argument is not valid. See the input schema of the tool.",
            Self::Forbidden => "You are not allowed to do this.",
            Self::NotFound => "Nothing was found for this request, or you may not see it.",
            Self::Conflict => {
                "The file is not what you expected. Nothing was written. Read it again and retry."
            }
            Self::Locked => "A sync window is open. Try again when it has closed.",
            Self::PayloadTooLarge => "The content is larger than 2 MiB.",
            Self::Unprocessable => "The request was understood but cannot be done.",
            Self::RateLimited => "Too many requests. Wait and try again.",
            Self::ConfirmationDeclined => "The user declined. Nothing was done.",
            Self::ConfirmationCancelled => "The confirmation was cancelled. Nothing was done.",
            Self::Unavailable => "The service cannot answer right now. Try again shortly.",
            Self::Internal => "Something went wrong on the server.",
        }
    }

    /// Whether the same request may succeed later without a change.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::Unavailable | Self::Locked | Self::RateLimited)
    }
}

/// A failed call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: Code,
    /// A more specific fixed sentence than the code's, when there is one.
    pub detail: Option<&'static str>,
    pub current_hash: Option<ContentHash>,
    pub retry_after_secs: Option<u32>,
    pub reason: Option<RejectReason>,
    pub issues: Vec<ValidationIssue>,
}

impl ToolError {
    #[must_use]
    pub fn new(code: Code) -> Self {
        Self {
            code,
            detail: None,
            current_hash: None,
            retry_after_secs: None,
            reason: None,
            issues: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_detail(code: Code, detail: &'static str) -> Self {
        Self {
            detail: Some(detail),
            ..Self::new(code)
        }
    }

    /// The `structuredContent` and the sentence for the `content` block.
    #[must_use]
    pub fn render(&self, request_id: &str) -> (Value, String) {
        let message = self.detail.unwrap_or(self.code.message());
        let mut error = json!({
            "code": self.code.as_str(),
            "message": message,
            "retryable": self.code.retryable(),
            "request_id": request_id,
        });
        if let Some(hash) = &self.current_hash {
            error["current_hash"] = json!(hash.to_string());
        }
        if let Some(secs) = self.retry_after_secs {
            error["retry_after_secs"] = json!(secs);
        }
        if let Some(reason) = self.reason {
            error["reason"] = serde_json::to_value(reason).unwrap_or(Value::Null);
        }
        if !self.issues.is_empty() {
            let issues: Vec<&ValidationIssue> = self
                .issues
                .iter()
                .take(ValidationIssue::MAX_PER_OUTCOME)
                .collect();
            error["issues"] = serde_json::to_value(issues).unwrap_or(Value::Null);
        }
        (json!({ "error": error }), message.to_owned())
    }
}

impl From<ReadError> for ToolError {
    fn from(error: ReadError) -> Self {
        Self::new(match error {
            ReadError::Forbidden => Code::Forbidden,
            ReadError::NotFound => Code::NotFound,
            ReadError::BadRequest => Code::InvalidArgument,
            ReadError::Unavailable => Code::Unavailable,
        })
    }
}

impl From<DocError> for ToolError {
    fn from(error: DocError) -> Self {
        Self::new(match error {
            DocError::Forbidden => Code::Forbidden,
            DocError::NotFound => Code::NotFound,
            DocError::BadRequest => Code::InvalidArgument,
            DocError::Unavailable => Code::Unavailable,
        })
    }
}

impl From<WriteError> for ToolError {
    fn from(error: WriteError) -> Self {
        Self::new(match error {
            WriteError::Forbidden => Code::Forbidden,
            WriteError::NotFound => Code::NotFound,
            WriteError::BadRequest => Code::InvalidArgument,
            // The same key was used for another request: a retry that is not one. The caller starts over.
            WriteError::IdempotencyConflict => Code::Conflict,
            WriteError::Unavailable => Code::Unavailable,
        })
    }
}

impl From<Refusal> for ToolError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::RateLimited { retry_after_secs } => Self {
                retry_after_secs: Some(retry_after_secs),
                ..Self::new(Code::RateLimited)
            },
            Refusal::Overloaded => {
                Self::with_detail(Code::Unavailable, "This server is busy. Try again shortly.")
            }
        }
    }
}

impl From<RedactError> for ToolError {
    /// The scan could not finish, so the text is not sent (S14b). Not the caller's fault and not a defect to hide.
    fn from(_: RedactError) -> Self {
        Self::with_detail(
            Code::Unprocessable,
            "The text could not be checked for secrets, so it is not shown. Read it in the web UI.",
        )
    }
}

impl From<crate::tools::args::ArgError> for ToolError {
    fn from(error: crate::tools::args::ArgError) -> Self {
        use crate::tools::args::ArgError;
        match error {
            ArgError::Invalid => Self::new(Code::InvalidArgument),
            ArgError::TooLarge => Self::new(Code::PayloadTooLarge),
            ArgError::Missing(name) => Self::with_detail(Code::InvalidArgument, missing_sentence(name)),
        }
    }
}

/// The sentence for a missing argument. The names are the tools' own, a closed set of ours, so each is a literal here.
fn missing_sentence(name: &'static str) -> &'static str {
    match name {
        "swimlane" => "The required argument `swimlane` is missing.",
        "path" => "The required argument `path` is missing.",
        "file" => "The required argument `file` is missing.",
        "tenant" => "The required argument `tenant` is missing.",
        "application" => "The required argument `application` is missing.",
        "left" => "The required argument `left` is missing.",
        "right" => "The required argument `right` is missing.",
        "swimlanes" => "The required argument `swimlanes` is missing.",
        "q" => "The required argument `q` is missing.",
        "pattern" => "The required argument `pattern` is missing.",
        "expected" => "The required argument `expected` is missing.",
        "content" => "The required argument `content` is missing.",
        "draft_id" => "The required argument `draft_id` is missing.",
        "expected_hash" => "The required argument `expected_hash` is missing.",
        "paths" => "The required argument `paths` is missing.",
        "title" => "The required argument `title` is missing.",
        "service" => "The required argument `service` is missing.",
        _ => "A required argument is missing.",
    }
}
