//! Why a seed run was refused or failed (plan 18, T7; S10).
//!
//! **No error repeats the file.** The seed file is text that an operator wrote and that may hold anything pasted by mistake, so an
//! error names a *place* (`swimlanes[2].agent_service_account`, built from constant names and numbers) and a fixed *reason*, and
//! never a value. The only text in any message is a constant or a number, which is what `FieldPath` and `Reason` hold, and what
//! the fuzz target `hub_seed_file` checks for every input.

use std::fmt;

/// The largest file `hub seed` reads. A seed file for the design scale (40 swimlanes) is a few kilobytes.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// The most rows one run may change. The audit events of a run are one `record_many` batch (`ports::MAX_AUDIT_BATCH`), and the
/// audit chain's lock is held from that call to the commit, so a run that changed more would hold up every other audited write.
pub const MAX_CHANGES: usize = ports::MAX_AUDIT_BATCH;

/// Where in the file a problem is, built from constants and numbers only: `version`, `swimlanes[2].agent_service_account`,
/// `swimlanes[2].tenants[0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldPath {
    pub section: &'static str,
    /// The position of the entry in its section (counted from 0), when the problem is in an entry.
    pub item: Option<usize>,
    pub field: Option<&'static str>,
    /// The position within a list field (`tenants`), when the problem is in one element.
    pub element: Option<usize>,
}

impl FieldPath {
    #[must_use]
    pub const fn section(section: &'static str) -> Self {
        Self {
            section,
            item: None,
            field: None,
            element: None,
        }
    }

    #[must_use]
    pub const fn item(section: &'static str, item: usize) -> Self {
        Self {
            section,
            item: Some(item),
            field: None,
            element: None,
        }
    }

    #[must_use]
    pub const fn field(section: &'static str, item: usize, field: &'static str) -> Self {
        Self {
            section,
            item: Some(item),
            field: Some(field),
            element: None,
        }
    }

    #[must_use]
    pub const fn element(section: &'static str, item: usize, field: &'static str, element: usize) -> Self {
        Self {
            section,
            item: Some(item),
            field: Some(field),
            element: Some(element),
        }
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.section)?;
        if let Some(item) = self.item {
            write!(f, "[{item}]")?;
        }
        if let Some(field) = self.field {
            write!(f, ".{field}")?;
        }
        if let Some(element) = self.element {
            write!(f, "[{element}]")?;
        }
        Ok(())
    }
}

/// What is wrong with a value. Fixed text; the value is never shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Reason {
    #[error("must not be empty")]
    Empty,
    #[error("is too long")]
    TooLong,
    #[error("holds a control character")]
    Control,
    #[error("starts or ends with a space")]
    Padded,
    #[error("is not a valid id of this kind")]
    BadId,
    #[error("is not a valid email address")]
    BadEmail,
    #[error("is not a Google service account (it must end in .gserviceaccount.com)")]
    NotServiceAccount,
    #[error("is not a clean absolute path")]
    BadPath,
    #[error("is not a host name or address")]
    BadHost,
    #[error("is not a version this hub reads")]
    Unsupported,
    #[error("lists more entries than a file may")]
    TooMany,
}

