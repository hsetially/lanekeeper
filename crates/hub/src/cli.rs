//! The command line (plan 18, T1): `hub` and `hub serve` run the hub, `hub help` prints the usage.
//!
//! A hand parser, because there are only a few words and a parser crate would be a dependency on a trust boundary for nothing.
//! The arguments are untrusted text (rule 10): an error is a fixed message that never repeats the argument, so an argument
//! that is a pasted secret does not end up in a log. `migrate` is T2; a later task adds `seed` (T7).

/// What the process was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Run the hub until told to stop. The default, so `hub` with no arguments is what the container's entry point runs.
    Serve,
    /// Apply the database migrations as `lanekeeper_migrator` and exit (T2, G4). Run as a Job before the hub rolls: the
    /// serving hub never migrates at start, so the migrator credential is never in its pod.
    Migrate,
    /// Print [`USAGE`] and exit 0.
    Help,
}

/// Why the arguments were refused. The text is fixed; it never contains the argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("unknown command")]
    UnknownCommand,
    #[error("unexpected argument")]
    UnexpectedArgument,
}

/// The text `hub help` prints and a usage error points at.
pub const USAGE: &str = "usage: hub [serve]\n       hub migrate\n       hub help\n\nserve    run the hub until SIGTERM or SIGINT (the default); settings are the LK_* environment variables\nmigrate  apply the database migrations as lanekeeper_migrator and exit; needs LK_MIGRATOR_URL_FILE and LK_DB_CA_FILE\nhelp     print this text\n";

/// Parse the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Serve);
    };
    let command = match first.as_ref() {
        "serve" => Command::Serve,
        "migrate" => Command::Migrate,
        "help" | "--help" | "-h" => Command::Help,
        _ => return Err(CliError::UnknownCommand),
    };
    if args.next().is_some() {
        return Err(CliError::UnexpectedArgument);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().copied())
    }

    #[test]
    fn no_arguments_serve() {
        assert_eq!(parse_all(&[]), Ok(Command::Serve));
        assert_eq!(parse_all(&["serve"]), Ok(Command::Serve));
    }

    #[test]
    fn migrate_is_a_command_of_its_own() {
        assert_eq!(parse_all(&["migrate"]), Ok(Command::Migrate));
        assert_eq!(parse_all(&["migrate", "now"]), Err(CliError::UnexpectedArgument));
        // Spelled exactly: the migration job is a fixed word in a manifest, not something to guess at.
        for word in ["Migrate", "--migrate", "migrations", "migrate "] {
            assert_eq!(parse_all(&[word]), Err(CliError::UnknownCommand), "{word:?}");
        }
    }

    #[test]
    fn help_has_three_spellings() {
        for word in ["help", "--help", "-h"] {
            assert_eq!(parse_all(&[word]), Ok(Command::Help), "{word}");
        }
    }

    #[test]
    fn an_unknown_word_is_refused() {
        for word in ["seed", "SERVE", "--serve", "", "serve ", "-x", "--", "ca-init"] {
            assert_eq!(parse_all(&[word]), Err(CliError::UnknownCommand), "{word:?}");
        }
    }

    #[test]
    fn extra_arguments_are_refused() {
        assert_eq!(parse_all(&["serve", "now"]), Err(CliError::UnexpectedArgument));
        assert_eq!(parse_all(&["help", "serve"]), Err(CliError::UnexpectedArgument));
        assert_eq!(parse_all(&["help", "migrate"]), Err(CliError::UnexpectedArgument));
    }

    #[test]
    fn an_error_never_repeats_the_argument() {
        let planted = "hunter2-planted-secret";
        for args in [vec![planted], vec!["serve", planted]] {
            let error = parse(args).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("hunter2"));
        }
    }

    #[test]
    fn the_usage_text_is_the_golden_one() {
        insta::assert_snapshot!(USAGE, @r"
        usage: hub [serve]
               hub migrate
               hub help

        serve    run the hub until SIGTERM or SIGINT (the default); settings are the LK_* environment variables
        migrate  apply the database migrations as lanekeeper_migrator and exit; needs LK_MIGRATOR_URL_FILE and LK_DB_CA_FILE
        help     print this text
        ");
    }

    proptest::proptest! {
        /// Any argument list is either a command or one of the two errors, and never a panic.
        #[test]
        fn arbitrary_arguments_never_panic(args in proptest::collection::vec(".{0,24}", 0..4)) {
            let _ = parse(args.iter().map(String::as_str));
        }
    }
}
