//! Errors of the write path. None carries input text: a rejected value may be a secret (S21), so an error is a stage and a
//! kind, both fixed words, and the details that help an operator (an SQLSTATE) are logged with the request id.

use ports::WriteError;

/// Why the write hub could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("write configuration is not valid: {0}")]
    Invalid(&'static str),
    #[error("an extra deny glob is not valid")]
    DenyGlob,
}

/// A failure inside the write path that the caller cannot act on: the store, a port or the agent did not do what was asked.
/// It becomes [`WriteError::Unavailable`] at the edge; this value is for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{what} failed during {stage}")]
pub(crate) struct Internal {
    pub stage: &'static str,
    pub what: &'static str,
}

pub(crate) type Res<T> = Result<T, Internal>;

impl Internal {
    pub fn new(stage: &'static str, what: &'static str) -> Self {
        Self { stage, what }
    }

    /// A database error. Only the SQLSTATE (or the class of the error) is logged: the message of the database can quote values.
    pub fn db(stage: &'static str, e: &sqlx::Error) -> Self {
        let code = e
            .as_database_error()
            .and_then(|d| d.code().map(|c| c.to_string()))
            .unwrap_or_default();
        let class = match e {
            sqlx::Error::PoolTimedOut => "pool_timeout",
            sqlx::Error::Io(_) | sqlx::Error::Tls(_) => "io",
            sqlx::Error::Database(_) => "database",
            sqlx::Error::RowNotFound => "no_row",
            sqlx::Error::PoolClosed | sqlx::Error::WorkerCrashed => "closed",
            _ => "other",
        };
        tracing::warn!(stage, class, sqlstate = %code, "database call failed");
        Self {
            stage,
            what: "database",
        }
    }

    pub fn timeout(stage: &'static str) -> Self {
        tracing::warn!(stage, "call timed out");
        Self {
            stage,
            what: "timeout",
        }
    }
}

impl From<Internal> for WriteError {
    fn from(_: Internal) -> Self {
        WriteError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_copy<T: Copy>() {}

    /// An error that is `Copy` cannot own a `String`, so it cannot hold text that came in with a request.
    #[test]
    fn errors_are_copy_and_carry_no_input_text() {
        is_copy::<Internal>();
        is_copy::<ConfigError>();
        for e in [ConfigError::Invalid("a fixed word"), ConfigError::DenyGlob] {
            assert!(!e.to_string().is_empty());
        }
        let i = Internal::new("claim", "database");
        assert_eq!(i.to_string(), "database failed during claim");
        assert_eq!(WriteError::from(i), WriteError::Unavailable);
    }
}