/// Why `hub seed` did not do what was asked. Every message is fixed text, a constant name or a number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeedError {
    // ------------------------------------------------------------------------------------------------ the file
    #[error("the seed file could not be read (it must be a regular file that the Job can open)")]
    Read,
    #[error("the seed file is larger than {MAX_FILE_BYTES} bytes")]
    TooLarge,
    #[error("the seed file is not UTF-8 text")]
    NotText,
    /// The file is not YAML or JSON of the seed format: a syntax error, an unknown field, a missing field, a value of the wrong
    /// type, a second document, or a budget of the parser.
    #[error(
        "the seed file is not YAML or JSON of the seed format{place}: an unknown or missing field, a value of the wrong type, a second document, or more than the parser's budget"
    )]
    Syntax { place: Place },
    #[error("{field} {reason}")]
    Invalid { field: FieldPath, reason: Reason },
    #[error("{field} is listed more than once")]
    Duplicate { field: FieldPath },
    // ------------------------------------------------------------------------------------------------ the file against the database
    #[error("{field} names a project that is neither in the file nor in the database")]
    UnknownProject { field: FieldPath },
    #[error("{field} names a swimlane that is neither in the file nor in the database")]
    UnknownSwimlane { field: FieldPath },
    #[error("{field} is required to create a row that the database does not have yet")]
    MissingOnCreate { field: FieldPath },
    #[error("this run would change {changes} rows and one run changes at most {MAX_CHANGES}; split the file")]
    TooManyChanges { changes: usize },
    #[error("the audit events of this run are too large for one batch; split the file")]
    AuditTooLarge,
    // ------------------------------------------------------------------------------------------------ the database
    #[error("the database URL (LK_DATABASE_URL_FILE) is not a valid database URL")]
    Url,
    #[error("refusing to run: hub seed runs only as the role lanekeeper_app")]
    WrongRole,
    #[error(
        "could not connect to the database as the application role (the server, its certificate and the CA are checked)"
    )]
    Connect,
    #[error("the database refused a statement of the seed (SQLSTATE {0})")]
    Database(String),
    #[error("the audit log refused the seed's events; nothing was written")]
    Audit,
    #[error("a row changed while the seed was running; nothing was written, run it again")]
    Raced,
    #[error("the seed did not finish within {0} seconds (LK_SEED_TIMEOUT_SECS)")]
    TimedOut(u64),
}

/// Where the parser stopped, when it said: a line and a column, both counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place(Option<(u64, u64)>);

impl Place {
    #[must_use]
    pub const fn at(line: u64, column: u64) -> Self {
        Self(Some((line, column)))
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self(None)
    }
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some((line, column)) => write!(f, " at line {line}, column {column}"),
            None => Ok(()),
        }
    }
}

impl SeedError {
    /// The process exit code: 2 when the file or the command was refused (nothing was tried), 1 when the run could not be
    /// completed (the database, the audit log, the clock). Same meaning as for `hub migrate`.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Read
            | Self::TooLarge
            | Self::NotText
            | Self::Syntax { .. }
            | Self::Invalid { .. }
            | Self::Duplicate { .. }
            | Self::UnknownProject { .. }
            | Self::UnknownSwimlane { .. }
            | Self::MissingOnCreate { .. }
            | Self::TooManyChanges { .. }
            | Self::AuditTooLarge => 2,
            Self::Url
            | Self::WrongRole
            | Self::Connect
            | Self::Database(_)
            | Self::Audit
            | Self::Raced
            | Self::TimedOut(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_is_a_section_an_entry_a_field_and_an_element() {
        assert_eq!(FieldPath::section("version").to_string(), "version");
        assert_eq!(FieldPath::item("swimlanes", 2).to_string(), "swimlanes[2]");
        assert_eq!(
            FieldPath::field("swimlanes", 2, "agent_service_account").to_string(),
            "swimlanes[2].agent_service_account"
        );
        assert_eq!(
            FieldPath::element("swimlanes", 0, "tenants", 3).to_string(),
            "swimlanes[0].tenants[3]"
        );
    }

    #[test]
    fn a_syntax_error_gives_a_place_when_the_parser_did() {
        let with = SeedError::Syntax {
            place: Place::at(12, 5),
        }
        .to_string();
        assert!(with.contains("line 12, column 5"), "{with}");
        let without = SeedError::Syntax {
            place: Place::unknown(),
        }
        .to_string();
        assert!(!without.contains("line"), "{without}");
    }

    #[test]
    fn the_exit_code_separates_a_refused_file_from_a_failed_run() {
        let refused = SeedError::Invalid {
            field: FieldPath::section("version"),
            reason: Reason::Unsupported,
        };
        assert_eq!(refused.exit_code(), 2);
        assert_eq!(SeedError::TooManyChanges { changes: 201 }.exit_code(), 2);
        assert_eq!(SeedError::WrongRole.exit_code(), 1);
        assert_eq!(SeedError::Database("23505".to_owned()).exit_code(), 1);
        assert_eq!(SeedError::TimedOut(120).exit_code(), 1);
    }
}
